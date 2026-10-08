//! Tool: add one complete node definition to a plugin VFS workspace.

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;
use container_plugin::node_definition::NodeDefinition;
use serde_json::json;

use super::PluginToolState;
use super::helpers::{resolve_target, tool_error};
use super::manifest::{editable_manifest, load_manifest, save_manifest};

#[tool(
    name = "plugin_node_create",
    description = "Add one complete node definition to a plugin VFS workspace, authored as a standalone \
                   NodeDefinition TOML document: top-level `kind`/`desc`/`doc` with `[ports]`, \
                   `[params.<name>]`, and `[command]` tables — without the `nodes.` prefixes used inside \
                   manifest.toml. Create its script separately before review. The node is validated and the \
                   whole manifest compiled before it lands."
)]
pub(super) struct PluginNodeCreateInput {
    /// Plugin workspace path, such as `/plugins/dev/hello-world`.
    plugin_path: String,
    /// Complete NodeDefinition as a standalone TOML document.
    node_toml: String,
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use agentik_core::tools::ToolFunction as _;
    use serde_json::json;

    fn node_toml(deprecated: &str) -> String {
        format!(
            "kind = \"demo_node\"\n\
             desc = \"Copy a file\"\n\
             doc = \"Copy input 0 to output 0.\"\n\
             deprecated = {deprecated}\n\
             timeout_secs = 3600\n\
             \n\
             [ports]\n\
             inputs = [{{ type = \"file\" }}]\n\
             outputs = [{{ path = \"result.txt\" }}]\n\
             \n\
             [command]\n\
             interpreter = \"sh\"\n\
             argv = []\n\
             script_file = \"scripts/adapter.sh\"\n"
        )
    }

    #[test]
    fn create_schema_takes_flat_node_toml_text() {
        let registrations = super::super::plugin_development_tool_registrations("schema-agent");
        let registration = registrations
            .iter()
            .find(|registration| registration.definition.name == "plugin_node_create")
            .unwrap();
        let properties = &registration.definition.input_schema.properties;

        assert!(properties.get("plugin_path").is_some());
        assert_eq!(properties["node_toml"]["type"], "string");
        assert!(
            properties.get("node").is_none(),
            "the nested node parameter must be gone"
        );
    }

    #[tokio::test]
    async fn malformed_node_toml_is_rejected_before_the_workspace_is_touched() {
        let tool = PluginNodeCreateTool {
            state: super::super::PluginToolState {
                registry: super::super::PluginDevelopmentToolsetRegistry::global(),
                principal: super::super::principal_for_agent("schema-agent"),
            },
        };
        let error = tool
            .execute(json!({
                "plugin_path": "/plugins/dev/demo-plugin",
                "node_toml": node_toml("\"false\""),
            }))
            .await
            .expect_err("string deprecated must fail");

        assert!(
            error.to_string().contains("deprecated"),
            "unexpected error: {error}"
        );
    }
}

pub(super) struct PluginNodeCreateTool {
    pub(super) state: PluginToolState,
}

#[async_trait]
impl ToolFunction for PluginNodeCreateTool {
    type Input = PluginNodeCreateInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let node: NodeDefinition =
            toml::from_str(&input.node_toml).map_err(|error| ToolError::ValidationFailed {
                message: format!("invalid node definition TOML: {error}"),
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
        crate::validate::registry_compile(&manifest).map_err(|error| {
            ToolError::ValidationFailed {
                message: format!("manifest does not compile with the new node: {error}"),
            }
        })?;
        let created_kind = manifest
            .nodes
            .last()
            .map(|node| node.kind.clone())
            .unwrap_or_default();
        save_manifest(&target.workspace, &mut manifest).map_err(tool_error)?;
        // Reload from disk: what landed must parse (the write is a fresh
        // serialization, so this also guards round-trip drift).
        let reloaded = load_manifest(&target).await.map_err(tool_error)?;
        Ok(ToolResult::success_json(json!({
            "kind": created_kind,
            "plugin_vfs_path": target.virtual_path,
            "node_count": reloaded.nodes.len(),
        })))
    }
}
