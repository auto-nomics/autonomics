//! Built-in plan and task tools.
//!
//! These are the always-injected tools that the agent framework requires
//! for task signaling. Primitive tools (bash, read, write, etc.) live in
//! the `agentik-tools` crate.

pub mod plan;
pub mod task_tools;

pub use plan::{PlanHandle, PlanStepInput, UpdatePlanInput, UpdatePlanTool};
pub use task_tools::{
    TaskResultViewerTool, ViewTaskResultsInput, WaitTaskInput, WaitTaskTool, task_registrations,
};

/// Create the `update_plan` tool registration from a [`PlanHandle`].
///
/// Called by [`AgentBuilder`](crate::agent_builder::AgentBuilder) after
/// `AgentShared` is constructed, since the handle needs the shared plan
/// `ArcSwap`, agent id, storage, and event channel.
pub fn plan_registrations(handle: PlanHandle) -> Vec<crate::tools::ToolRegistration> {
    vec![crate::tools::ToolRegistration::from(UpdatePlanTool::new(
        handle,
    ))]
}
