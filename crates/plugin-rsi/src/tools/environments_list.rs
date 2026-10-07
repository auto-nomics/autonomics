//! Tool: list host-approved digest-pinned base images.

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;
use serde_json::{Value, json};

use super::PluginToolState;
use super::helpers::tool_error;

#[tool(
    name = "plugin_environments_list",
    description = "List host-approved digest-pinned base images available to plugin development."
)]
pub(super) struct PluginEnvironmentsListInput {}

pub(super) struct PluginEnvironmentsListTool {
    pub(super) state: PluginToolState,
}

#[async_trait]
impl ToolFunction for PluginEnvironmentsListTool {
    type Input = PluginEnvironmentsListInput;

    async fn run(&self, _input: Self::Input) -> Result<ToolResult, ToolError> {
        let environments = self.state.registry.environments().map_err(tool_error)?;
        let environments = environments
            .list()
            .into_iter()
            .map(|(id, environment)| {
                json!({
                    "id": id,
                    "reference": environment.reference,
                    "interpreters": environment.interpreters,
                })
            })
            .collect::<Vec<_>>();
        Ok(ToolResult::success_json(Value::Array(environments)))
    }
}
