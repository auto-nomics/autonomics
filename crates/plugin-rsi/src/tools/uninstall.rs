//! Tool: uninstall one plugin's active runtime source.

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;
use serde_json::json;

use super::PluginToolState;
use super::helpers::{resolve_target, tool_error};

#[tool(
    name = "plugin_uninstall",
    description = "Uninstall a plugin from the live DAG registry and remove its active runtime source. \
                   The development workspace and immutable snapshots are retained for audit and later reinstall."
)]
pub(super) struct PluginUninstallInput {
    /// Installed workspace path, such as `/plugins/dev/hello-world`.
    plugin_path: String,
}

pub(super) struct PluginUninstallTool {
    pub(super) state: PluginToolState,
}

#[async_trait]
impl ToolFunction for PluginUninstallTool {
    type Input = PluginUninstallInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let target = resolve_target(&self.state, &input.plugin_path)
            .await
            .map_err(tool_error)?;
        let infra = self.state.registry.infra().map_err(tool_error)?;
        let plugin_name = target.plugin_name.clone();
        let manifest_lock = self.state.registry.manifest_lock(&plugin_name);
        let _guard = manifest_lock.lock().await;
        let source =
            tokio::task::spawn_blocking(move || infra.uninstall(&plugin_name).map_err(tool_error))
                .await
                .map_err(|join| ToolError::ExecutionFailed {
                    source: format!("plugin uninstall task failed: {join}").into(),
                })??;

        let (source_kind, remote, commit, digest) = match &source {
            crate::InstalledPluginSource::Git(source) => (
                "github",
                Some(source.remote.clone()),
                source.commit.clone(),
                None,
            ),
            crate::InstalledPluginSource::Local(source) => (
                "local_snapshot",
                None,
                source.commit.clone(),
                Some(source.digest.clone()),
            ),
        };
        Ok(ToolResult::success_json(json!({
            "uninstalled": true,
            "plugin_name": target.plugin_name,
            "plugin_vfs_path": target.virtual_path,
            "source_kind": source_kind,
            "remote": remote,
            "source_commit": commit,
            "source_digest": digest,
            "development_workspace_retained": true,
            "snapshots_retained": true,
        })))
    }
}
