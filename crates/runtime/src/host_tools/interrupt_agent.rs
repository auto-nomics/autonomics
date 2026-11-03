use agentik_core::tools::ToolFunction;
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use crate::control::HostControl;

#[tool(
    name = "interrupt_agent",
    description = "Interrupt one of your direct child agents' CURRENT turn without \
                   shutting it down. The child emits LifecycleChanged(Cancelled), \
                   aborts any in-flight tool calls, and remains registered — a \
                   follow-up delegate_to will start a fresh turn on the same \
                   session. Use this when:\n\
                   - You delegated a long task and want to cancel it (e.g. wrong \
                     agent selected, task is taking too long, etc.)\n\
                   - The child is stuck in a retry loop and you want to break out.\n\
                   - You want to redirect the child's work mid-turn."
)]
pub(super) struct InterruptAgentInput {
    /// Name of the child agent to interrupt. Accepts a short name (e.g. \
    /// "worker") or full path.
    pub(super) agent_name: String,
    /// Optional human-readable reason logged alongside the cancel \
    /// event. Useful when debugging why an agent got interrupted.
    #[serde(default)]
    reason: Option<String>,
}

pub(super) struct InterruptAgentTool {
    pub(super) control: HostControl,
    pub(super) self_path: agentik_types::AgentPath,
}

#[async_trait]
impl ToolFunction for InterruptAgentTool {
    type Input = InterruptAgentInput;

    /// Synchronous — interrupt is a fast operation. The child's
    /// cancel_token is cancelled, which propagates immediately to any
    /// in-flight LLM request or tool execution. The child then emits
    /// `LifecycleChanged(Cancelled)` through the event stream; if you're
    /// tracking the result, follow up with `view_task_results` to see the
    /// Cancelled status.
    /// Sync (default) — returns as soon as the host confirms the cancel
    /// command (or denies a non-child target).
    /// 1 hour cap — interrupt itself should be near-instant; the cap
    /// only matters if the host command channel is jammed.
    fn timeout_seconds(&self) -> u64 {
        3600
    }

    async fn run(
        &self,
        input: InterruptAgentInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        let reason = input
            .reason
            .as_deref()
            .unwrap_or("user-requested interrupt");
        tracing::info!(
            agent = %input.agent_name,
            reason = %reason,
            "interrupt_agent: cancelling current turn"
        );
        match self
            .control
            .interrupt_child_agent(self.self_path.as_str(), &input.agent_name)
            .await
        {
            Some(Ok(())) => Ok(ToolResult::success(format!(
                "Interrupt requested for agent '{}'. \
                 The current turn will be cancelled; the agent remains \
                 available for new messages. Reason: {}",
                input.agent_name, reason
            ))),
            Some(Err(e)) => Ok(ToolResult::success(format!("Interrupt failed: {e}"))),
            None => Ok(ToolResult::success(
                "Interrupt failed: host unavailable (runtime shut down or unresponsive).",
            )),
        }
    }
}
