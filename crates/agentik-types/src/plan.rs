//! Per-session task plan types.
//!
//! The plan is a first-class citizen of a conversation (`Session`) — it lives
//! as long as that session does, is persisted per `(agent_id, session_id)` in
//! the `session_plans` table, is copied into forks, and is surfaced to the
//! model via the `update_plan` tool.
//!
//! ## Design (ported from Codex's `update_plan`)
//!
//! Like the Codex original, the plan is **full-replacement**: each
//! `update_plan` call carries the complete new plan snapshot, not a diff.
//! This keeps the tool handler stateless and makes updates idempotent.
//!
//! Unlike Codex (which stores nothing server-side), the plan here is
//! **persisted as per-session state** so it survives restarts of its session.

use serde::{Deserialize, Serialize};

/// Status of a single step in the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    InProgress,
    Completed,
}

impl StepStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "in_progress" => Some(Self::InProgress),
            "completed" => Some(Self::Completed),
            _ => None,
        }
    }
}

/// One step in the plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    /// Short, human-readable step description (ideally ≤ 5-7 words).
    pub step: String,
    pub status: StepStatus,
}

/// The payload of an `update_plan` tool call — a full-snapshot replacement.
///
/// `explanation` is optional context for *why* the plan changed (e.g. when
/// redirecting mid-task). It is surfaced to the user but not stored long-term;
/// only [`plan`](Self::plan) becomes the agent's persistent plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PlanUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    pub plan: Vec<PlanStep>,
}

impl PlanUpdate {
    /// Count how many steps are completed.
    pub fn completed_count(&self) -> usize {
        self.plan
            .iter()
            .filter(|s| s.status == StepStatus::Completed)
            .count()
    }
}

/// The persistent session plan stored on each `Session` and in the storage layer.
///
/// This wraps a [`PlanUpdate`] with a revision counter so observers can detect
/// changes efficiently. An empty `plan` vector means no plan is active.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AgentPlan {
    /// Monotonically increasing revision. Incremented on every update.
    pub revision: u64,
    /// The latest plan snapshot. Empty when no plan has been set.
    #[serde(default)]
    pub update: PlanUpdate,
}

impl AgentPlan {
    /// Create a new plan with revision 0 and no steps.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the plan content and bump the revision.
    pub fn replace(&mut self, update: PlanUpdate) {
        self.revision = self.revision.saturating_add(1);
        self.update = update;
    }

    /// Whether any plan steps exist.
    pub fn is_empty(&self) -> bool {
        self.update.plan.is_empty()
    }

    /// `(completed, total)` progress tuple, or `None` if the plan is empty.
    pub fn progress(&self) -> Option<(usize, usize)> {
        let total = self.update.plan.len();
        if total == 0 {
            None
        } else {
            Some((self.update.completed_count(), total))
        }
    }
}
