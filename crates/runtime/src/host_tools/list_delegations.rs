use agentik_core::tools::ToolFunction;
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use crate::control::HostControl;

#[tool(
    name = "list_delegations",
    description = "List your agent-to-agent delegations with stable IDs, target \
                   agent, turn ID, terminal status, and response. Filter by \
                   target agent or status; omit filters to list all of your \
                   delegations, newest first."
)]
pub(super) struct ListDelegationsInput {
    /// Full or short target-agent name.
    target_agent: Option<String>,
    /// `pending`, `running`, `completed`, `interrupted`, or `failed`.
    status: Option<String>,
}

pub(super) struct ListDelegationsTool {
    pub(super) control: HostControl,
    pub(super) caller_path: String,
}

#[async_trait]
impl ToolFunction for ListDelegationsTool {
    type Input = ListDelegationsInput;

    async fn run(
        &self,
        input: ListDelegationsInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        match self
            .control
            .list_delegations(
                Some(&self.caller_path),
                input.target_agent.as_deref(),
                input.status.as_deref(),
            )
            .await
        {
            Some(delegations) => Ok(ToolResult::success_json(
                serde_json::to_value(delegations).unwrap_or_default(),
            )),
            None => Ok(ToolResult::success(
                "Failed to query delegations — host unavailable.",
            )),
        }
    }
}
