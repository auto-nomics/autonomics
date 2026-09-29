//! Central skill manager: the process-wide authority for the skill
//! library.
//!
//! One invariant, learned from the EvoScientist codebase (its
//! `skills_manager` carried this after reviewers called out the
//! "vibes and hope" pattern on PR #371): **every mutation fires the
//! change notification at the mutation point**, so no caller has to
//! remember. Install and uninstall are methods here for exactly that
//! reason — free functions in [`crate::install`] remain available to
//! callers with no process state (the CLI in a one-shot process),
//! but anything running inside the daemon must mutate through the
//! manager or subscribers will silently go stale.
//!
//! The manager deliberately does **not** cache registry contents —
//! scans stay always-fresh (see [`crate::registry`]). What it adds:
//!
//! - a **generation counter**, bumped on every successful mutation.
//!   Consumers that do cache (a prompt-index snapshot, a skill browser
//!   listing) key their cache on `generation()` instead of inventing
//!   their own invalidation.
//! - a **change broadcast** (`tokio::sync::broadcast`) carrying the
//!   new generation. Fire-and-forget: a misbehaving or absent
//!   subscriber can never break the install path, mirroring the
//!   swallowed-exception contract upstream.
//! - **usage telemetry**, the fitness signal for skill evolution: the
//!   tools record every `get`/`search`/`run`/`eval` here, and the
//!   future distillation loop reads the snapshot to decide which
//!   skills earn their context-window cost and which decay.
//!
//! ## Singleton
//!
//! [`init`] installs the process-wide instance (the runtime does this
//! from `SharedInfra::open`); [`global`] lazily constructs a default
//! from the environment when nobody initialized one. Re-`init` is
//! allowed — tests and embedded hosts may swap configurations — with
//! the usual consequence that previously handed-out `Arc`s keep
//! pointing at the old instance.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::broadcast;

use crate::error::SkillError;
use crate::install::{self, InstallOutcome};
use crate::registry::{SkillRegistry, SkillTier};

/// What a skill was used for. Evolution cares about the difference
/// between "listed in a prompt" (free), "read" (mild interest),
/// "run" (real adoption), and "eval'd" (being verified).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageKind {
    Get,
    SearchHit,
    Run,
    Eval,
}

impl UsageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            UsageKind::Get => "get",
            UsageKind::SearchHit => "search_hit",
            UsageKind::Run => "run",
            UsageKind::Eval => "eval",
        }
    }
}

/// Per-skill usage counters. In-memory for now; the evolution loop
/// (V2) will persist a snapshot alongside proposals.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UsageRecord {
    pub gets: u64,
    pub search_hits: u64,
    pub runs: u64,
    pub evals: u64,
    /// Unix seconds of the last recorded use; 0 when never used.
    pub last_used: i64,
}

impl UsageRecord {
    fn record(&mut self, kind: UsageKind, now: i64) {
        match kind {
            UsageKind::Get => self.gets += 1,
            UsageKind::SearchHit => self.search_hits += 1,
            UsageKind::Run => self.runs += 1,
            UsageKind::Eval => self.evals += 1,
        }
        self.last_used = now;
    }

    pub fn total(&self) -> u64 {
        self.gets + self.search_hits + self.runs + self.evals
    }
}

/// The central manager. Cloning the `Arc` is how it is shared.
pub struct SkillManager {
    state_dir: PathBuf,
    /// Extra roots layered above the global tier, in ascending
    /// priority order.
    workspace_roots: Vec<PathBuf>,
    generation: AtomicU64,
    changes: broadcast::Sender<u64>,
    usage: RwLock<HashMap<String, UsageRecord>>,
}

static GLOBAL: arc_swap::ArcSwapOption<SkillManager> = arc_swap::ArcSwapOption::const_empty();

impl SkillManager {
    /// Install the process-wide manager. Returns the installed `Arc`.
    /// Calling again replaces the instance (see module docs on re-init).
    pub fn init(manager: SkillManager) -> std::sync::Arc<SkillManager> {
        let arc = std::sync::Arc::new(manager);
        GLOBAL.store(Some(std::sync::Arc::clone(&arc)));
        arc
    }

    /// The process-wide manager, lazily constructed from the environment
    /// (`$AUTONOMICS_STATE_DIR` or `~/.autonomics`) when [`init`] was
    /// never called.
    pub fn global() -> std::sync::Arc<SkillManager> {
        if let Some(existing) = GLOBAL.load().as_ref() {
            return std::sync::Arc::clone(existing);
        }
        Self::init(Self::from_env())
    }
    /// Standard layout: global tier at `<state_dir>/skills`.
    #[must_use]
    pub fn new(state_dir: impl Into<PathBuf>) -> Self {
        Self {
            state_dir: state_dir.into(),
            workspace_roots: Vec::new(),
            generation: AtomicU64::new(1),
            changes: broadcast::channel(16).0,
            usage: RwLock::new(HashMap::new()),
        }
    }

    /// Default from the environment (see [`global`]).
    #[must_use]
    pub fn from_env() -> Self {
        Self::new(crate::default_state_dir())
    }

    /// Layer a workspace tier root above everything added so far.
    #[must_use]
    pub fn with_workspace_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.workspace_roots.push(root.into());
        self
    }

    /// Build a fresh registry view from the configured roots. The
    /// manager holds configuration, never scanned state — see the
    /// always-fresh contract in [`crate::registry`].
    #[must_use]
    pub fn registry(&self) -> SkillRegistry {
        let mut registry = SkillRegistry::standard(&self.state_dir);
        for root in &self.workspace_roots {
            registry = registry.with_root(SkillTier::Workspace, root.clone());
        }
        registry
    }

    /// Monotonic library version. Bumped by every successful mutation;
    /// cache-key your derived state on this value.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Subscribe to change notifications. Each message is the new
    /// generation. Lagging receivers miss intermediate generations —
    /// by design, the generation is the payload, not the diff.
    pub fn subscribe(&self) -> broadcast::Receiver<u64> {
        self.changes.subscribe()
    }

    /// The tier roots in ascending priority order (for callers that
    /// need the filesystem layout, e.g. install targets).
    pub fn tier_roots(&self) -> Vec<PathBuf> {
        let mut roots = vec![self.state_dir.join("skills")];
        roots.extend(self.workspace_roots.iter().cloned());
        roots
    }

    // ── mutation: every path bumps the generation and broadcasts ──

    /// Install from a local path into the global tier. Returns the
    /// outcome; a successful install bumps the generation and fires
    /// the change broadcast — no caller has to remember.
    pub fn install_local(&self, source: &Path) -> Result<InstallOutcome, SkillError> {
        let record = install::InstallRecord {
            source: source.display().to_string(),
            commit: None,
            installed_at: unix_now(),
        };
        let dest = self.state_dir.join("skills");
        let outcome = install::install_from_local(source, &dest, Some(&record))?;
        self.notify_if_changed(&outcome);
        Ok(outcome)
    }

    /// Install from a git source (URL or shorthand) into the global
    /// tier, recording the cloned HEAD as provenance.
    pub fn install_git(&self, source: &str) -> Result<InstallOutcome, SkillError> {
        let dest = self.state_dir.join("skills");
        let outcome = install::install_from_git(source, &dest)?;
        self.notify_if_changed(&outcome);
        Ok(outcome)
    }

    /// Uninstall by name, searching tiers in descending priority.
    pub fn uninstall(&self, name: &str) -> Result<PathBuf, SkillError> {
        let mut roots = self.tier_roots();
        roots.reverse();
        let removed = install::uninstall(name, &roots)?;
        self.notify();
        Ok(removed)
    }

    fn notify_if_changed(&self, outcome: &InstallOutcome) {
        if outcome.success() {
            self.notify();
        }
    }

    /// Bump the generation and broadcast it. Send errors (no
    /// receivers, or a full channel) are ignored — a subscriber must
    /// not be able to break the mutation path.
    fn notify(&self) {
        let next = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let _ = self.changes.send(next);
    }

    // ── telemetry: the evolution fitness signal ──

    /// Record one use of a skill. Unknown names are recorded too —
    /// usage of a shadowed-away skill is still signal.
    pub fn record_usage(&self, name: &str, kind: UsageKind) {
        let now = unix_now();
        if let Ok(mut usage) = self.usage.write() {
            usage.entry(name.to_string()).or_default().record(kind, now);
        }
    }

    /// Point-in-time copy of the usage table, sorted by total uses
    /// descending then name — the "which skills earn their keep"
    /// ranking.
    pub fn usage_snapshot(&self) -> Vec<(String, UsageRecord)> {
        let Ok(usage) = self.usage.read() else {
            return Vec::new();
        };
        let mut rows: Vec<(String, UsageRecord)> =
            usage.iter().map(|(k, v)| (k.clone(), *v)).collect();
        rows.sort_by(|a, b| b.1.total().cmp(&a.1.total()).then(a.0.cmp(&b.0)));
        rows
    }

    // ── evolution loop: observations, distillation, proposals ──

    /// The observation store under this manager's state dir.
    pub fn observations(&self) -> crate::observation::ObservationStore {
        crate::observation::ObservationStore::open(&self.state_dir)
    }

    /// Record one observation (idempotent on content). This is the
    /// feedstock call for the whole evolution loop — agent tool,
    /// automatic failure capture, and CLI all land here.
    pub fn record_observation(
        &self,
        input: crate::observation::ObservationInput,
    ) -> Result<crate::observation::Observation, SkillError> {
        self.observations().record(input)
    }

    /// The proposal area under this manager's state dir.
    pub fn proposals(&self) -> crate::proposals::Proposals {
        crate::proposals::Proposals::open(&self.state_dir)
    }

    /// Run one deterministic distillation pass: cluster anchored
    /// observations, write proposals for unconsumed clusters.
    pub fn distill(&self) -> Result<crate::distill::DistillReport, SkillError> {
        let observations = self.observations().list();
        crate::distill::distill(&observations, &self.proposals())
    }

    /// Approve a pending proposal into the global tier. Bumps the
    /// generation — the approved skill is live for the next agent
    /// build and every cached view invalidates.
    pub fn approve_proposal(
        &self,
        name: &str,
    ) -> Result<crate::proposals::ApproveOutcome, SkillError> {
        let outcome = self
            .proposals()
            .approve(name, &self.state_dir.join("skills"))?;
        self.notify();
        Ok(outcome)
    }

    /// Reject a pending proposal; its cluster is consumed and will
    /// not re-propose.
    pub fn reject_proposal(&self, name: &str) -> Result<crate::proposals::Proposal, SkillError> {
        let proposal = self.proposals().reject(name)?;
        self.notify();
        Ok(proposal)
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager_in(tmp: &std::path::Path) -> SkillManager {
        SkillManager::new(tmp.join("state"))
    }

    fn make_skill(root: &std::path::Path, name: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: d\n---\nb\n"),
        )
        .unwrap();
    }

    #[test]
    fn install_uninstall_bump_generation_and_broadcast() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = manager_in(tmp.path());
        let mut rx = manager.subscribe();
        assert_eq!(manager.generation(), 1);

        make_skill(tmp.path(), "alpha");
        let outcome = manager.install_local(&tmp.path().join("alpha")).unwrap();
        assert!(outcome.success());
        assert_eq!(manager.generation(), 2);
        assert_eq!(rx.try_recv().unwrap(), 2);

        // The installed skill is visible through the view.
        let names: Vec<String> = manager
            .registry()
            .list()
            .into_iter()
            .map(|e| e.meta.name)
            .collect();
        assert!(names.contains(&"alpha".to_string()));

        manager.uninstall("alpha").unwrap();
        assert_eq!(manager.generation(), 3);
        assert_eq!(rx.try_recv().unwrap(), 3);
    }

    #[test]
    fn failed_install_does_not_bump() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = manager_in(tmp.path());
        // Empty dir: a hard error (no skill found). Neither a hard
        // error nor a no-skill outcome may move the generation.
        assert!(manager.install_local(tmp.path()).is_err());
        assert_eq!(manager.generation(), 1);

        // A dir that holds one valid and one invalid skill yields a
        // partial outcome — success bumps, so this is a change.
        make_skill(tmp.path(), "ok");
        let broken = tmp.path().join("broken");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join("SKILL.md"), "no frontmatter").unwrap();
        let outcome = manager.install_local(tmp.path()).unwrap();
        assert!(outcome.success());
        assert_eq!(manager.generation(), 2);
    }

    #[test]
    fn absent_subscribers_cannot_break_mutation() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = manager_in(tmp.path());
        // No subscriber at all; install still succeeds.
        make_skill(tmp.path(), "beta");
        assert!(manager.install_local(&tmp.path().join("beta")).is_ok());
    }

    #[test]
    fn usage_accumulates_and_ranks() {
        let manager = SkillManager::new("/nonexistent");
        manager.record_usage("popular", UsageKind::Run);
        manager.record_usage("popular", UsageKind::Get);
        manager.record_usage("quiet", UsageKind::Get);
        let snapshot = manager.usage_snapshot();
        assert_eq!(snapshot[0].0, "popular");
        assert_eq!(snapshot[0].1.total(), 2);
        assert_eq!(snapshot[0].1.runs, 1);
        assert!(snapshot[0].1.last_used > 0);
        assert_eq!(snapshot[1].0, "quiet");
    }

    #[test]
    fn singleton_init_and_lazy_global() {
        let tmp = tempfile::tempdir().unwrap();
        let arc = SkillManager::init(manager_in(tmp.path()));
        let fetched = SkillManager::global();
        assert!(std::sync::Arc::ptr_eq(&arc, &fetched));

        // Re-init replaces the global; old Arc stays valid.
        let arc2 = SkillManager::init(SkillManager::new(tmp.path().join("other")));
        assert!(std::sync::Arc::ptr_eq(&arc2, &SkillManager::global()));
        assert!(!std::sync::Arc::ptr_eq(&arc, &arc2));
    }

    #[test]
    fn tier_roots_order_workspace_above_global() {
        let manager = SkillManager::new("/state").with_workspace_root("/ws");
        assert_eq!(
            manager.tier_roots(),
            vec![PathBuf::from("/state/skills"), PathBuf::from("/ws")]
        );
        let registry = manager.registry();
        assert_eq!(registry.roots().len(), 2);
    }
}
