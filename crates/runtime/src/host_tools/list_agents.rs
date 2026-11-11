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
                   Direct children come from the persisted identity registry, so \
                   dormant (passivated) children are listed too with their last \
                   activity; live children additionally carry their current status. \
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
        // Direct children from the persisted registry — includes dormant
        // virtual-actor identities, not just live instances.
        let records = self
            .control
            .list_sub_agents(self.self_path.as_str())
            .await
            .unwrap_or_default();

        let Some(status) = self.control.get_status().await else {
            return Ok(ToolResult::success("Failed to get host status."));
        };

        // One-hop visibility: parent + direct children (self
        // excluded). Siblings and other branches are invisible, and
        // the host's global topology fields are never exposed.
        // Live entries carry their real-time status.
        let live: Vec<_> = status
            .agents
            .into_iter()
            .filter(|a| {
                agentik_types::AgentPath::try_from(a.path.as_str())
                    .is_ok_and(|p| p != self.self_path && visible_from(&self.self_path, &p))
            })
            .collect();
        let live_child_paths: std::collections::HashSet<&str> = live
            .iter()
            .filter(|a| {
                agentik_types::AgentPath::try_from(a.path.as_str())
                    .is_ok_and(|p| p.is_direct_child_of(&self.self_path))
            })
            .map(|a| a.path.as_str())
            .collect();

        // Dormant children: persisted identities with no live instance.
        // Status is derived, not stored — a dormant agent has no runtime
        // status, so we surface the last-activity timestamp instead and
        // let the caller judge staleness.
        let dormant: Vec<_> = records
            .iter()
            .filter(|record| !live_child_paths.contains(record.name.as_str()))
            .map(|record| {
                let name = record
                    .name
                    .rsplit('/')
                    .next()
                    .unwrap_or(&record.name)
                    .to_string();
                serde_json::json!({
                    "name": name,
                    "path": record.name,
                    "dormant": true,
                    "last_active": record.last_active,
                    "agent_id": record.id,
                })
            })
            .collect();

        let mut profiles = status.profiles;
        profiles.retain(|p| p.name != self.self_path.name());
        Ok(ToolResult::success_json(serde_json::json!({
            "agents": live,
            "dormant_children": dormant,
            "profiles": profiles,
        })))
    }
}
