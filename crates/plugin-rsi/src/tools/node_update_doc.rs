//! Tool: replace one plugin node's documentation without changing its spec.

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;

use super::PluginToolState;
use super::helpers::{resolve_target, tool_error};
use super::manifest::{editable_manifest, save_manifest, selected_node};

#[tool(
    name = "plugin_node_update_doc",
    description = "Replace one plugin node's documentation without changing its structural spec."
)]
pub(super) struct PluginNodeUpdateDocInput {
    /// Plugin workspace path, such as `/plugins/dev/hello-world`.
    plugin_path: String,
    /// Kind of the node to update.
    node_kind: String,
    /// Complete replacement documentation. Markdown formatting is allowed.
    doc: String,
}

pub(super) struct PluginNodeUpdateDocTool {
    pub(super) state: PluginToolState,
}

#[async_trait]
impl ToolFunction for PluginNodeUpdateDocTool {
    type Input = PluginNodeUpdateDocInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        if input.doc.trim().is_empty() {
            return Err(ToolError::ValidationFailed {
                message: "node documentation cannot be empty".into(),
            });
        }
        let target = resolve_target(&self.state, &input.plugin_path)
            .await
            .map_err(tool_error)?;
        let manifest_lock = self.state.registry.manifest_lock(&target.plugin_name);
        let _guard = manifest_lock.lock().await;
        let mut manifest = editable_manifest(&target).await.map_err(tool_error)?;
        let mut node = selected_node(&manifest, &input.node_kind).map_err(tool_error)?;
        node.doc = input.doc;
        let index = manifest
            .nodes
            .iter()
            .position(|existing| existing.kind == node.kind)
            .expect("selected node position");
        manifest.nodes[index] = node;
        save_manifest(&target.workspace, &manifest).map_err(tool_error)?;
        Ok(ToolResult::success("updated node documentation"))
    }
}
