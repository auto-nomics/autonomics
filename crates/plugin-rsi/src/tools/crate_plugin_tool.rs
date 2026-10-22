//! Tool: create an empty plugin development workspace.

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;
use serde_json::json;

use super::PluginToolState;
use super::helpers::tool_error;

pub(super) struct PluginCreateTool {
    pub(super) state: PluginToolState,
}

#[tool(
    name = "plugin_create",
    description = "Create a new empty Draft plugin workspace bound to an approved environment. \
                   Add nodes, scripts, and documentation before calling plugin_install."
)]
pub(super) struct PluginCreateInput {
    /// New plugin name in lowercase kebab-case.
    plugin_name: String,
    /// Approved environment id from plugin_environments_list.
    environment_id: String,
    /// Why this plugin is being developed.
    purpose: String,
}

#[derive(Debug)]
struct CreateOutput {
    plugin_name: String,
    plugin_vfs_path: String,
    environment_id: String,
    environment_reference: String,
    interpreters: Vec<String>,
}

#[async_trait]
impl ToolFunction for PluginCreateTool {
    type Input = PluginCreateInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let plugin_name = input.plugin_name.trim().to_string();
        let environment_id = input.environment_id.trim().to_string();
        let purpose = input.purpose.trim().to_string();
        if plugin_name.is_empty() || environment_id.is_empty() || purpose.is_empty() {
            return Err(ToolError::ValidationFailed {
                message: "plugin_name, environment_id, and purpose must be non-empty".into(),
            });
        }

        let catalog = self.state.registry.environments().map_err(tool_error)?;
        let environment = catalog.get(&environment_id).ok_or_else(|| {
            ToolError::ValidationFailed {
                message: format!(
                    "environment `{environment_id}` is not approved; use plugin_environments_list"
                ),
            }
        })?;
        let interpreters = environment.interpreters.clone();

        let infra = self.state.registry.infra().map_err(tool_error)?;
        let manifest_lock = self.state.registry.manifest_lock(&plugin_name);
        let _guard = manifest_lock.lock().await;
        let output = tokio::task::spawn_blocking(move || -> Result<CreateOutput, ToolError> {
            let operator = infra
                .create_plugin(&plugin_name, &environment_id, &purpose)
                .map_err(tool_error)?;
            let environment_reference = operator.manifest().image.reference.as_str().to_string();
            Ok(CreateOutput {
                plugin_vfs_path: operator.development_vfs_path().map_err(tool_error)?,
                plugin_name: operator.plugin_name().to_string(),
                environment_id,
                environment_reference,
                interpreters,
            })
        })
        .await
        .map_err(|join| ToolError::ExecutionFailed {
            source: format!("plugin create task failed: {join}").into(),
        })??;

        Ok(ToolResult::success_json(json!({
            "plugin_name": output.plugin_name,
            "plugin_vfs_path": output.plugin_vfs_path,
            "environment_id": output.environment_id,
            "environment_reference": output.environment_reference,
            "interpreters": output.interpreters,
            "status": "draft",
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentik_sdk::types::ToolInput;
    use serde_json::to_value;

    #[test]
    fn input_schema_exposes_create_contract() {
        let definition = PluginCreateInput::definition();
        let schema = to_value(&definition.input_schema).unwrap();

        assert_eq!(definition.name, "plugin_create");
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["properties"]["plugin_name"]["type"], "string");
        assert_eq!(schema["properties"]["environment_id"]["type"], "string");
        assert_eq!(schema["properties"]["purpose"]["type"], "string");
        assert_eq!(
            schema["required"],
            serde_json::json!(["plugin_name", "environment_id", "purpose"])
        );
    }
}
