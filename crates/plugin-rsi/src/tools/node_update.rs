//! Tool: replace one editable plugin node's complete manifest definition.

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;
use container_plugin::node_definition::NodeDefinition;
use serde_json::{Value, json};

use super::PluginToolState;
use super::helpers::{parse_tool_input, resolve_target, tool_error};
use super::manifest::{editable_manifest, save_manifest};

#[tool(
    name = "plugin_node_update",
    description = "Replace one plugin node's complete manifest definition, including its params, ports, command, and resources. \
                   Provide the full replacement definition; partial patches are rejected."
)]
pub(super) struct PluginNodeUpdateInput {
    /// Plugin workspace path, such as `/plugins/dev/hello-world`.
    plugin_path: String,
    /// Kind of the existing node to replace.
    node_kind: String,
    /// Complete replacement NodeDefinition in container-plugin JSON form.
    node: NodeDefinition,
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use agentik_core::tools::ToolFunction as _;
    use serde_json::json;

    #[test]
    fn update_schema_exposes_nested_node_types() {
        let registrations = super::super::plugin_development_tool_registrations("schema-agent");
        let registration = registrations
            .iter()
            .find(|registration| registration.definition.name == "plugin_node_update")
            .unwrap();
        let node = &registration.definition.input_schema.properties["node"];

        assert_eq!(node["properties"]["deprecated"]["type"], "boolean");
        assert_eq!(
            node["properties"]["ports"]["properties"]["inputs"]["type"],
            "array"
        );
        assert_eq!(node["properties"]["params"]["type"], "object");
    }

    #[tokio::test]
    async fn malformed_nested_update_fields_report_their_path() {
        let tool = PluginNodeUpdateTool {
            state: super::super::PluginToolState {
                registry: super::super::PluginDevelopmentToolsetRegistry::global(),
                principal: super::super::principal_for_agent("schema-agent"),
            },
        };
        let error = tool
            .execute(json!({
                "plugin_path": "/plugins/dev/demo-plugin",
                "node_kind": "demo_node",
                "node": {
                    "kind": "replacement_node",
                    "desc": "Copy a file",
                    "doc": "Copy input 0 to output 0.",
                    "deprecated": json!("false"),
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
                }
            }))
            .await
            .expect_err("string deprecated must fail");

        assert!(
            error.to_string().contains("node.deprecated"),
            "unexpected error: {error}"
        );
    }
}

pub(super) struct PluginNodeUpdateTool {
    pub(super) state: PluginToolState,
}

#[async_trait]
impl ToolFunction for PluginNodeUpdateTool {
    type Input = PluginNodeUpdateInput;

    async fn execute(&self, input: Value) -> Result<ToolResult, ToolError> {
        let typed = parse_tool_input(input)?;
        self.run(typed).await
    }

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        if input.node_kind.trim().is_empty() {
            return Err(ToolError::ValidationFailed {
                message: "node_kind must be non-empty".into(),
            });
        }
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
        let index = manifest
            .nodes
            .iter()
            .position(|existing| existing.kind == input.node_kind)
            .ok_or_else(|| ToolError::ValidationFailed {
                message: format!("node kind `{}` does not exist", input.node_kind),
            })?;
        if node.kind != input.node_kind
            && manifest
                .nodes
                .iter()
                .enumerate()
                .any(|(position, existing)| position != index && existing.kind == node.kind)
        {
            return Err(ToolError::ValidationFailed {
                message: format!("node kind `{}` already exists", node.kind),
            });
        }
        let previous_kind = manifest.nodes[index].kind.clone();
        manifest.nodes[index] = node;
        save_manifest(&target.workspace, &manifest).map_err(tool_error)?;

        Ok(ToolResult::success_json(json!({
            "previous_kind": previous_kind,
            "kind": manifest.nodes[index].kind,
            "node_count": manifest.nodes.len(),
        })))
    }
}
