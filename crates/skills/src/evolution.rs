//! The skill control loop, the **only** mutation path in the daemon:
//!
//! - **Workflow** ([`run_evolution_cycle`]): the idempotent "what
//!   happens" — snapshot observations, distill proposals (create or
//!   update), apply the approval policy. Pure, synchronous; called
//!   only by the worker on this crate. Stays `pub(crate)` so the
//!   auto-approve loop inside the cycle can promote without routing
//!   through the channel (the worker is already on the executor —
//!   a channel round-trip would deadlock).
//! - **Commands** ([`SkillControlHandle`]): the unified control
//!   surface. Every external mutation — evolution triggers,
//!   proposal approve/reject, an explicit `RunCycle` — travels
//!   through one `mpsc::Sender<SkillCommand>` into a single worker
//!   task spawned by [`start`]. Trigger variants coalesce within a
//!   quiet window; inline mutations (approve/reject) and explicit
//!   cycles process immediately with a oneshot reply.
//!
//! ## Why one channel
//!
//! Before this refactor the gateway HTTP handlers and the agent
//! `skill_evolve` tool bypassed the channel with `spawn_blocking`,
//! and the CLI exposed a `distill` / `approve` / `reject` surface
//! that called the manager directly. Every path except the worker's
//! own timer/startup/observation feed had its own control code path.
//! Routing them all through one channel gives a single funnel for
//! mutation ordering, dropped/timeout observability, and the same
//! `try_send`-never-blocks contract the observation forwarder
//! already relies on.
//!
//! ## Reply semantics
//!
//! Mutations that need a result (approve/reject/run-cycle) carry an
//! `oneshot::Sender<SkillCommandOutcome<T>>`. The worker sends
//! exactly one outcome per command: `Ok`, `Err(SkillError)`, or
//! `WorkerGone` (worker exited before replying — distinct fault
//! class that maps to HTTP 503 at the boundary). Triggers are
//! fire-and-forget: a full channel drops the event and bumps the
//! dropped counter, never blocking the caller.
//!
//! ## Safety rails on automation
//!
//! [`EvolutionPolicy`] gates auto-approval separately from
//! distillation: by default the cycle only *writes pending
//! proposals* (human review stays in the loop), and even with
//! `auto_approve` enabled it is capped per cycle, only ever touches
//! loop-created skills (human-owned names are refused at distill
//! time), and every promotion still passes the full validation
//! contract. External approve commands (TUI clicks) are deliberately
//! unbounded — the cap exists to bound *unattended automation*, not
//! deliberate human action.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::{broadcast, mpsc, oneshot};

use crate::error::SkillError;
use crate::manager::SkillManager;
use crate::proposals::{ApproveOutcome, Proposal, ProposalStatus};

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
#[derive(Debug, Clone, Default, serde::Serialize)]
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
    /// Pending proposals that are agent-authored — review-gated even
    /// under auto-approve.
    pub human_review_only: usize,
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

// ────────────────────────── cycle monitor ──────────────────────────

/// Live worker phase, updated at every transition so the status
/// endpoint can show what the loop is doing *right now*.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case", tag = "phase")]
pub enum CyclePhase {
    /// No batch is being collected and no cycle is running.
    #[default]
    Idle,
    /// The quiet window is draining; `queued` triggers have
    /// accumulated so far.
    Coalescing { queued: usize },
    /// A cycle (distill + policy) is executing for these trigger
    /// labels.
    Distilling { triggers: Vec<String> },
}

impl CyclePhase {
    /// Stable lowercase label for flat DTOs and rendering.
    pub fn label(&self) -> &'static str {
        match self {
            CyclePhase::Idle => "idle",
            CyclePhase::Coalescing { .. } => "coalescing",
            CyclePhase::Distilling { .. } => "distilling",
        }
    }
}

/// One point-in-time read of the worker's progress — what the
/// dashboard polls while a cycle runs and what it shows between
/// cycles.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct CycleStatus {
    pub phase: CyclePhase,
    /// How long the current phase has held (ms).
    pub phase_elapsed_ms: u64,
    /// Cycles executed since service start (successes and failures).
    pub cycles_completed: u64,
    pub last_cycle_at: Option<i64>,
    pub last_cycle_duration_ms: Option<u64>,
    /// Trigger labels of the most recent cycle.
    pub last_triggers: Vec<String>,
    /// Error text when the most recent cycle failed; cleared by the
    /// next success.
    pub last_error: Option<String>,
    /// Commands dropped because the channel was full or the worker
    /// was gone — an observability signal, not an error.
    pub dropped_commands: u64,
    /// Cycles short-circuited because the observation pool was empty.
    /// Distinct from `cycles_completed`: the worker never ran a cycle
    /// body, so it did not consume cluster cycles' worth of work.
    pub cycle_skipped_empty: u64,
    /// Stable label for why the most recent cycle (or short-circuit)
    /// was skipped; `None` when the last cycle body actually ran.
    pub last_skipped_reason: Option<String>,
}

/// Mutable core behind the monitor lock. Phase updates are rare (a
/// handful per cycle) and reads poll at ~1 Hz, so a plain mutex with
/// short critical sections is plenty.
#[derive(Debug)]
struct MonitorCore {
    phase: CyclePhase,
    phase_since: std::time::Instant,
    cycles_completed: u64,
    last_cycle_at: Option<i64>,
    last_cycle_duration_ms: Option<u64>,
    last_triggers: Vec<String>,
    last_error: Option<String>,
    cycle_skipped_empty: u64,
    last_skipped_reason: Option<String>,
}

impl Default for MonitorCore {
    fn default() -> Self {
        Self {
            phase: CyclePhase::Idle,
            phase_since: std::time::Instant::now(),
            cycles_completed: 0,
            last_cycle_at: None,
            last_cycle_duration_ms: None,
            last_triggers: Vec::new(),
            last_error: None,
            cycle_skipped_empty: 0,
            last_skipped_reason: None,
        }
    }
}

/// Shared between the worker (writer) and the control handle
/// (reader). Runtime-only observation state — resets on restart by
/// design; the durable record of what cycles produced lives in the
/// proposals area.
#[derive(Debug, Default)]
pub struct CycleMonitor(std::sync::Mutex<MonitorCore>);

impl CycleMonitor {
    /// The quiet window opened (or grew): `queued` triggers so far.
    fn coalescing(&self, queued: usize) {
        let mut core = self.lock();
        if !matches!(core.phase, CyclePhase::Coalescing { .. }) {
            core.phase_since = std::time::Instant::now();
        }
        core.phase = CyclePhase::Coalescing { queued };
    }

    /// A cycle is starting for these trigger labels.
    fn distilling(&self, triggers: &[&'static str]) {
        let mut core = self.lock();
        core.phase = CyclePhase::Distilling {
            triggers: triggers.iter().map(|t| t.to_string()).collect(),
        };
        core.phase_since = std::time::Instant::now();
    }

    /// A cycle returned — success or failure — and the worker is idle
    /// again.
    fn cycle_finished(
        &self,
        triggers: &[&'static str],
        duration: Duration,
        error: Option<String>,
    ) {
        let mut core = self.lock();
        core.phase = CyclePhase::Idle;
        core.phase_since = std::time::Instant::now();
        core.cycles_completed += 1;
        core.last_cycle_at = Some(unix_now());
        core.last_cycle_duration_ms = Some(duration.as_millis() as u64);
        core.last_triggers = triggers.iter().map(|t| t.to_string()).collect();
        core.last_error = error;
        // A cycle body actually ran, so the prior short-circuit
        // reason (if any) no longer applies.
        core.last_skipped_reason = None;
    }

    /// The worker received triggers but found no observations to
    /// distill. It skips the cycle body, leaves `cycles_completed`
    /// alone (no work was done), and stamps a stable skip reason so
    /// the dashboard can surface it.
    fn cycle_skipped_empty(&self, triggers: &[&'static str]) {
        let mut core = self.lock();
        core.phase = CyclePhase::Idle;
        core.phase_since = std::time::Instant::now();
        core.cycle_skipped_empty += 1;
        core.last_cycle_at = Some(unix_now());
        core.last_cycle_duration_ms = Some(0);
        core.last_triggers = triggers.iter().map(|t| t.to_string()).collect();
        core.last_error = None;
        core.last_skipped_reason = Some("empty_pool".to_string());
    }

    fn snapshot(&self, dropped_commands: u64) -> CycleStatus {
        let core = self.lock();
        CycleStatus {
            phase_elapsed_ms: core.phase_since.elapsed().as_millis() as u64,
            phase: core.phase.clone(),
            cycles_completed: core.cycles_completed,
            last_cycle_at: core.last_cycle_at,
            last_cycle_duration_ms: core.last_cycle_duration_ms,
            last_triggers: core.last_triggers.clone(),
            last_error: core.last_error.clone(),
            dropped_commands,
            cycle_skipped_empty: core.cycle_skipped_empty,
            last_skipped_reason: core.last_skipped_reason.clone(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MonitorCore> {
        // Poisoning only happens if a worker panicked mid-update; the
        // monitor is observation state, so recovering with a fresh
        // core beats propagating the panic into every poll.
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ────────────────────────── workflow ──────────────────────────

/// Run one evolution cycle: distill, then apply the approval policy.
///
/// Idempotent — running it twice without new observations produces a
/// no-op report (consumed clusters are skipped at distill time).
/// Internal-use only: the worker is the only caller. `pub(crate)`
/// because routing an external request through this function would
/// be the bypass the unified-channel design exists to prevent.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn run_evolution_cycle(
    manager: &SkillManager,
    policy: &EvolutionPolicy,
) -> Result<EvolutionReport, SkillError> {
    run_evolution_cycle_labeled(manager, policy, Vec::new())
}

/// [`run_evolution_cycle`] with the trigger labels that caused it,
/// used by the worker so reports say why they ran.
pub(crate) fn run_evolution_cycle_labeled(
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

    let all_pending = manager.proposals().list();
    let pending: Vec<String> = all_pending
        .iter()
        .filter(|p| p.status == ProposalStatus::Pending)
        .map(|p| p.name.clone())
        .collect();
    // Agent-authored proposals are human-review-only: the model may
    // draft, never promote its own draft.
    let agent_authored_pending = all_pending
        .iter()
        .filter(|p| p.status == ProposalStatus::Pending && p.authored_by == "agent")
        .count();

    if policy.auto_approve {
        // Filter before the cap so agent entries consume no approval
        // slots.
        let eligible: Vec<String> = all_pending
            .iter()
            .filter(|p| p.status == ProposalStatus::Pending && p.authored_by != "agent")
            .map(|p| p.name.clone())
            .collect();
        for name in eligible.iter().take(policy.max_approvals_per_cycle) {
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
    report.human_review_only = agent_authored_pending;
    // Persist the fitness signal as part of every cycle — the
    // natural checkpoint cadence, and the restart boundary for
    // usage data.
    if let Err(e) = manager.persist_usage() {
        tracing::warn!(error = %e, "cannot persist usage table");
    }
    Ok(report)
}

// ────────────────────────── trigger side ──────────────────────────

/// The reply payload a worker sends back to a request that needs one.
///
/// Three-variant sum type (not `Result<T, SkillError>`): `WorkerGone`
/// is a distinct fault class — the worker task exited before
/// replying, which surfaces as HTTP 503 at the gateway boundary.
/// Folding it into `Result` would force every handler to sniff error
/// variants to pick a status code.
#[derive(Debug)]
pub enum SkillCommandOutcome<T> {
    /// Operation succeeded with the produced value.
    Ok(T),
    /// Operation failed with a domain error (bad proposal name,
    /// validation refused, etc.). Maps to 400/404 at the HTTP layer.
    Err(SkillError),
    /// Worker exited before this command was processed. Maps to 503.
    WorkerGone,
}

impl<T> SkillCommandOutcome<T> {
    pub fn is_ok(&self) -> bool {
        matches!(self, SkillCommandOutcome::Ok(_))
    }
}

/// One unit of work the worker processes. The worker owns the
/// `mpsc::Receiver`; every external mutation travels through this
/// enum.
///
/// Trigger variants are fire-and-forget — the worker never replies.
/// Mutation variants carry an `oneshot::Sender`; the worker sends
/// exactly one `SkillCommandOutcome<T>` back before dropping the
/// command (or `WorkerGone` if it shut down first).
pub enum SkillCommand {
    /// Coalesced with other `Evolution` variants in the quiet window
    /// before one cycle. The four sub-variants come straight from
    /// [`EvolutionTrigger`].
    Evolution(EvolutionTrigger),
    /// Process a pending proposal approval inline (between batch
    /// collection and cycle run). The worker replies with
    /// `ApproveOutcome` on success.
    ApproveProposal {
        name: String,
        reply: oneshot::Sender<SkillCommandOutcome<ApproveOutcome>>,
    },
    /// Process a pending proposal rejection inline. Replies with
    /// the updated [`Proposal`].
    RejectProposal {
        name: String,
        reply: oneshot::Sender<SkillCommandOutcome<Proposal>>,
    },
    /// Run a full cycle now (no coalescing) with the supplied
    /// policy. Used by the gateway `POST /skills/evolution/trigger`
    /// handler and the agent `skill_evolve` tool. Replies with the
    /// produced [`EvolutionReport`].
    RunCycle {
        policy: EvolutionPolicy,
        reply: oneshot::Sender<SkillCommandOutcome<EvolutionReport>>,
    },
}

/// Cheap, cloneable handle for every skill mutation in the daemon.
/// `try_send` semantics for the trigger path: a full channel drops
/// the event and bumps the dropped counter, never blocking the
/// caller (the same "subscriber cannot break the mutation path"
/// contract the manager's broadcast carries). The `call_*` methods
/// await a oneshot reply and are intended for request/response
/// callers (HTTP handlers, the agent tool).
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
        self.monitor
            .snapshot(self.dropped.load(Ordering::Relaxed))
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

// ────────────────────────── service ──────────────────────────

/// Start the skill control service for one manager: spawns the
/// worker task, fires the startup sweep, and returns the control
/// handle.
///
/// The worker coalesces trigger bursts within
/// `options.quiet_window` into a single cycle, processes inline
/// approve/reject commands as they arrive, and runs explicit
/// `RunCycle` commands on demand. When `options.timer` is set, the
/// worker also fires a periodic `Timer` trigger.
pub fn start(manager: Arc<SkillManager>, options: EvolutionOptions) -> SkillControlHandle {
    // Capacity bumped from 64 → 128: inline approve/reject commands
    // now share the channel, and a burst of TUI clicks during a slow
    // cycle must not push approvals out the back door.
    let (tx, rx) = mpsc::channel::<SkillCommand>(128);
    let (report_tx, _) = broadcast::channel::<Arc<EvolutionReport>>(16);
    let dropped = Arc::new(AtomicU64::new(0));
    let monitor = Arc::new(CycleMonitor::default());

    // Snapshot the introspection copy before the worker owns the rest.
    let policy = options.policy.clone();
    let timer = options.timer;

    let worker = Worker {
        manager,
        options,
        reports: report_tx.clone(),
        monitor: Arc::clone(&monitor),
    };
    tokio::spawn(worker.run(rx));

    let handle = SkillControlHandle {
        tx,
        dropped,
        reports: report_tx,
        policy,
        timer,
        monitor,
    };
    handle.trigger(EvolutionTrigger::Startup);
    handle
}

struct Worker {
    manager: Arc<SkillManager>,
    options: EvolutionOptions,
    reports: broadcast::Sender<Arc<EvolutionReport>>,
    monitor: Arc<CycleMonitor>,
}

impl Worker {
    async fn run(self, mut rx: mpsc::Receiver<SkillCommand>) {
        // Consume the interval's immediate first tick — the explicit
        // Startup trigger already performs the boot sweep.
        let mut timer = self.options.timer.map(tokio::time::interval);
        if let Some(interval) = timer.as_mut() {
            interval.tick().await;
        }
        loop {
            // First command of a new iteration: either an inbound
            // command or a timer-driven trigger.
            let first = match &mut timer {
                Some(interval) => {
                    tokio::select! {
                        cmd = rx.recv() => match cmd {
                            Some(cmd) => cmd,
                            None => break,
                        },
                        _ = interval.tick() => SkillCommand::Evolution(EvolutionTrigger::Timer),
                    }
                }
                None => match rx.recv().await {
                    Some(cmd) => cmd,
                    None => break,
                },
            };

            match first {
                SkillCommand::Evolution(trigger) => {
                    // Quiet-window drain: collect triggers into the
                    // batch; process inline mutations as they arrive
                    // (HTTP callers must not wait behind a possibly
                    // slow cycle); remember the first `RunCycle` we
                    // see so it can run *after* the inline replies
                    // but still skip coalescing.
                    let mut triggers = vec![trigger];
                    self.monitor.coalescing(triggers.len());
                    let mut inline_approves: Vec<(
                        String,
                        oneshot::Sender<SkillCommandOutcome<ApproveOutcome>>,
                    )> = Vec::new();
                    let mut inline_rejects: Vec<(
                        String,
                        oneshot::Sender<SkillCommandOutcome<Proposal>>,
                    )> = Vec::new();
                    let mut run_cycle: Option<(
                        EvolutionPolicy,
                        oneshot::Sender<SkillCommandOutcome<EvolutionReport>>,
                    )> = None;
                    let deadline = tokio::time::Instant::now() + self.options.quiet_window;
                    loop {
                        let remain =
                            deadline.saturating_duration_since(tokio::time::Instant::now());
                        if remain.is_zero() {
                            break;
                        }
                        match tokio::time::timeout(remain, rx.recv()).await {
                            Ok(Some(SkillCommand::Evolution(t))) => {
                                triggers.push(t);
                                self.monitor.coalescing(triggers.len());
                            }
                            Ok(Some(SkillCommand::ApproveProposal { name, reply })) => {
                                inline_approves.push((name, reply));
                            }
                            Ok(Some(SkillCommand::RejectProposal { name, reply })) => {
                                inline_rejects.push((name, reply));
                            }
                            Ok(Some(cmd @ SkillCommand::RunCycle { .. })) => {
                                // First explicit cycle wins; subsequent
                                // RunCycle commands wait for the next
                                // iteration rather than stacking here.
                                if run_cycle.is_none()
                                    && let SkillCommand::RunCycle { policy, reply } = cmd
                                {
                                    run_cycle = Some((policy, reply));
                                }
                            }
                            Ok(None) => break,
                            Err(_) => break,
                        }
                    }

                    // 1) Inline approve/reject FIRST — these are user
                    //    clicks and must not wait behind a cycle.
                    for (name, reply) in inline_approves {
                        let outcome = match self.manager.approve_proposal(&name) {
                            Ok(o) => SkillCommandOutcome::Ok(o),
                            Err(e) => SkillCommandOutcome::Err(e),
                        };
                        let _ = reply.send(outcome);
                    }
                    for (name, reply) in inline_rejects {
                        let outcome = match self.manager.reject_proposal(&name) {
                            Ok(p) => SkillCommandOutcome::Ok(p),
                            Err(e) => SkillCommandOutcome::Err(e),
                        };
                        let _ = reply.send(outcome);
                    }

                    // 2) Coalesced cycle if any triggers accumulated.
                    if !triggers.is_empty() {
                        let labels: Vec<&'static str> =
                            triggers.iter().map(EvolutionTrigger::label).collect();
                        let summary = labels.join(",");
                        // Short-circuit when the observation pool is
                        // empty: there's nothing to cluster, no cycle
                        // body to run, no report to broadcast. Explicit
                        // RunCycle commands still go through the body
                        // below — the operator asked for it.
                        if run_cycle.is_none()
                            && self.manager.observations().list().is_empty()
                        {
                            tracing::debug!(
                                triggers = %summary,
                                "evolution cycle skipped: empty observation pool"
                            );
                            self.monitor.cycle_skipped_empty(&labels);
                            continue;
                        }
                        self.monitor.distilling(&labels);
                        let started = std::time::Instant::now();
                        let outcome = run_evolution_cycle_labeled(
                            &self.manager,
                            &self.options.policy,
                            labels.clone(),
                        );
                        self.monitor.cycle_finished(
                            &labels,
                            started.elapsed(),
                            match &outcome {
                                Ok(_) => None,
                                Err(e) => Some(e.to_string()),
                            },
                        );
                        match outcome {
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

                    // 3) Explicit RunCycle (skip coalescing).
                    if let Some((policy, reply)) = run_cycle {
                        let labels = vec!["manual"];
                        self.monitor.distilling(&labels);
                        let started = std::time::Instant::now();
                        let outcome =
                            run_evolution_cycle_labeled(&self.manager, &policy, labels.clone());
                        self.monitor.cycle_finished(
                            &labels,
                            started.elapsed(),
                            match &outcome {
                                Ok(_) => None,
                                Err(e) => Some(e.to_string()),
                            },
                        );
                        let outcome = match outcome {
                            Ok(report) => {
                                let _ = self.reports.send(Arc::new(report.clone()));
                                SkillCommandOutcome::Ok(report)
                            }
                            Err(e) => SkillCommandOutcome::Err(e),
                        };
                        let _ = reply.send(outcome);
                    }
                }
                SkillCommand::ApproveProposal { name, reply } => {
                    let outcome = match self.manager.approve_proposal(&name) {
                        Ok(o) => SkillCommandOutcome::Ok(o),
                        Err(e) => SkillCommandOutcome::Err(e),
                    };
                    let _ = reply.send(outcome);
                }
                SkillCommand::RejectProposal { name, reply } => {
                    let outcome = match self.manager.reject_proposal(&name) {
                        Ok(p) => SkillCommandOutcome::Ok(p),
                        Err(e) => SkillCommandOutcome::Err(e),
                    };
                    let _ = reply.send(outcome);
                }
                SkillCommand::RunCycle { policy, reply } => {
                    let labels = vec!["manual"];
                    self.monitor.distilling(&labels);
                    let started = std::time::Instant::now();
                    let outcome =
                        run_evolution_cycle_labeled(&self.manager, &policy, labels.clone());
                    self.monitor.cycle_finished(
                        &labels,
                        started.elapsed(),
                        match &outcome {
                            Ok(_) => None,
                            Err(e) => Some(e.to_string()),
                        },
                    );
                    let outcome = match outcome {
                        Ok(report) => {
                            let _ = self.reports.send(Arc::new(report.clone()));
                            SkillCommandOutcome::Ok(report)
                        }
                        Err(e) => SkillCommandOutcome::Err(e),
                    };
                    let _ = reply.send(outcome);
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
        assert_eq!(handle.dropped_commands(), 0);
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

#[cfg(test)]
mod timer_tests {
    use super::*;
    use crate::observation::{
        ObservationInput, ObservationKind, ObservationSource,
    };

    /// The timer trigger path: with no events arriving, the periodic
    /// sweep alone produces cycles.
    #[tokio::test]
    async fn timer_sweeps_without_events() {
        // Seed the observation pool so the worker actually runs the
        // cycle body; the empty-pool short-circuit introduced later
        // would otherwise elide the broadcast entirely.
        let tmp = tempfile::tempdir().unwrap();
        let manager = Arc::new(SkillManager::new(tmp.path().join("state")));
        for body in ["fix a", "fix b", "fix c"] {
            manager
                .record_observation(ObservationInput {
                    kind: ObservationKind::Failure,
                    source: ObservationSource::Agent,
                    summary: "fix for boom N".into(),
                    body: body.into(),
                    node_kind: Some("sql".into()),
                    error: Some("boom at step N".into()),
                })
                .unwrap();
        }
        let options = EvolutionOptions {
            quiet_window: Duration::from_millis(20),
            timer: Some(Duration::from_millis(120)),
            policy: EvolutionPolicy::default(),
        };
        let handle = start(manager, options);
        let mut reports = handle.subscribe_reports();
        // Startup report first…
        let _ = tokio::time::timeout(Duration::from_secs(2), reports.recv()).await;
        // …then at least one timer report within a generous window.
        let timer_report = tokio::time::timeout(Duration::from_secs(3), reports.recv())
            .await
            .expect("timer report")
            .expect("channel open");
        assert!(
            timer_report.triggers.contains(&"timer"),
            "{:?}",
            timer_report.triggers
        );
    }
}

#[cfg(test)]
mod authoring_tests {
    use super::*;
    use crate::observation::{ObservationInput, ObservationKind, ObservationSource};

    #[test]
    fn auto_approve_never_promotes_agent_authored_proposals() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = Arc::new(SkillManager::new(tmp.path().join("state")));
        let id = manager
            .record_observation(ObservationInput {
                kind: ObservationKind::Failure,
                source: ObservationSource::Agent,
                summary: "evidence".into(),
                body: "fix".into(),
                node_kind: Some("sql".into()),
                error: Some("boom".into()),
            })
            .unwrap()
            .id;
        manager
            .propose_skill(
                "agent-made",
                "Agent-drafted skill.",
                &[],
                "# Body\n\nSteps.\n",
                std::slice::from_ref(&id),
                "pattern",
            )
            .unwrap();

        let policy = EvolutionPolicy {
            auto_approve: true,
            max_approvals_per_cycle: 5,
        };
        let generation = manager.generation();
        let report = run_evolution_cycle(&manager, &policy).unwrap();
        // Refused: still pending, human-review-only, generation unmoved.
        assert!(report.auto_approved.is_empty());
        assert_eq!(report.left_pending, 1);
        assert_eq!(report.human_review_only, 1);
        assert_eq!(manager.generation(), generation);
        assert_eq!(
            manager.proposals().find("agent-made").unwrap().status,
            crate::proposals::ProposalStatus::Pending
        );
    }
}

/// Coverage for the unified channel: every skill mutation
/// (evolution triggers, proposal approve/reject, explicit cycle)
/// travels through one `mpsc::Sender<SkillCommand>` into the worker
/// task. These tests pin the funnel invariants the design relies on.
#[cfg(test)]
mod channel_funnel {
    use super::*;
    use crate::observation::ObservationInput;
    use crate::proposals::ProposalStatus;

    fn start_service(state_dir: &std::path::Path) -> (Arc<SkillManager>, SkillControlHandle) {
        let manager = Arc::new(SkillManager::new(state_dir));
        let handle = start(
            manager.clone(),
            EvolutionOptions {
                quiet_window: Duration::from_millis(20),
                timer: None,
                policy: EvolutionPolicy::default(),
            },
        );
        (manager, handle)
    }

    /// Three anchored failures on `(node_kind=sql, error=boom)` plus
    /// a `RunCycle` produces one pending proposal. Returns the
    /// proposal name (deterministic slug from `distill`).
    ///
    /// Caller is responsible for draining the Startup sweep
    /// broadcast BEFORE calling this — the Startup trigger's
    /// quiet-window drain would otherwise capture the RunCycle
    /// and create the proposal out from under us.
    async fn seed_pending_proposal(manager: &SkillManager, handle: &SkillControlHandle) -> String {
        for body in ["fix a", "fix b", "fix c"] {
            manager
                .record_observation(ObservationInput {
                    kind: crate::observation::ObservationKind::Failure,
                    source: crate::observation::ObservationSource::Agent,
                    summary: "fix for boom".into(),
                    body: body.into(),
                    node_kind: Some("sql".into()),
                    error: Some("boom".into()),
                })
                .unwrap();
        }
        let outcome = handle.call_run_cycle(EvolutionPolicy::default()).await;
        let report = match outcome {
            SkillCommandOutcome::Ok(r) => r,
            other => panic!("seed cycle failed: {other:?}"),
        };
        assert_eq!(report.proposals_written.len(), 1);
        report.proposals_written[0].clone()
    }

    /// Happy path: `call_approve` returns `SkillCommandOutcome::Ok`
    /// and bumps the manager's generation (the channel worker
    /// called `manager.approve_proposal` directly).
    #[tokio::test]
    async fn channel_funnel_call_approve_returns_outcome() {
        let tmp = tempfile::tempdir().unwrap();
        let (manager, handle) = start_service(&tmp.path().join("state"));
        let mut reports = handle.subscribe_reports();
        // Drain the Startup sweep broadcast BEFORE recording
        // observations; otherwise the Startup trigger's quiet-window
        // drain captures our RunCycle and creates the proposal
        // before we can control the cycle order.
        let _ = tokio::time::timeout(Duration::from_secs(2), reports.recv()).await;
        let name = seed_pending_proposal(&manager, &handle).await;
        let _ = tokio::time::timeout(Duration::from_secs(2), reports.recv()).await;
        let generation_before = manager.generation();
        let outcome = handle.call_approve(name.clone()).await;
        match outcome {
            SkillCommandOutcome::Ok(o) => assert_eq!(o.name, name),
            other => panic!("expected Ok, got {other:?}"),
        }
        assert_eq!(
            manager.generation(),
            generation_before + 1,
            "approve bumps the generation"
        );
    }

    /// Happy path: `call_reject` returns the updated proposal with
    /// `Rejected` status.
    #[tokio::test]
    async fn channel_funnel_call_reject_returns_outcome() {
        let tmp = tempfile::tempdir().unwrap();
        let (manager, handle) = start_service(&tmp.path().join("state"));
        let mut reports = handle.subscribe_reports();
        let _ = tokio::time::timeout(Duration::from_secs(2), reports.recv()).await;
        let name = seed_pending_proposal(&manager, &handle).await;
        let _ = tokio::time::timeout(Duration::from_secs(2), reports.recv()).await;

        let outcome = handle.call_reject(name.clone()).await;
        match outcome {
            SkillCommandOutcome::Ok(proposal) => {
                assert_eq!(proposal.name, name);
                assert_eq!(proposal.status, ProposalStatus::Rejected);
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    /// `call_run_cycle` runs a cycle and replies with the report;
    /// the same report is also broadcast on `subscribe_reports` so
    /// the dashboard live view picks it up.
    #[tokio::test]
    async fn channel_funnel_call_run_cycle_returns_report() {
        let tmp = tempfile::tempdir().unwrap();
        let (manager, handle) = start_service(&tmp.path().join("state"));
        let mut reports = handle.subscribe_reports();
        // Drain startup report first.
        let _ = tokio::time::timeout(Duration::from_secs(2), reports.recv()).await;
        let name = seed_pending_proposal(&manager, &handle).await;
        // Drain the seed cycle broadcast.
        let _ = tokio::time::timeout(Duration::from_secs(2), reports.recv()).await;
        assert_eq!(name, "sql-boom");
    }

    /// After the channel closes (every handle dropped, worker
    /// exited), the manager's `record_observation` forwarder still
    /// succeeds — the dropped-triggers contract means the
    /// observation is recorded but no evolution trigger fires.
    /// This pins the no-block guarantee on the observation hot
    /// path.
    #[tokio::test]
    async fn channel_funnel_observation_forwarder_does_not_block_after_drop() {
        let tmp = tempfile::tempdir().unwrap();
        let (manager, handle) = start_service(&tmp.path().join("state"));
        drop(handle);
        // Let the worker observe the closed channel and exit.
        tokio::time::sleep(Duration::from_millis(30)).await;
        let probe = manager.record_observation(ObservationInput {
            kind: crate::observation::ObservationKind::Failure,
            source: crate::observation::ObservationSource::Agent,
            summary: "probe".into(),
            body: "x".into(),
            node_kind: Some("sql".into()),
            error: Some("boom".into()),
        });
        assert!(
            probe.is_ok(),
            "observation must succeed even with worker gone"
        );
    }

    /// Dropped-command observability: the counter is reachable
    /// through the handle and starts at zero; a successful trigger
    /// does not bump it (the channel has capacity).
    #[tokio::test]
    async fn channel_funnel_dropped_command_counter_starts_at_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let (_manager, handle) = start_service(&tmp.path().join("state"));
        let baseline = handle.dropped_commands();
        handle.trigger(EvolutionTrigger::Manual { by: "t0".into() });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(handle.dropped_commands(), baseline);
    }

    /// The critical UX invariant: a TUI approve click must NOT
    /// wait behind a possibly-slow coalesced evolution cycle. The
    /// worker drains the quiet window first; inline approve/reject
    /// commands are processed BEFORE the cycle runs, so the HTTP
    /// caller never sees the cycle's file I/O.
    ///
    /// We exercise this by setting a 300 ms quiet window, firing
    /// a manual trigger, then queueing an ApproveProposal
    /// mid-drain. With the design, both the approve reply and the
    /// cycle broadcast must arrive within a generous window; the
    /// cycle broadcast's `triggers` field confirms the manual
    /// trigger was coalesced (proves the cycle ran the manual
    /// trigger's batch, not just a no-op).
    #[tokio::test]
    async fn channel_funnel_inline_approve_does_not_block_cycle() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = Arc::new(SkillManager::new(tmp.path().join("state")));
        let handle = start(
            manager.clone(),
            EvolutionOptions {
                quiet_window: Duration::from_millis(300),
                timer: None,
                policy: EvolutionPolicy::default(),
            },
        );
        let mut reports = handle.subscribe_reports();
        let _ = tokio::time::timeout(Duration::from_secs(2), reports.recv()).await;
        let name = seed_pending_proposal(&manager, &handle).await;
        let _ = tokio::time::timeout(Duration::from_secs(2), reports.recv()).await;

        // Open a fresh quiet-window drain with a manual trigger.
        handle.trigger(EvolutionTrigger::Manual { by: "t0".into() });
        tokio::time::sleep(Duration::from_millis(20)).await;
        let outcome = handle.call_approve(name.clone()).await;
        match outcome {
            SkillCommandOutcome::Ok(_) => {}
            other => panic!("expected Ok, got {other:?}"),
        }
        // The cycle broadcast should arrive within the drain
        // window plus a cycle's worth of file I/O. Generous
        // bound keeps the test stable on busy CI.
        let cycle_broadcast = tokio::time::timeout(Duration::from_secs(2), reports.recv())
            .await
            .expect("cycle broadcast")
            .expect("channel open");
        // The coalesced cycle should have the manual trigger in
        // its triggers list (the worker coalesced the manual
        // trigger into the next cycle, not a fresh cycle).
        assert!(
            cycle_broadcast.triggers.iter().any(|t| *t == "manual"),
            "cycle report missing manual trigger: {:?}",
            cycle_broadcast.triggers
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn monitor_reflects_coalesce_distill_idle_phases() {
        // The monitor must transition idle → coalescing → distilling →
        // idle around a run, and stamp a meaningful last-cycle row.
        let tmp = tempfile::tempdir().unwrap();
        let manager = Arc::new(SkillManager::new(tmp.path().join("state")));
        // Seed one observation so the startup sweep has something to
        // distill; otherwise the worker short-circuits the cycle
        // body and `cycles_completed` stays at 0.
        manager
            .record_observation(crate::observation::ObservationInput {
                kind: crate::observation::ObservationKind::Failure,
                source: crate::observation::ObservationSource::Agent,
                summary: "fix for boom N".into(),
                body: "fix a".into(),
                node_kind: Some("sql".into()),
                error: Some("boom at step N".into()),
            })
            .unwrap();
        let handle = start(
            manager.clone(),
            EvolutionOptions {
                policy: EvolutionPolicy::default(),
                quiet_window: Duration::from_millis(80),
                timer: None,
            },
        );

        // Startup sweep finishes before we sample, so the monitor
        // should report at least one cycle right out of the gate.
        tokio::time::sleep(Duration::from_millis(120)).await;
        let post_startup = handle.cycle_status();
        assert_eq!(post_startup.phase, CyclePhase::Idle);
        assert!(post_startup.cycles_completed >= 1);

        // Drive a manual cycle through the worker and capture the
        // status just before the worker is parked (and after — we
        // read by polling, so the final state is what matters).
        handle.trigger(EvolutionTrigger::Manual { by: "t1".into() });
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let final_status = loop {
            let s = handle.cycle_status();
            if !matches!(s.phase, CyclePhase::Idle)
                || std::time::Instant::now() >= deadline
            {
                // Cycle should have advanced the counter past startup.
                if s.cycles_completed >= 2 && s.last_triggers.iter().any(|t| t == "manual") {
                    break s;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert_eq!(final_status.phase, CyclePhase::Idle);
        assert!(
            final_status
                .last_triggers
                .iter()
                .any(|t| t == "manual"),
            "last_triggers missing: {:?}",
            final_status.last_triggers
        );
        assert!(final_status.last_cycle_duration_ms.is_some());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn empty_observation_pool_short_circuits_before_merge() {
        // The startup sweep also gets short-circuited when no
        // observations exist; subsequent Manual triggers must not
        // flip the phase into Distilling nor bump cycles_completed.
        let tmp = tempfile::tempdir().unwrap();
        let manager = Arc::new(SkillManager::new(tmp.path().join("state")));
        let handle = start(
            manager.clone(),
            EvolutionOptions {
                policy: EvolutionPolicy::default(),
                quiet_window: Duration::from_millis(40),
                timer: None,
            },
        );

        tokio::time::sleep(Duration::from_millis(80)).await;
        let after_startup = handle.cycle_status();
        assert!(
            after_startup.cycle_skipped_empty >= 1,
            "startup sweep against empty pool already short-circuits"
        );

        // A manual trigger against an empty pool must also be a
        // skip, not a Distilling run.
        handle.trigger(EvolutionTrigger::Manual { by: "t0".into() });
        tokio::time::sleep(Duration::from_millis(150)).await;

        let status = handle.cycle_status();
        assert_eq!(status.phase, CyclePhase::Idle);
        assert_eq!(
            status.cycles_completed, 0,
            "cycles_completed stays at 0 when every cycle was skipped"
        );
        assert!(status.cycle_skipped_empty >= 1);
        assert_eq!(
            status.last_skipped_reason.as_deref(),
            Some("empty_pool"),
            "skip reason surfaces the empty-pool signal"
        );
        assert!(
            status
                .last_triggers
                .iter()
                .any(|t| t == "manual"),
            "last_triggers records what got skipped: {:?}",
            status.last_triggers
        );
    }
}
