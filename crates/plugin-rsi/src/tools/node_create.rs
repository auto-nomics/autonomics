//! Tool: add one complete node definition to a plugin VFS workspace.

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;
use container_plugin::node_definition::NodeDefinition;
use serde_json::Value;

use super::PluginToolState;
use super::helpers::{resolve_target, tool_error};
use super::manifest::{editable_manifest, save_manifest};

#[tool(
    name = "plugin_node_create",
    description = "Add one complete node definition to a plugin VFS workspace. Create its script separately before review."
)]
pub(super) struct PluginNodeCreateInput {
    /// Plugin workspace path, such as `/plugins/dev/hello-world`.
    plugin_path: String,
    /// Complete NodeDefinition value in container-plugin JSON form.
    node: Value,
}

pub(super) struct PluginNodeCreateTool {
    pub(super) state: PluginToolState,
}

#[async_trait]
impl ToolFunction for PluginNodeCreateTool {
    type Input = PluginNodeCreateInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let node: NodeDefinition =
            serde_json::from_value(input.node).map_err(|error| ToolError::ValidationFailed {
                message: format!("invalid node definition: {error}"),
            })?;
        container_plugin::node_definition::validate(&node).map_err(|error| {
            ToolError::ValidationFailed {
                message: format!("invalid node definition: {error}"),
            }
        })?;

        let target = resolve_target(&self.state, &input.plugin_path)
            .await
            .map_err(tool_error)?;
        let manifest_lock = self.state.registry.manifest_lock(&target.plugin_name);
        let _guard = manifest_lock.lock().await;
        let mut manifest = editable_manifest(&target).await.map_err(tool_error)?;
        if manifest
            .nodes
            .iter()
            .any(|existing| existing.kind == node.kind)
        {
            return Err(ToolError::ValidationFailed {
                message: format!("node kind `{}` already exists", node.kind),
            });
        }
        manifest.nodes.push(node);
        save_manifest(&target.workspace, &manifest).map_err(tool_error)?;
        Ok(ToolResult::success("created node"))
    }
}
