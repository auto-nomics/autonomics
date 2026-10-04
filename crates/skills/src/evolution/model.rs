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
use std::time::Duration;
