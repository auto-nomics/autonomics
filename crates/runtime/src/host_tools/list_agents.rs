use agentik_core::tools::ToolFunction;
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use super::visible_from;
use crate::control::HostControl;

#[tool(
    name = "list_agents",
    description = "List the agents visible to you — your direct parent (if any) and \
                   your direct children — plus the spawnable role profiles, as JSON. \
                   Each agent entry includes both `name` (short name) and `path` \
                   (full hierarchical path). Use the path with `delegate_to` when \
                   names are ambiguous."
)]
pub(super) struct ListAgentsInput {}

pub(super) struct ListAgentsTool {
    pub(super) control: HostControl,
    pub(super) self_path: agentik_types::AgentPath,
}

#[async_trait]
impl ToolFunction for ListAgentsTool {
    type Input = ListAgentsInput;

    async fn run(
        &self,
        _input: ListAgentsInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        match self.control.get_status().await {
            Some(status) => {
                // One-hop visibility: parent + direct children (self
                // excluded). Siblings and other branches are invisible, and
                // the host's global topology fields are never exposed.
                let visible: Vec<_> = status
                    .agents
                    .into_iter()
                    .filter(|a| {
                        agentik_types::AgentPath::try_from(a.path.as_str())
                            .is_ok_and(|p| p != self.self_path && visible_from(&self.self_path, &p))
                    })
                    .collect();
                let mut profiles = status.profiles;
                profiles.retain(|p| p.name != self.self_path.name());
                Ok(ToolResult::success_json(serde_json::json!({
                    "agents": visible,
                    "profiles": profiles,
                })))
            }
            None => Ok(ToolResult::success("Failed to get host status.")),
        }
    }
}
