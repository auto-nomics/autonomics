//! Tool: add one complete node definition to a plugin VFS workspace.

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;
use container_plugin::node_definition::NodeDefinition;
use serde_json::Value;

use super::PluginToolState;
use super::helpers::{parse_tool_input, resolve_target, tool_error};
use super::manifest::{editable_manifest, save_manifest};

#[tool(
    name = "plugin_node_create",
    description = "Add one complete node definition to a plugin VFS workspace. Create its script separately before review."
)]
pub(super) struct PluginNodeCreateInput {
    /// Plugin workspace path, such as `/plugins/dev/hello-world`.
    plugin_path: String,
    /// Complete NodeDefinition value in container-plugin JSON form.
    node: NodeDefinition,
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use agentik_core::tools::ToolFunction as _;
    use serde_json::json;

    fn complete_node(deprecated: Value) -> Value {
        json!({
            "kind": "demo_node",
            "desc": "Copy a file",
            "doc": "Copy input 0 to output 0.",
            "deprecated": deprecated,
            "ports": {
                "inputs": [{ "type": "file" }],
                "outputs": [{ "path": "result.txt" }]
            },
            "command": {
                "interpreter": "sh",
                "argv": [],
                "script_file": "scripts/adapter.sh",
                "env": {},
                "files": {}
            }
        })
    }

    #[test]
    fn node_schema_exposes_nested_types_to_tool_call_models() {
        let registrations = super::super::plugin_development_tool_registrations("schema-agent");
        let registration = registrations
            .iter()
            .find(|registration| registration.definition.name == "plugin_node_create")
            .unwrap();
        let node = &registration.definition.input_schema.properties["node"];

        assert_eq!(node["properties"]["deprecated"]["type"], "boolean");
        assert_eq!(
            node["properties"]["ports"]["properties"]["inputs"]["type"],
            "array"
        );
        assert_eq!(
            node["properties"]["ports"]["properties"]["outputs"]["type"],
            "array"
        );
        assert_eq!(node["properties"]["params"]["type"], "object");
    }

    #[tokio::test]
    async fn malformed_nested_node_fields_report_their_path() {
        let tool = PluginNodeCreateTool {
            state: super::super::PluginToolState {
                registry: super::super::PluginDevelopmentToolsetRegistry::global(),
                principal: super::super::principal_for_agent("schema-agent"),
            },
        };
        let error = tool
            .execute(json!({
                "plugin_path": "/plugins/dev/demo-plugin",
                "node": complete_node(json!("false")),
            }))
            .await
            .expect_err("string deprecated must fail");

        assert!(
            error.to_string().contains("node.deprecated"),
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

    async fn execute(&self, input: Value) -> Result<ToolResult, ToolError> {
        let typed = parse_tool_input(input)?;
        self.run(typed).await
    }

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let node = input.node;
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
