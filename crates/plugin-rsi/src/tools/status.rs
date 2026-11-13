use super::{
    PluginToolState,
    helpers::{resolve_target, tool_error},
};
use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;
#[tool(
    name = "plugin_status",
    description = "Compare actual development content with the active immutable version. Reports unactivated edits, content fingerprints and behavior verification; saving a script never activates it."
)]
pub(super) struct PluginStatusInput {
    plugin_path: String,
}
pub(super) struct PluginStatusTool {
    pub(super) state: PluginToolState,
}
#[async_trait]
impl ToolFunction for PluginStatusTool {
    type Input = PluginStatusInput;
    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let target = resolve_target(&self.state, &input.plugin_path)
            .await
            .map_err(tool_error)?;
        let infra = self.state.registry.infra().map_err(tool_error)?;
        Ok(ToolResult::success_json(
            infra
                .plugin_status(&target.plugin_name)
                .map_err(tool_error)?,
        ))
    }
}
