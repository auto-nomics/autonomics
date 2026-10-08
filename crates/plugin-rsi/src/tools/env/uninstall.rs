//! Tool: remove one environment's active catalog entry.

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;
use serde_json::json;

use super::EnvToolState;
use super::{environment_manifest_lock, resolve_target, tool_error};

#[tool(
    name = "environment_uninstall",
    description = "Remove an environment from the catalog so plugins can no longer bind it. \
                   The development workspace and its history are retained for audit and later reinstall."
)]
pub(super) struct EnvironmentUninstallInput {
    /// Environment workspace path, such as `/environments/dev/bioconductor-extra`.
    environment_path: String,
}

pub(super) struct EnvironmentUninstallTool {
    pub(super) state: EnvToolState,
}

#[async_trait]
impl ToolFunction for EnvironmentUninstallTool {
    type Input = EnvironmentUninstallInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let target = resolve_target(&self.state, &input.environment_path)
            .await
            .map_err(tool_error)?;
        let infra = self.state.registry.environment_infra().map_err(tool_error)?;
        let manifest_lock = environment_manifest_lock(&self.state.registry, &target.environment_id);
        let _guard = manifest_lock.lock().await;
        let environment_id = target.environment_id.clone();
        let removed = tokio::task::spawn_blocking(
            move || infra.uninstall_environment(&environment_id).map_err(tool_error),
        )
        .await
        .map_err(|join| ToolError::ExecutionFailed {
            source: format!("environment uninstall task failed: {join}").into(),
        })??;

        Ok(ToolResult::success_json(json!({
            "uninstalled": true,
            "environment_id": target.environment_id,
            "environment_vfs_path": target.virtual_path,
            "removed_reference": removed.reference,
            "removed_interpreters": removed.interpreters,
            "development_workspace_retained": true,
        })))
    }
}
