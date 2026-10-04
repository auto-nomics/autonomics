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
    pub(super) fn coalescing(&self, queued: usize) {
        let mut core = self.lock();
        if !matches!(core.phase, CyclePhase::Coalescing { .. }) {
            core.phase_since = std::time::Instant::now();
        }
        core.phase = CyclePhase::Coalescing { queued };
    }

    /// A cycle is starting for these trigger labels.
    pub(super) fn distilling(&self, triggers: &[&'static str]) {
        let mut core = self.lock();
        core.phase = CyclePhase::Distilling {
            triggers: triggers.iter().map(|t| t.to_string()).collect(),
        };
        core.phase_since = std::time::Instant::now();
    }

    /// A cycle returned — success or failure — and the worker is idle
    /// again.
    pub(super) fn cycle_finished(
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
    pub(super) fn cycle_skipped_empty(&self, triggers: &[&'static str]) {
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

    pub(super) fn snapshot(&self, dropped_commands: u64) -> CycleStatus {
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
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Run one evolution cycle: distill, then apply the approval policy.
///
/// Idempotent — running it twice without new observations produces a
/// no-op report (consumed clusters are skipped at distill time).
/// Internal-use only: the worker is the only caller. `pub(crate)`
/// because routing an external request through this function would
use std::time::Duration;
