use agentik_core::tools::ToolFunction;
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use crate::control::HostControl;

#[tool(
    name = "spawn_agent",
    description = "Spawn a new child agent and register it with the host. \
                   The child's path is automatically derived from your path \
                   (e.g. spawning 'worker' becomes /root/you/worker). \
                   Use profile_segment='researcher' for data analysis, DAG \
                   execution, and interpretation. Use profile_segment='developer' \
                   only to implement, validate, install, or repair a plugin/node. \
                   Use profile_segment='browser' for web navigation, extraction, \
                   and form interaction — the child gets a private browser \
                   session with browser_* tools. Use profile_segment='curator' \
                   to create or restructure wiki knowledge entries — the child \
                   is the only role holding wiki write tools. Do not use \
                   developer as an execution service for research data; omit \
                   profile_segment to reuse your own role."
)]
pub(super) struct SpawnAgentInput {
    /// Short name for the new agent (a path segment, e.g. `worker`, `analyst`).
    /// Must be lowercase `[a-z0-9_]`, 1-32 chars.
    pub(super) agent_name: String,
    /// Role to instantiate: `researcher`, `developer`, `browser`, `curator`, \
    /// or omitted to reuse your own role.
    profile_segment: Option<String>,
}

pub(super) struct SpawnAgentTool {
    pub(super) control: HostControl,
    pub(super) caller_path: agentik_types::AgentPath,
    pub(super) caller_kind: agentik_core::AgentKind,
}

#[async_trait]
impl ToolFunction for SpawnAgentTool {
    type Input = SpawnAgentInput;

    async fn run(
        &self,
        input: SpawnAgentInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        match self
            .control
            .spawn_agent(
                &input.agent_name,
                &self.caller_path,
                self.caller_kind,
                input.profile_segment.as_deref(),
            )
            .await
        {
            Ok(path) => Ok(ToolResult::success(format!(
                "Agent at path `{path}` spawned and registered."
            ))),
            Err(e) => Ok(ToolResult::success(format!("Spawn failed: {e}"))),
        }
    }
}
