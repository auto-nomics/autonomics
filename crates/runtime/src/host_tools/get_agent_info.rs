use agentik_core::tools::ToolFunction;
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use super::visible_from;
use crate::control::HostControl;

#[tool(
    name = "get_agent_info",
    description = "Get detailed capability info for a visible agent (yourself, your \
                   direct parent, or one of your direct children): summary, tags, \
                   expertise areas, and available tools. Also answers for role \
                   profiles (e.g. 'researcher') to inspect what a spawned child \
                   of that kind can do."
)]
pub(super) struct GetAgentInfoInput {
    /// Name of the agent to query.
    pub(super) agent_name: String,
}

pub(super) struct GetAgentInfoTool {
    pub(super) control: HostControl,
    pub(super) self_path: agentik_types::AgentPath,
}

#[async_trait]
impl ToolFunction for GetAgentInfoTool {
    type Input = GetAgentInfoInput;

    async fn run(
        &self,
        input: GetAgentInfoInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        match self.control.get_agent_info(&input.agent_name).await {
            Some(info) => {
                // Visibility gate: only parent/children (and self) are
                // answerable. Non-`/root` paths are the static role-profile
                // fallback and are always allowed. Invisible agents get the
                // same message as not-found so callers cannot probe the
                // registry.
                if let Ok(path) = agentik_types::AgentPath::try_from(info.path.as_str()) {
                    if !visible_from(&self.self_path, &path) {
                        return Ok(ToolResult::success(format!(
                            "Agent '{}' not found.",
                            input.agent_name
                        )));
                    }
                }
                Ok(ToolResult::success_json(
                    serde_json::to_value(&info).unwrap_or_default(),
                ))
            }
            None => Ok(ToolResult::success(format!(
                "Agent '{}' not found.",
                input.agent_name
            ))),
        }
    }
}
