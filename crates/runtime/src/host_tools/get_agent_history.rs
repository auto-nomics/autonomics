use agentik_core::tools::ToolFunction;
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use super::visible_from;
use crate::control::HostControl;

#[tool(
    name = "get_agent_history",
    description = "Read the persisted conversation history of a visible agent \
                   (yourself, your direct parent, or one of your direct children). \
                   Returns the most recent messages for the agent, which can be used \
                   to inspect what a delegated task actually did."
)]
pub(super) struct GetAgentHistoryInput {
    /// Full or short name of the target agent.
    pub(super) agent_name: String,
    /// Maximum number of messages to return. Default 20; clamped to 100.
    #[default = 20]
    limit: Option<usize>,
}

pub(super) struct GetAgentHistoryTool {
    pub(super) control: HostControl,
    pub(super) self_path: agentik_types::AgentPath,
}

#[async_trait]
impl ToolFunction for GetAgentHistoryTool {
    type Input = GetAgentHistoryInput;

    async fn run(
        &self,
        input: GetAgentHistoryInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        let limit = input.limit.unwrap_or(20).clamp(1, 100);
        match self.control.agent_history(&input.agent_name, limit).await {
            Some(history) => {
                // Visibility gate on the resolved target path. A history
                // whose path cannot be parsed is treated as invisible.
                let visible = agentik_types::AgentPath::try_from(history.agent_path.as_str())
                    .is_ok_and(|p| visible_from(&self.self_path, &p));
                if !visible {
                    return Ok(ToolResult::success(format!(
                        "Agent '{}' not found or not visible to you.",
                        input.agent_name
                    )));
                }
                Ok(ToolResult::success_json(
                    serde_json::to_value(history).unwrap_or_default(),
                ))
            }
            None => Ok(ToolResult::success(
                "Failed to read agent history — host unavailable.",
            )),
        }
    }
}
