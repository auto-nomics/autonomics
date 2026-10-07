//! Tool: bind an editable plugin workspace to an approved environment image.

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;
use serde_json::json;

use super::PluginToolState;
use super::helpers::{resolve_target, tool_error};

#[tool(
    name = "plugin_environment_bind",
    description = "Bind an editable plugin development workspace to an approved environment image. \
                   The environment id must come from plugin_environments_list."
)]
pub(super) struct PluginEnvironmentBindInput {
    /// Developed workspace path, such as `/plugins/dev/hello-world`.
    plugin_path: String,
    /// Approved environment id from the host catalog.
    environment_id: String,
}

pub(super) struct PluginEnvironmentBindTool {
    pub(super) state: PluginToolState,
}

#[async_trait]
impl ToolFunction for PluginEnvironmentBindTool {
    type Input = PluginEnvironmentBindInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let environment_id = input.environment_id.trim().to_string();
        if environment_id.is_empty() {
            return Err(ToolError::ValidationFailed {
                message: "environment_id must be non-empty".into(),
            });
        }
        let catalog = self.state.registry.environments().map_err(tool_error)?;
        let environment =
            catalog
                .get(&environment_id)
                .ok_or_else(|| ToolError::ValidationFailed {
                    message: format!("environment `{environment_id}` is not approved"),
                })?;
        let target = resolve_target(&self.state, &input.plugin_path)
            .await
            .map_err(tool_error)?;
        let infra = self.state.registry.infra().map_err(tool_error)?;
        let plugin_name = target.plugin_name;
        let task_plugin_name = plugin_name.clone();
        let task_environment_id = environment_id.clone();
        let task_catalog = catalog.clone();
        let manifest_lock = self.state.registry.manifest_lock(&plugin_name);
        let _guard = manifest_lock.lock().await;
        let output =
            tokio::task::spawn_blocking(move || -> std::result::Result<String, ToolError> {
                let store = infra.store();
                let mut operator = store
                    .develop(&task_plugin_name)
                    .map_err(tool_error)?
                    .ok_or_else(|| ToolError::ValidationFailed {
                        message: format!("plugin `{task_plugin_name}` does not exist"),
                    })?;
                operator
                    .bind_environment(&task_environment_id, &task_catalog)
                    .map_err(tool_error)?;
                Ok(operator.manifest().image.reference.as_str().to_string())
            })
            .await
            .map_err(|join| ToolError::ExecutionFailed {
                source: format!("plugin environment bind task failed: {join}").into(),
            })??;

        Ok(ToolResult::success_json(json!({
            "plugin_name": plugin_name,
            "plugin_vfs_path": target.virtual_path,
            "environment_id": environment_id,
            "environment_reference": output,
            "interpreters": environment.interpreters,
        })))
    }
}
