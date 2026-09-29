//! The evolution loop, split into two halves bridged by a tokio
//! channel:
//!
//! - **Workflow** ([`run_evolution_cycle`]): the idempotent "what
//!   happens" — snapshot observations, distill proposals (create or
//!   update), apply the approval policy. Pure, synchronous, cheap;
//!   the CLI calls it directly, the service calls it per event batch.
//! - **Triggers** ([`EvolutionHandle`]): the "when to try" — manual,
//!   observation-recorded, startup, timer. A trigger is only a
//!   wake-up signal: every condition that matters (cluster
//!   thresholds, consumed hashes, delta bars) is re-evaluated inside
//!   the workflow itself, so a spurious trigger costs one no-op scan
//!   and a missed trigger only delays the next one. Triggers can be
//!   added freely without touching the workflow, and vice versa.
//!
//! The bridge is a bounded `mpsc` channel with `try_send`: a full
//! channel drops the event and counts it (never blocks the caller —
//! the same "subscriber cannot break the mutation path" contract the
//! manager's broadcast carries), and the worker coalesces bursts
//! within a quiet window into one cycle, so a failing eval storm
//! produces one distillation pass, not twenty.
//!
//! ## Safety rails on automation
//!
//! [`EvolutionPolicy`] gates auto-approval separately from
//! distillation: by default the cycle only *writes pending
//! proposals* (human review stays in the loop), and even with
//! `auto_approve` enabled it is capped per cycle, only ever touches
//! loop-created skills (human-owned names are refused at distill
//! time), and every promotion still passes the full validation
//! contract.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::{broadcast, mpsc};

use crate::error::SkillError;
use crate::manager::SkillManager;
use crate::proposals::ProposalStatus;

/// Why an evolution cycle was attempted. Pure signal — the workflow
/// re-derives everything it needs from the stores.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvolutionTrigger {
    /// Explicit request (CLI, agent tool, future host control).
    Manual { by: String },
    /// An observation was recorded (covers agent `skill_observe`,
    /// automatic eval-failure capture, CLI `observe`).
    ObservationRecorded { id: String },
    /// Service start: sweep once so overnight accumulation is dealt
    /// with before anyone asks.
    Startup,
    /// Periodic sweep.
    Timer,
}

impl EvolutionTrigger {
    pub fn label(&self) -> &'static str {
        match self {
            EvolutionTrigger::Manual { .. } => "manual",
            EvolutionTrigger::ObservationRecorded { .. } => "observation",
            EvolutionTrigger::Startup => "startup",
            EvolutionTrigger::Timer => "timer",
        }
    }
}

/// What one cycle may do beyond writing pending proposals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvolutionPolicy {
    /// Approve eligible pending proposals without human review.
    /// Distillation itself always runs; this only lifts the review
    /// gate, with the cap below still enforced.
    pub auto_approve: bool,
    /// Upper bound on approvals per cycle — automation advances the
    /// library in bounded steps even after a long accumulation.
    pub max_approvals_per_cycle: usize,
}

impl Default for EvolutionPolicy {
    fn default() -> Self {
        Self {
            auto_approve: false,
            max_approvals_per_cycle: 5,
        }
    }
}

/// What one cycle did.
#[derive(Debug, Clone, Default)]
pub struct EvolutionReport {
    /// Coalesced triggers that caused this cycle (labels).
    pub triggers: Vec<&'static str>,
    pub clusters_considered: usize,
    /// Proposal names written this cycle (creates and updates).
    pub proposals_written: Vec<String>,
    /// Subset of `proposals_written` that revise installed skills.
    pub updated_existing: Vec<String>,
    /// Names auto-approved under the policy.
    pub auto_approved: Vec<String>,
    /// Pending proposals left for human review.
    pub left_pending: usize,
    /// (cluster hash, reason) — skipped clusters.
    pub skipped: Vec<(String, String)>,
}

impl EvolutionReport {
    pub fn acted(&self) -> bool {
        !self.proposals_written.is_empty() || !self.auto_approved.is_empty()
    }
}

/// Service tuning knobs.
#[derive(Debug, Clone)]
pub struct EvolutionOptions {
    pub policy: EvolutionPolicy,
    /// Bursts arriving within this window run as one cycle.
    pub quiet_window: Duration,
    /// Periodic sweep cadence; `None` disables the timer.
    pub timer: Option<Duration>,
}

impl Default for EvolutionOptions {
    fn default() -> Self {
        Self {
            policy: EvolutionPolicy::default(),
            quiet_window: Duration::from_millis(500),
            timer: None,
        }
    }
}

// ────────────────────────── workflow ──────────────────────────

/// Run one evolution cycle: distill, then apply the approval policy.
///
/// Idempotent — running it twice without new observations produces a
/// no-op report (consumed clusters are skipped at distill time).
/// This is the single entry point shared by the CLI, the agent
/// tool, and the service worker.
pub fn run_evolution_cycle(
    manager: &SkillManager,
    policy: &EvolutionPolicy,
) -> Result<EvolutionReport, SkillError> {
    run_evolution_cycle_labeled(manager, policy, Vec::new())
}

/// [`run_evolution_cycle`] with the trigger labels that caused it,
/// used by the service so reports say why they ran.
pub fn run_evolution_cycle_labeled(
    manager: &SkillManager,
    policy: &EvolutionPolicy,
    triggers: Vec<&'static str>,
) -> Result<EvolutionReport, SkillError> {
    let distill_report = manager.distill()?;
    let mut report = EvolutionReport {
        triggers,
        clusters_considered: distill_report.candidates_considered,
        proposals_written: distill_report.proposals_written,
        updated_existing: distill_report.updated_existing,
        skipped: distill_report.skipped,
        ..Default::default()
    };

    let pending: Vec<String> = manager
        .proposals()
        .list()
        .into_iter()
        .filter(|p| p.status == ProposalStatus::Pending)
        .map(|p| p.name)
        .collect();

    if policy.auto_approve {
        for name in pending.iter().take(policy.max_approvals_per_cycle) {
            match manager.approve_proposal(name) {
                Ok(outcome) => {
                    tracing::info!(
                        skill = %outcome.name,
                        destination = %outcome.destination.display(),
                        "evolution cycle auto-approved a proposal"
                    );
                    report.auto_approved.push(name.clone());
                }
                Err(e) => {
                    // Validation refused it — that is the gate doing
                    // its job; surface and continue with the rest.
                    tracing::warn!(proposal = name, error = %e, "auto-approve refused");
                }
            }
        }
    }
    report.left_pending = pending.len() - report.auto_approved.len();
    Ok(report)
}

// ────────────────────────── trigger side ──────────────────────────

/// Cheap, cloneable handle for firing triggers at the evolution
/// worker. `try_send` semantics: sending to a full or closed channel
/// never blocks and only bumps the dropped counter.
#[derive(Clone)]
pub struct EvolutionHandle {
    tx: mpsc::Sender<EvolutionTrigger>,
    dropped: Arc<AtomicU64>,
    reports: broadcast::Sender<Arc<EvolutionReport>>,
}

impl EvolutionHandle {
    /// Fire a trigger. Returns false when the channel is full or the
    /// worker is gone (the event is dropped and counted — the next
    /// timer or observation will sweep again).
    pub fn trigger(&self, event: EvolutionTrigger) -> bool {
        match self.tx.try_send(event) {
            Ok(()) => true,
            Err(_) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// Triggers dropped so far because the channel was full or the
    /// worker had exited — an observability signal, not an error.
    pub fn dropped_triggers(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Subscribe to per-cycle reports (fire-and-forget broadcast;
    /// lagging receivers simply miss reports).
    pub fn subscribe_reports(&self) -> broadcast::Receiver<Arc<EvolutionReport>> {
        self.reports.subscribe()
    }

    /// The raw trigger sender, for the manager's observation
    /// forwarder. Internal-use: callers should go through
    /// [`EvolutionHandle::trigger`].
    pub(crate) fn sender_for_manager(&self) -> mpsc::Sender<EvolutionTrigger> {
        self.tx.clone()
    }
}

// ────────────────────────── service ──────────────────────────

/// Start the evolution service for one manager: spawns the worker
/// task, fires the startup sweep, and returns the trigger handle.
///
/// The worker coalesces trigger bursts within
/// `options.quiet_window` into a single cycle and, when
/// `options.timer` is set, sweeps periodically.
pub fn start(manager: Arc<SkillManager>, options: EvolutionOptions) -> EvolutionHandle {
    let (tx, rx) = mpsc::channel::<EvolutionTrigger>(64);
    let (report_tx, _) = broadcast::channel::<Arc<EvolutionReport>>(16);
    let dropped = Arc::new(AtomicU64::new(0));

    let worker = Worker {
        manager,
        options,
        reports: report_tx.clone(),
    };
    tokio::spawn(worker.run(rx));

    let handle = EvolutionHandle {
        tx,
        dropped,
        reports: report_tx,
    };
    handle.trigger(EvolutionTrigger::Startup);
    handle
}

struct Worker {
    manager: Arc<SkillManager>,
    options: EvolutionOptions,
    reports: broadcast::Sender<Arc<EvolutionReport>>,
}

impl Worker {
    async fn run(self, mut rx: mpsc::Receiver<EvolutionTrigger>) {
        // Consume the interval's immediate first tick — the explicit
        // Startup trigger already performs the boot sweep.
        let mut timer = self.options.timer.map(tokio::time::interval);
        if let Some(interval) = timer.as_mut() {
            interval.tick().await;
        }
        loop {
            // tokio's bounded recv returns Option: None means every
            // sender (the handle) is gone — shut down.
            let first = match &mut timer {
                Some(interval) => {
                    tokio::select! {
                        event = rx.recv() => match event {
                            Some(event) => event,
                            None => break,
                        },
                        _ = interval.tick() => EvolutionTrigger::Timer,
                    }
                }
                None => match rx.recv().await {
                    Some(event) => event,
                    None => break,
                },
            };
            // Quiet-window drain: everything arriving while the first
            // event settles joins the same cycle.
            let mut batch = vec![first];
            let deadline = tokio::time::Instant::now() + self.options.quiet_window;
            loop {
                let remain = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remain.is_zero() {
                    break;
                }
                match tokio::time::timeout(remain, rx.recv()).await {
                    Ok(Some(event)) => batch.push(event),
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            let labels: Vec<&'static str> = batch.iter().map(EvolutionTrigger::label).collect();
            let summary = labels.join(",");
            match run_evolution_cycle_labeled(&self.manager, &self.options.policy, labels) {
                Ok(report) => {
                    if report.acted() {
                        tracing::info!(
                            triggers = %summary,
                            written = report.proposals_written.len(),
                            auto_approved = report.auto_approved.len(),
                            left_pending = report.left_pending,
                            "evolution cycle"
                        );
                    }
                    let _ = self.reports.send(Arc::new(report));
                }
                Err(e) => {
                    tracing::warn!(triggers = %summary, error = %e, "evolution cycle failed");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observation::{
        ObservationInput, ObservationKind, ObservationSource, ObservationStore,
    };

    fn manager_in(tmp: &std::path::Path) -> Arc<SkillManager> {
        Arc::new(SkillManager::new(tmp.join("state")))
    }

    fn observe_three(manager: &SkillManager, error: &str) {
        for body in ["fix a", "fix b", "fix c"] {
            manager
                .record_observation(ObservationInput {
                    kind: ObservationKind::Failure,
                    source: ObservationSource::Agent,
                    summary: format!("fix for {error}"),
                    body: body.into(),
                    node_kind: Some("sql".into()),
                    error: Some(error.into()),
                })
                .unwrap();
        }
    }

    #[test]
    fn default_policy_leaves_proposals_pending() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = manager_in(tmp.path());
        observe_three(&manager, "boom N");
        let report = run_evolution_cycle(&manager, &EvolutionPolicy::default()).unwrap();
        assert_eq!(report.proposals_written.len(), 1);
        assert!(report.auto_approved.is_empty());
        assert_eq!(report.left_pending, 1);
    }

    #[test]
    fn auto_approve_respects_cap_and_bumps_generation() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = manager_in(tmp.path());
        // Two distinct anchors → two pending proposals.
        observe_three(&manager, "boom one");
        observe_three(&manager, "crash two");
        let policy = EvolutionPolicy {
            auto_approve: true,
            max_approvals_per_cycle: 1,
        };
        let generation = manager.generation();
        let report = run_evolution_cycle(&manager, &policy).unwrap();
        assert_eq!(report.auto_approved.len(), 1, "cap enforced");
        assert_eq!(report.left_pending, 1);
        assert_eq!(manager.generation(), generation + 1);
    }

    #[test]
    fn cycle_without_new_observations_is_a_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = manager_in(tmp.path());
        let first = run_evolution_cycle(&manager, &EvolutionPolicy::default()).unwrap();
        assert!(!first.acted());
        let second = run_evolution_cycle(&manager, &EvolutionPolicy::default()).unwrap();
        assert!(!second.acted());
    }

    #[tokio::test]
    async fn service_coalesces_bursts_into_one_cycle() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = manager_in(tmp.path());
        observe_three(&manager, "storm N");

        let options = EvolutionOptions {
            quiet_window: Duration::from_millis(150),
            timer: None,
            policy: EvolutionPolicy::default(),
        };
        let handle = start(manager.clone(), options);
        let mut reports = handle.subscribe_reports();

        // Wait for the startup-sweep report first.
        let startup = tokio::time::timeout(Duration::from_secs(2), reports.recv())
            .await
            .expect("startup report")
            .expect("channel open");
        assert!(startup.triggers.contains(&"startup"));
        assert_eq!(startup.proposals_written.len(), 1);

        // A burst of manual triggers coalesces into exactly one more
        // report (which is a no-op — the cluster is consumed).
        for i in 0..5 {
            handle.trigger(EvolutionTrigger::Manual {
                by: format!("t{i}"),
            });
        }
        let coalesced = tokio::time::timeout(Duration::from_secs(2), reports.recv())
            .await
            .expect("coalesced report")
            .expect("channel open");
        assert!(
            coalesced
                .triggers
                .iter()
                .filter(|t| **t == "manual")
                .count()
                >= 1,
            "{:?}",
            coalesced.triggers
        );
        assert!(!coalesced.acted());
        assert_eq!(handle.dropped_triggers(), 0);
    }

    #[tokio::test]
    async fn observation_recording_fires_the_trigger_path() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = manager_in(tmp.path());
        let options = EvolutionOptions {
            quiet_window: Duration::from_millis(50),
            timer: None,
            policy: EvolutionPolicy::default(),
        };
        let handle = start(manager.clone(), options.clone());
        let mut reports = handle.subscribe_reports();
        // Drain the startup report.
        let _ = tokio::time::timeout(Duration::from_secs(2), reports.recv()).await;

        // The manager must be attached to the service to forward
        // observation events.
        manager.attach_evolution(&handle);
        observe_three(&manager, "hooked N");
        let report = tokio::time::timeout(Duration::from_secs(2), reports.recv())
            .await
            .expect("event-driven report")
            .expect("channel open");
        assert!(
            report.triggers.contains(&"observation"),
            "{:?}",
            report.triggers
        );
        assert_eq!(report.proposals_written.len(), 1);
    }

    #[test]
    fn observation_store_roundtrip_under_manager() {
        // Guards the store path used by the cycle against accidental
        // regressions in manager wiring.
        let tmp = tempfile::tempdir().unwrap();
        let manager = manager_in(tmp.path());
        assert_eq!(manager.observations().list().len(), 0);
        observe_three(&manager, "x");
        assert_eq!(manager.observations().list().len(), 3);
    }
}
