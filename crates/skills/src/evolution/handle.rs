/// Cheap, cloneable handle for every skill mutation in the daemon.
#[derive(Clone)]
pub struct SkillControlHandle {
    tx: mpsc::Sender<SkillCommand>,
    dropped: Arc<AtomicU64>,
    reports: broadcast::Sender<Arc<EvolutionReport>>,
    /// Configuration snapshot for introspection surfaces (the TUI
    /// dashboard shows what the service was started with).
    policy: EvolutionPolicy,
    timer: Option<Duration>,
    monitor: Arc<CycleMonitor>,
}

impl SkillControlHandle {
    pub(super) fn new(
        tx: mpsc::Sender<SkillCommand>,
        dropped: Arc<AtomicU64>,
        reports: broadcast::Sender<Arc<EvolutionReport>>,
        policy: EvolutionPolicy,
        timer: Option<Duration>,
        monitor: Arc<CycleMonitor>,
    ) -> Self {
        Self {
            tx,
            dropped,
            reports,
            policy,
            timer,
            monitor,
        }
    }

    /// Fire a trigger. Returns false when the channel is full or the
    /// worker is gone (the event is dropped and counted — the next
    /// timer or observation will sweep again).
    pub fn trigger(&self, event: EvolutionTrigger) -> bool {
        match self.tx.try_send(SkillCommand::Evolution(event)) {
            Ok(()) => true,
            Err(_) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// Commands dropped so far because the channel was full or the
    /// worker had exited — an observability signal, not an error.
    /// Renamed from `dropped_triggers()` because the channel now
    /// carries every command kind, not just triggers.
    pub fn dropped_commands(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Live worker phase plus last-cycle telemetry — the dashboard's
    /// "is a distillation running and how did the last one go" read.
    pub fn cycle_status(&self) -> CycleStatus {
        self.monitor.snapshot(self.dropped.load(Ordering::Relaxed))
    }

    /// Subscribe to per-cycle reports (fire-and-forget broadcast;
    /// lagging receivers simply miss reports).
    pub fn subscribe_reports(&self) -> broadcast::Receiver<Arc<EvolutionReport>> {
        self.reports.subscribe()
    }

    /// The raw command sender, for the manager's observation
    /// forwarder. Internal-use: callers should go through
    /// [`SkillControlHandle::trigger`].
    pub(crate) fn sender_for_manager(&self) -> mpsc::Sender<SkillCommand> {
        self.tx.clone()
    }

    /// The policy this service runs with (an auto-approve cap of 0
    /// alongside `auto_approve: true` would approve nothing, so the
    /// pair is kept together for display).
    pub fn policy(&self) -> &EvolutionPolicy {
        &self.policy
    }

    /// The periodic sweep cadence, when the timer trigger is armed.
    pub fn timer(&self) -> Option<Duration> {
        self.timer
    }

    /// Approve a pending proposal now. Returns `WorkerGone` if the
    /// channel closed before the worker replied.
    pub async fn call_approve(&self, name: String) -> SkillCommandOutcome<ApproveOutcome> {
        let (tx, rx) = oneshot::channel();
        if self
            .tx
            .send(SkillCommand::ApproveProposal { name, reply: tx })
            .await
            .is_err()
        {
            return SkillCommandOutcome::WorkerGone;
        }
        match rx.await {
            Ok(outcome) => outcome,
            Err(_) => SkillCommandOutcome::WorkerGone,
        }
    }

    /// Reject a pending proposal now.
    pub async fn call_reject(&self, name: String) -> SkillCommandOutcome<Proposal> {
        let (tx, rx) = oneshot::channel();
        if self
            .tx
            .send(SkillCommand::RejectProposal { name, reply: tx })
            .await
            .is_err()
        {
            return SkillCommandOutcome::WorkerGone;
        }
        match rx.await {
            Ok(outcome) => outcome,
            Err(_) => SkillCommandOutcome::WorkerGone,
        }
    }

    /// Run one evolution cycle now with the supplied policy (no
    /// coalescing delay). The cycle report is also broadcast on
    /// [`SkillControlHandle::subscribe_reports`] so dashboard
    /// listeners pick it up.
    pub async fn call_run_cycle(
        &self,
        policy: EvolutionPolicy,
    ) -> SkillCommandOutcome<EvolutionReport> {
        let (tx, rx) = oneshot::channel();
        if self
            .tx
            .send(SkillCommand::RunCycle { policy, reply: tx })
            .await
            .is_err()
        {
            return SkillCommandOutcome::WorkerGone;
        }
        match rx.await {
            Ok(outcome) => outcome,
            Err(_) => SkillCommandOutcome::WorkerGone,
        }
    }
}

/// Start the skill control service for one manager: spawns the
/// worker task, fires the startup sweep, and returns the control
/// handle.
///
/// The worker coalesces trigger bursts within
/// `options.quiet_window` into a single cycle, processes inline
/// approve/reject commands as they arrive, and runs explicit
/// `RunCycle` commands on demand. When `options.timer` is set, the
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::{broadcast, mpsc, oneshot};

use crate::proposals::{ApproveOutcome, Proposal};

use super::command::{SkillCommand, SkillCommandOutcome};
use super::model::{EvolutionPolicy, EvolutionReport, EvolutionTrigger};
use super::monitor::{CycleMonitor, CycleStatus};
