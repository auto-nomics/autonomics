use super::{
    PluginToolState,
    helpers::{resolve_target, tool_error},
    manifest::{editable_manifest, save_manifest},
};
use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;
use container_plugin::node_definition::NodeDefinition;

#[tool(
    name = "plugin_node_upsert",
    description = "Create or replace one node using the native structured schema. Host writes the manifest; call plugin_install after editing. Other nodes are preserved."
)]
pub(super) struct PluginNodeUpsertInput {
    pub(super) plugin_path: String,
    pub(super) node: NodeDefinition,
}
pub(super) struct PluginNodeUpsertTool {
    pub(super) state: PluginToolState,
}
#[async_trait]
impl ToolFunction for PluginNodeUpsertTool {
    type Input = PluginNodeUpsertInput;
    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        container_plugin::node_definition::validate(&input.node).map_err(tool_error)?;
        let target = resolve_target(&self.state, &input.plugin_path)
            .await
            .map_err(tool_error)?;
        let lock = self.state.registry.manifest_lock(&target.plugin_name);
        let _guard = lock.lock().await;
        let mut manifest = editable_manifest(&target).await.map_err(tool_error)?;
        let kind = input.node.kind.clone();
        if let Some(node) = manifest.nodes.iter_mut().find(|n| n.kind == kind) {
            *node = input.node;
        } else {
            manifest.nodes.push(input.node);
        }
        crate::plugin::validation::compiler::registry_compile(&manifest).map_err(tool_error)?;
        save_manifest(&target.workspace, &mut manifest).map_err(tool_error)?;
        Ok(ToolResult::success_json(
            serde_json::json!({"kind":kind,"activation_required":true,"next_action":"plugin_install"}),
        ))
    }
}
