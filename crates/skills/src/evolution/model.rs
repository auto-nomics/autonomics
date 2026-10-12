/// Why an evolution cycle was attempted. Pure signal — the workflow
/// re-derives everything it needs from the stores.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvolutionTrigger {
    /// Explicit request (CLI, agent tool, future host control).
    Manual { by: String },
    /// Service start: sweep once so overnight accumulation is dealt
    /// with before anyone asks.
    Startup,
    /// Periodic sweep.
    Timer,
    /// A skill-eligible observation was just recorded. Fired
    /// best-effort by the recording tools (agent `evo_observe`, eval
    /// auto-capture); a full channel or a dead worker drops the event
    /// and counts it — the next timer sweep still picks the evidence
    /// up, so the trigger is an optimization, not a correctness
    /// dependency.
    Observation,
}

impl EvolutionTrigger {
    pub fn label(&self) -> &'static str {
        match self {
            EvolutionTrigger::Manual { .. } => "manual",
            EvolutionTrigger::Startup => "startup",
            EvolutionTrigger::Timer => "timer",
            EvolutionTrigger::Observation => "observation",
        }
    }
}

/// How one cycle advances the library. There is no review gate:
/// everything the cycle writes it also installs, bounded by the cap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvolutionPolicy {
    /// Upper bound on installs per cycle — the library advances in
    /// bounded steps even after a long accumulation; overflow applies
    /// on the next cycle.
    pub max_installs_per_cycle: usize,
}

impl Default for EvolutionPolicy {
    fn default() -> Self {
        Self {
            max_installs_per_cycle: 5,
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
    /// Names installed into the live library this cycle.
    pub installed: Vec<String>,
    /// Proposals deferred past the install cap — the next cycle
    /// applies them.
    pub deferred: usize,
    /// (cluster hash, reason) — skipped clusters.
    pub skipped: Vec<(String, String)>,
}

impl EvolutionReport {
    pub fn acted(&self) -> bool {
        !self.proposals_written.is_empty() || !self.installed.is_empty()
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
use std::time::Duration;
