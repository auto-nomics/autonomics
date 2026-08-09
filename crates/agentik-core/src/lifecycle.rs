use thiserror::Error;

/// Re-exported so historical `agentik_core::lifecycle::AgentLifecycleStatus`
/// paths keep resolving.
pub use agentik_types::AgentLifecycleStatus;

#[derive(Debug, Error)]
pub enum AgentLifecycleError {
    #[error("{0}")]
    OtherLifecycleError(String),
}

/// In-memory lifecycle tracker for a [`Session`](crate::Session).
///
/// The status is the single source of truth for the agent's current phase.
/// Every transition should go through [`set_status`](Self::set_status),
/// which records the new status. The caller is responsible for emitting
/// `AgentEvent::LifecycleChanged` alongside the transition (typically via
/// `Session::set_lifecycle`).
pub struct AgentLifecycle {
    status: AgentLifecycleStatus,
}

impl AgentLifecycle {
    pub fn new() -> Self {
        Self {
            status: AgentLifecycleStatus::Idle,
        }
    }

    pub fn status(&self) -> &AgentLifecycleStatus {
        &self.status
    }

    /// Set the lifecycle status to `status`. Returns `true` if the status
    /// actually changed (caller can use this to decide whether to emit an
    /// event).
    pub fn set_status(&mut self, status: AgentLifecycleStatus) -> bool {
        if self.status != status {
            self.status = status;
            true
        } else {
            false
        }
    }

    // ── Convenience predicates ───────────────────────────

    /// Returns `true` when the agent is actively processing — the session
    /// loop should continue iterating.
    pub fn is_running(&self) -> bool {
        self.status.is_active()
    }

    /// Returns `true` when the agent is idle.
    pub fn is_idle(&self) -> bool {
        self.status.is_idle()
    }

    // ── Legacy compatibility shims ───────────────────────
    //
    // Older code called `set_idle()` / `set_running()` / `set_aborted()`.
    // These are kept as thin wrappers so existing call sites compile while
    // being migrated to the richer `set_status` API.

    #[deprecated(note = "use set_status(AgentLifecycleStatus::Idle)")]
    pub fn set_idle(&mut self) {
        self.status = AgentLifecycleStatus::Idle;
    }

    #[deprecated(note = "use set_status(AgentLifecycleStatus::Requesting)")]
    pub fn set_running(&mut self) {
        self.status = AgentLifecycleStatus::Requesting;
    }

    #[deprecated(note = "use set_status(AgentLifecycleStatus::Idle); Aborted is legacy")]
    pub fn set_aborted(&mut self) {
        self.status = AgentLifecycleStatus::Idle;
    }
}

impl Default for AgentLifecycle {
    fn default() -> Self {
        Self::new()
    }
}
