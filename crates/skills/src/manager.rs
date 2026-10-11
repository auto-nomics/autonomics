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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};

use crate::error::SkillError;
use crate::install::{self, InstallOutcome};
use crate::registry::{SkillRegistry, SkillTier};
use evolution_core::embedding::{ObservationEmbedder, SharedObservationEmbedder};

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
/// Serialized in `<state_dir>/skill-usage.toml`; loading merges by
/// per-field maxima so restarts never lose counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
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

/// Persisted usage table filename under the state dir.
const USAGE_FILE: &str = "skill-usage.toml";

/// The central manager. Cloning the `Arc` is how it is shared.
pub struct SkillManager {
    state_dir: PathBuf,
    /// Extra roots layered above the global tier, in ascending
    /// priority order.
    workspace_roots: Vec<PathBuf>,
    generation: AtomicU64,
    changes: broadcast::Sender<u64>,
    usage: RwLock<HashMap<String, UsageRecord>>,
    /// Optional forwarder to the skill control service — set by
    /// [`SkillManager::attach_evolution`]. Storing just the sender
    /// (not the handle) keeps the manager free of any service
    /// lifetime coupling.
    evolution_tx: RwLock<Option<mpsc::Sender<crate::evolution::SkillCommand>>>,
    /// The upstream observation hub this manager reads evidence from.
    /// Held by reference, not resolved through the global on every
    /// call: parallel tests replace the global singleton, and an
    /// instance must keep seeing the state dir it was built with.
    hub: Arc<evolution_core::ObservationHub>,
    /// Optional semantic embedding provider for observation clustering.
    embedder: RwLock<Option<Arc<dyn ObservationEmbedder>>>,
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
    ///
    /// Also installs the process-wide [`ObservationHub`] over the same
    /// state dir: observations are upstream of skills, and the manager
    /// must never read them from a different directory than the hub
    /// hands to every other consumer.
    #[must_use]
    pub fn new(state_dir: impl Into<PathBuf>) -> Self {
        let state_dir = state_dir.into();
        let hub = Arc::new(evolution_core::ObservationHub::new(state_dir.clone()));
        Self {
            state_dir,
            workspace_roots: Vec::new(),
            generation: AtomicU64::new(1),
            changes: broadcast::channel(16).0,
            usage: RwLock::new(HashMap::new()),
            evolution_tx: RwLock::new(None),
            hub,
            embedder: RwLock::new(None),
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
    /// usage of a shadowed-away skill is still signal. In-memory on
    /// the hot path; the evolution cycle persists the table (see
    /// [`SkillManager::persist_usage`]).
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

    /// Persist the usage table to `<state_dir>/skill-usage.toml` so
    /// the fitness signal survives daemon restarts. Called by each
    /// evolution cycle; atomic-ish via temp-file + rename because a
    /// torn table must never block the loop.
    pub fn persist_usage(&self) -> Result<(), SkillError> {
        let Ok(usage) = self.usage.read() else {
            return Ok(());
        };
        let path = self.state_dir.join(USAGE_FILE);
        std::fs::create_dir_all(&self.state_dir)?;
        let text =
            toml::to_string_pretty(&*usage).map_err(|_| SkillError::BadManifest(path.clone()))?;
        let tmp = self
            .state_dir
            .join(format!(".{USAGE_FILE}.{}.tmp", std::process::id()));
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Load a persisted usage table, **merging** into the in-memory
    /// one (per-field maxima) so restarts never lose counts and
    /// concurrent CLI usage and daemon usage both survive. A corrupt
    /// or missing file reads as empty.
    pub fn load_usage(&self) {
        let path = self.state_dir.join(USAGE_FILE);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        let Ok(persisted) = toml::from_str::<HashMap<String, UsageRecord>>(&text) else {
            tracing::warn!(path = %path.display(), "malformed usage table; starting fresh");
            return;
        };
        if let Ok(mut usage) = self.usage.write() {
            for (name, record) in persisted {
                let slot = usage.entry(name).or_default();
                slot.gets = slot.gets.max(record.gets);
                slot.search_hits = slot.search_hits.max(record.search_hits);
                slot.runs = slot.runs.max(record.runs);
                slot.evals = slot.evals.max(record.evals);
                slot.last_used = slot.last_used.max(record.last_used);
            }
        }
    }

    // ── evolution loop: observations, distillation, proposals ──

    /// Every recorded observation, resolved through this manager's
    /// hub storage.
    ///
    /// Observations are upstream of skills: the hub owns the ledger
    /// and this accessor only hands its view to skill-side callers.
    pub async fn observations(&self) -> Vec<evolution_core::observation::Observation> {
        self.hub.storage().list().await
    }

    /// The hub this manager reads evidence from.
    pub fn observation_hub(&self) -> Arc<evolution_core::ObservationHub> {
        Arc::clone(&self.hub)
    }

    /// Record one observation (idempotent on content). This is the
    /// feedstock call for the whole evolution loop — agent tool,
    /// automatic failure capture, and CLI all land here.
    /// Forward observation events to the skill control service, when
    /// one is attached. Never blocks: a full or absent channel only
    /// means this observation rides the next timer/startup sweep
    /// instead.
    pub fn attach_evolution(&self, handle: &crate::evolution::SkillControlHandle) {
        if let Ok(mut slot) = self.evolution_tx.write() {
            *slot = Some(handle.sender_for_manager());
        }
    }

    pub async fn record_observation(
        &self,
        input: evolution_core::observation::ObservationInput,
    ) -> Result<evolution_core::observation::Observation, SkillError> {
        // The hub is the canonical ledger; this wrapper only adds the
        // skill-side notification so a fresh observation can trigger
        // an immediate evolution cycle.
        let observation = self
            .hub
            .record(input)
            .await
            .map_err(SkillError::Evolution)?;
        if let Ok(slot) = self.evolution_tx.read()
            && let Some(tx) = slot.as_ref()
        {
            let _ = tx.try_send(crate::evolution::SkillCommand::Evolution(
                crate::evolution::EvolutionTrigger::ObservationRecorded {
                    id: observation.id.clone(),
                },
            ));
        }
        Ok(observation)
    }

    /// The proposal area under this manager's state dir.
    pub fn proposals(&self) -> crate::proposals::Proposals {
        crate::proposals::Proposals::open(&self.state_dir)
    }

    /// Run one deterministic distillation pass for skill-eligible evidence.
    ///
    /// Observations are shared across evolution subsystems. A route ledger
    /// entry explicitly selects which observations reach skill distillation;
    /// legacy un-routed observations remain skill-eligible for compatibility.
    pub async fn distill(&self) -> Result<crate::distill::DistillReport, SkillError> {
        let observations = self.skill_eligible_observations().await;
        let embedder = self.embedder()?;
        // Warm the vector cache for this embedder's signature; only
        // cache misses reach the model, and freshly computed vectors
        // are persisted afterwards so the next pass is all hits.
        let signature = embedder.signature();
        let ids: Vec<String> = observations.iter().map(|o| o.id.clone()).collect();
        let cache = evolution_core::load_embedding_map(
            self.hub.storage().as_ref(),
            &ids,
            &signature,
        )
        .await
        .map_err(SkillError::Evolution)?;
        let cached = evolution_core::CachedObservationEmbedder::new(
            embedder,
            cache.into_iter().collect(),
        );
        let report = crate::distill::distill_with_embedder(
            &observations,
            &self.proposals(),
            &self.registry(),
            &cached,
        )?;
        let storage = self.hub.storage();
        for (id, vector) in cached.take_computed() {
            storage
                .save_embedding(&id, &signature, &vector)
                .await
                .map_err(SkillError::Evolution)?;
        }
        Ok(report)
    }

    /// Replace the observation embedder used by distillation. Passing
    /// `None` restores the offline hashing embedder.
    pub fn set_embedder(&self, embedder: Option<Arc<dyn ObservationEmbedder>>) {
        if let Ok(mut slot) = self.embedder.write() {
            *slot = embedder;
        }
    }

    fn embedder(&self) -> Result<Arc<dyn ObservationEmbedder>, SkillError> {
        self.embedder
            .read()
            .map_err(|_| {
                evolution_core::Error::Embedding("embedding provider lock poisoned".into())
            })?
            .clone()
            .map(Ok)
            .unwrap_or_else(|| Ok(Arc::new(SharedObservationEmbedder::hashing())))
    }

    async fn skill_eligible_observations(&self) -> Vec<evolution_core::Observation> {
        let observations = self.hub.storage().list().await;
        let routes = self.hub.routes().list();
        let mut routed = std::collections::HashSet::new();
        let mut eligible = std::collections::HashSet::new();
        for route in routes {
            routed.insert(route.observation_id.clone());
            if matches!(
                route.audience,
                evolution_core::ObservationAudience::Skill
                    | evolution_core::ObservationAudience::Both
            ) {
                eligible.insert(route.observation_id);
            }
        }
        observations
            .into_iter()
            .filter(|observation| {
                eligible.contains(&observation.id) || !routed.contains(&observation.id)
            })
            .collect()
    }

    /// Approve a pending proposal into the global tier. Bumps the
    /// generation — the approved skill is live for the next agent
    /// build and every cached view invalidates.
    /// Approve a pending proposal. Creates land in the global tier;
    /// updates overlay the tier where the existing skill lives (its
    /// previous SKILL.md archived first). Either way the generation
    /// advances — the change is live for the next agent build.
    pub fn approve_proposal(
        &self,
        name: &str,
    ) -> Result<crate::proposals::ApproveOutcome, SkillError> {
        let proposals = self.proposals();
        // Route by manifest: an update targets the installed skill's
        // own tier so the overlay lands where the skill actually
        // lives (and tier shadowing keeps any deeper copy intact).
        let dest_root = match proposals.find(name) {
            Some(proposal) if proposal.update => {
                let entry = self
                    .registry()
                    .list()
                    .into_iter()
                    .find(|e| e.meta.name == name)
                    .ok_or_else(|| SkillError::NotFound(name.to_string()))?;
                let dir = entry
                    .dir()
                    .ok_or_else(|| SkillError::NotFound(name.to_string()))?
                    .to_path_buf();
                dir.parent()
                    .ok_or_else(|| SkillError::NotFound(name.to_string()))?
                    .to_path_buf()
            }
            _ => self.state_dir.join("skills"),
        };
        let outcome = proposals.approve(name, &dest_root)?;
        self.notify();
        Ok(outcome)
    }

    /// Agent-authored skill proposal: the `skill_propose` tool path.
    ///
    /// The agent supplies structured fields — name, description,
    /// tags, body — and this side assembles the frontmatter, so the
    /// model never writes raw YAML (the injection surface stays the
    /// validated fields, not arbitrary frontmatter). Guard rails:
    ///
    /// - at least one **existing** observation must back the claim
    ///   (`supporting_observation_ids` are checked against the store
    ///   — proposals must trace to evidence, not invention)
    /// - the name must not collide with an installed skill (updates
    ///   belong to the deterministic distiller; an agent wanting an
    ///   update records observations and lets the loop propose it)
    /// - the name must not collide with an existing proposal either —
    ///   one draft per name, checked **before** anything is written so
    ///   a retry can never overwrite a queued (or rejected) draft
    /// - the full strict contract applies (kebab-case, description
    ///   rules, no TODO placeholders)
    /// - the manifest is marked `authored_by = "agent"`, which the
    ///   auto-approval path refuses unconditionally — human review
    ///   only
    pub async fn propose_skill(
        &self,
        name: &str,
        description: &str,
        tags: &[String],
        body: &str,
        supporting_observation_ids: &[String],
        rationale: &str,
    ) -> Result<crate::proposals::Proposal, SkillError> {
        if supporting_observation_ids.is_empty() {
            return Err(SkillError::invalid_frontmatter(
                "<skill_propose>",
                "at least one supporting observation id is required — record the evidence with evo_observe first",
            ));
        }
        let known: std::collections::BTreeSet<String> =
            self.observations().await.into_iter().map(|o| o.id).collect();
        let unknown: Vec<&String> = supporting_observation_ids
            .iter()
            .filter(|id| !known.contains(*id))
            .collect();
        if !unknown.is_empty() {
            return Err(SkillError::invalid_frontmatter(
                "<skill_propose>",
                format!(
                    "unknown observation id(s): {} — every supporting id must \
                     exist in the observation store",
                    unknown
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
        if self.registry().list().iter().any(|e| e.meta.name == name) {
            return Err(SkillError::invalid_frontmatter(
                "<skill_propose>",
                format!(
                    "a skill named {name:?} is already installed; agent proposals \
                     are create-only — record observations and let the loop \
                     propose the update"
                ),
            ));
        }

        // Assemble the document from validated fields only.
        let tags_line = if tags.is_empty() {
            String::new()
        } else {
            format!("tags: [{}]", tags.join(", "))
        };
        let content =
            format!("---\nname: {name}\ndescription: {description}\n{tags_line}\n---\n\n{body}\n");
        let proposals = self.proposals();
        // Refuse a name collision before writing: submit_as would
        // refuse it too, but only after this side had already
        // overwritten the queued draft's SKILL.md.
        if let Some(existing) = proposals.find(name) {
            return Err(SkillError::invalid_frontmatter(
                "<skill_propose>",
                format!(
                    "a proposal named {name:?} already exists with status {} — \
                     one draft per name, and a rejected name never revives",
                    existing.status.as_str()
                ),
            ));
        }
        let dir = proposals.root().join(name);

        // The cluster hash for an authored proposal is the hash of
        // its supporting evidence — stable per evidence set, and
        // distinct from distiller hashes by construction.
        let ids: std::collections::BTreeSet<String> =
            supporting_observation_ids.iter().cloned().collect();
        let submitted = (|| {
            std::fs::create_dir_all(&dir)?;
            std::fs::write(dir.join("SKILL.md"), &content)?;
            proposals.submit_as(
                name,
                &crate::proposals::cluster_hash(&ids),
                rationale,
                supporting_observation_ids.to_vec(),
                false,
                "agent".to_string(),
            )
        })();

        submitted.inspect_err(|_| {
            // A refused submission must not leave an orphan draft
            // behind — remove what we just wrote so a retry starts
            // clean and listings stay honest.
            if let Err(cleanup) = std::fs::remove_dir_all(&dir) {
                tracing::warn!(
                    path = %dir.display(),
                    error = %cleanup,
                    "cannot remove refused proposal draft"
                );
            }
        })
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
    use evolution_core::routing::{ObservationRouteStore, observation_route};

    fn manager_in(tmp: &std::path::Path) -> SkillManager {
        SkillManager::new(tmp.join("state"))
    }

    #[tokio::test]
    async fn distillation_respects_observation_routes() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = manager_in(tmp.path());
        let routes = ObservationRouteStore::open(&tmp.path().join("state"));

        for index in 0..3 {
            let observation = manager
                .record_observation(evolution_core::ObservationInput {
                    kind: evolution_core::ObservationKind::Failure,
                    source: evolution_core::ObservationSource::Agent,
                    summary: format!("plugin failure {index}"),
                    body: format!("Fix attempt {index}."),
                    node_kind: Some("plugin_adapter".into()),
                    error: Some("adapter rejected input".into()),
                }).await
                .unwrap();
            let (_, route) = observation_route(&observation, Some("demo-plugin"), None);
            routes.record(route).unwrap();
        }
        for index in 0..3 {
            let observation = manager
                .record_observation(evolution_core::ObservationInput {
                    kind: evolution_core::ObservationKind::Recipe,
                    source: evolution_core::ObservationSource::Agent,
                    summary: format!("generic recipe {index}"),
                    body: format!("Useful generic procedure {index}."),
                    node_kind: Some("generic_node".into()),
                    error: None,
                }).await
                .unwrap();
            let (_, route) = observation_route(&observation, None, None);
            routes.record(route).unwrap();
        }

        manager.distill().await.unwrap();
        let proposals = manager.proposals().list();
        assert_eq!(proposals.len(), 1, "{proposals:?}");
        assert_eq!(proposals[0].name, "generic-node-generic-recipe-recipe");
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

    #[tokio::test]
    async fn install_uninstall_bump_generation_and_broadcast() {
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

    #[tokio::test]
    async fn failed_install_does_not_bump() {
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

    #[tokio::test]
    async fn absent_subscribers_cannot_break_mutation() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = manager_in(tmp.path());
        // No subscriber at all; install still succeeds.
        make_skill(tmp.path(), "beta");
        assert!(manager.install_local(&tmp.path().join("beta")).is_ok());
    }

    #[tokio::test]
    async fn usage_accumulates_and_ranks() {
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

    #[tokio::test]
    async fn singleton_init_and_lazy_global() {
        let tmp = tempfile::tempdir().unwrap();
        let arc = SkillManager::init(manager_in(tmp.path()));
        let fetched = SkillManager::global();
        assert!(std::sync::Arc::ptr_eq(&arc, &fetched));

        // Re-init replaces the global; old Arc stays valid.
        let arc2 = SkillManager::init(SkillManager::new(tmp.path().join("other")));
        assert!(std::sync::Arc::ptr_eq(&arc2, &SkillManager::global()));
        assert!(!std::sync::Arc::ptr_eq(&arc, &arc2));
    }

    #[tokio::test]
    async fn tier_roots_order_workspace_above_global() {
        let manager = SkillManager::new("/state").with_workspace_root("/ws");
        assert_eq!(
            manager.tier_roots(),
            vec![PathBuf::from("/state/skills"), PathBuf::from("/ws")]
        );
        let registry = manager.registry();
        assert_eq!(registry.roots().len(), 2);
    }
}

#[cfg(test)]
mod usage_persistence_tests {
    use super::*;

    #[tokio::test]
    async fn usage_survives_restart_by_field_maxima() {
        let tmp = tempfile::tempdir().unwrap();
        let first = SkillManager::new(tmp.path());
        first.record_usage("skill-a", UsageKind::Run);
        first.record_usage("skill-a", UsageKind::Get);
        first.record_usage("skill-b", UsageKind::Get);
        first.persist_usage().unwrap();
        assert!(tmp.path().join("skill-usage.toml").is_file());

        // A fresh manager (restart) loads and keeps the ranking.
        let second = SkillManager::new(tmp.path());
        second.load_usage();
        let snapshot = second.usage_snapshot();
        assert_eq!(snapshot[0].0, "skill-a");
        assert_eq!(snapshot[0].1.runs, 1);

        // Concurrent growth in both processes merges by maxima: the
        // in-memory manager had 1 get, the persisted one 1 get, and a
        // new run on the fresh manager pushes runs to 2.
        second.record_usage("skill-a", UsageKind::Run);
        second.persist_usage().unwrap();
        let third = SkillManager::new(tmp.path());
        third.load_usage();
        let record = &third.usage_snapshot()[0].1;
        assert_eq!(record.runs, 2);
        assert_eq!(record.gets, 1);
    }

    #[tokio::test]
    async fn corrupt_usage_table_reads_as_empty() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("skill-usage.toml"), "not [ valid").unwrap();
        let manager = SkillManager::new(tmp.path());
        manager.load_usage();
        assert!(manager.usage_snapshot().is_empty());
    }
}

#[cfg(test)]
mod propose_tests {
    use super::*;

    fn manager_in(tmp: &std::path::Path) -> SkillManager {
        SkillManager::new(tmp.join("state"))
    }

    async fn observe_once(manager: &SkillManager, body: &str) -> String {
        manager
            .record_observation(evolution_core::ObservationInput {
                kind: evolution_core::ObservationKind::Failure,
                source: evolution_core::ObservationSource::Agent,
                summary: "evidence".into(),
                body: body.into(),
                node_kind: Some("sql".into()),
                error: Some("boom".into()),
            }).await
            .unwrap()
            .id
    }

    #[tokio::test]
    async fn agent_proposal_lands_pending_and_marked() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = manager_in(tmp.path());
        let id = observe_once(&manager, "the fix").await;

        let proposal = manager
            .propose_skill(
                "agent-crafted-recipes",
                "Recipes distilled by the agent from real work.",
                &["agent".to_string()],
                "# Recipes\n\nStep one.\n",
                std::slice::from_ref(&id),
                "recurring pattern",
            ).await
            .unwrap();
        assert_eq!(proposal.authored_by, "agent");
        assert_eq!(proposal.status, crate::proposals::ProposalStatus::Pending);
        assert!(
            tmp.path()
                .join("state/skill-proposals/agent-crafted-recipes/SKILL.md")
                .is_file()
        );
        // The written document passes the strict contract.
        assert!(
            manager
                .proposals()
                .validate_dir("agent-crafted-recipes")
                .is_empty()
        );
        // Frontmatter was assembled from the validated fields.
        let content = std::fs::read_to_string(
            tmp.path()
                .join("state/skill-proposals/agent-crafted-recipes/SKILL.md"),
        )
        .unwrap();
        assert!(content.contains("name: agent-crafted-recipes"));
        assert!(content.contains("tags: [agent]"));
    }

    #[tokio::test]
    async fn evidence_must_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = manager_in(tmp.path());
        let err = manager
            .propose_skill("x", "d", &[], "b", &["O-doesnotexist".to_string()], "r").await
            .unwrap_err();
        assert!(err.to_string().contains("unknown observation id"), "{err}");
    }

    #[tokio::test]
    async fn evidence_is_mandatory() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = manager_in(tmp.path());
        let err = manager
            .propose_skill("x", "d", &[], "b", &[], "r").await
            .unwrap_err();
        assert!(err.to_string().contains("supporting observation"));
    }

    #[tokio::test]
    async fn create_only_no_collisions_with_installed_skills() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = manager_in(tmp.path());
        let id = observe_once(&manager, "e").await;
        // Install a skill with the target name first.
        let dir = tmp.path().join("state/skills/taken-name");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: taken-name\ndescription: d\n---\nb\n",
        )
        .unwrap();
        let err = manager
            .propose_skill("taken-name", "d", &[], "b", std::slice::from_ref(&id), "r").await
            .unwrap_err();
        assert!(err.to_string().contains("already installed"), "{err}");
    }

    #[tokio::test]
    async fn refused_submission_leaves_no_orphan_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = manager_in(tmp.path());
        let id = observe_once(&manager, "e").await;
        // A TODO placeholder fails the strict contract at submit time,
        // after the draft was written — the directory must go with the
        // refusal.
        let err = manager
            .propose_skill(
                "orphan-draft",
                "d",
                &[],
                "# Body\n\nTODO: fill this in\n",
                std::slice::from_ref(&id),
                "r",
            ).await
            .unwrap_err();
        assert!(err.to_string().contains("TODO"), "{err}");
        assert!(
            !tmp.path()
                .join("state/skill-proposals/orphan-draft")
                .exists(),
            "refused draft must not linger in the proposal area"
        );
        assert!(manager.proposals().list().is_empty());
    }

    #[tokio::test]
    async fn same_name_retry_never_overwrites_a_queued_draft() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = manager_in(tmp.path());
        let id = observe_once(&manager, "e").await;
        manager
            .propose_skill(
                "one-draft-per-name",
                "First.",
                &[],
                "# First\n\nOriginal body.\n",
                std::slice::from_ref(&id),
                "r",
            ).await
            .unwrap();
        let err = manager
            .propose_skill(
                "one-draft-per-name",
                "Second.",
                &[],
                "# Second\n\nOverwrite attempt.\n",
                std::slice::from_ref(&id),
                "r2",
            ).await
            .unwrap_err();
        assert!(err.to_string().contains("already exists"), "{err}");
        let content = std::fs::read_to_string(
            tmp.path()
                .join("state/skill-proposals/one-draft-per-name/SKILL.md"),
        )
        .unwrap();
        assert!(content.contains("Original body."));
        assert!(!content.contains("Overwrite attempt."));
    }
}
