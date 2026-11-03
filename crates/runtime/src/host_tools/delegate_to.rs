use agentik_core::tools::{ToolContext, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use crate::control::HostControl;

#[tool(
    name = "delegate_to",
    description = "Delegate a task to one of your direct child agents. Runs in the \
                   background and returns a task number (#N) immediately. Use \
                   `wait_task` with the task number to block until the result is \
                   ready, or `view_task_results` to poll for the output. Multiple \
                   delegates can run concurrently. The target agent's COMPLETE \
                   response becomes the task's result. For \
                   Researcher-to-Developer work, delegate capability \
                   implementation only: provide contracts and synthetic \
                   fixtures, never a research dataset or a request to \
                   run/interpret an analysis. You may delegate only to your \
                   own direct child agents (spawned via spawn_agent). Sibling, \
                   parent, and cross-branch delegation is rejected — spawn the \
                   agent you need first."
)]
pub(super) struct DelegateToInput {
    /// Name of the target agent: a direct child's short name (e.g. \
    /// "worker") or full path (e.g. "/root/you/worker").
    pub(super) agent_name: String,
    /// The task to send. For node/plugin development, include the requested \
    /// interface, acceptance criteria, and synthetic fixture instead of real \
    /// research data.
    task: String,
}

pub(super) struct DelegateToTool {
    pub(super) control: HostControl,
    pub(super) caller_path: String,
}

#[async_trait]
impl ToolFunction for DelegateToTool {
    type Input = DelegateToInput;

    /// Async — the caller is notified when the target agent finishes; the
    /// full response is pulled on demand via `view_task_results`.
    fn execution_mode(&self) -> agentik_core::tools::ExecutionMode {
        agentik_core::tools::ExecutionMode::Async
    }

    /// 24-hour timeout — delegated agents may run long analyses.
    fn timeout_seconds(&self) -> u64 {
        86400
    }

    async fn run(
        &self,
        input: DelegateToInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        match self
            .control
            .delegate(&self.caller_path, &input.agent_name, input.task)
            .await
        {
            Some(response) => Ok(ToolResult::success(response)),
            None => Ok(ToolResult::success(format!(
                "Delegation to '{}' failed — agent not found or host channel closed.",
                input.agent_name
            ))),
        }
    }

    async fn execute_with_context(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        let input: DelegateToInput = serde_json::from_value(input)?;
        let delegation_id = uuid::Uuid::new_v4();
        ctx.set_metadata(serde_json::json!({
            "kind": "delegation",
            "delegation_id": delegation_id,
            "caller_agent": self.caller_path,
            "target_agent": input.agent_name,
            "task": input.task,
        }));
        ctx.emit(
            agentik_core::tools::ProgressRecord::new("delegation")
                .status("pending")
                .message(format!("target={}", input.agent_name)),
        );
        match self
            .control
            .delegate_tracked(
                &input.agent_name,
                input.task,
                Some(self.caller_path.clone()),
                delegation_id,
                ctx.output.clone(),
            )
            .await
        {
            Some(response) => Ok(ToolResult::success(response)),
            None => Ok(ToolResult::success(format!(
                "Delegation to '{}' failed — agent not found or host channel closed.",
                input.agent_name
            ))),
        }
    }
}
