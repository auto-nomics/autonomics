//! Tool: fork an installed reference plugin into a new development workspace.

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;
use serde_json::json;

use super::PluginToolState;
use super::helpers::{resolve_target, tool_error};

#[tool(
    name = "plugin_fork",
    description = "Fork an installed reference plugin into a new development workspace. \
                   Edit only the returned target path; never copy active plugin files with vfs cp."
)]
pub(super) struct PluginForkInput {
    /// Installed reference workspace path, such as `/plugins/dev/hello-world`.
    reference_plugin_path: String,
    /// New plugin name in lowercase kebab-case.
    plugin_name: String,
    /// Why this fork is being developed.
    purpose: String,
}

pub(super) struct PluginForkTool {
    pub(super) state: PluginToolState,
}

#[derive(Debug)]
struct ForkOutput {
    reference_plugin: String,
    target_plugin: String,
    target_vfs_path: String,
    source_commit: String,
}

#[async_trait]
impl ToolFunction for PluginForkTool {
    type Input = PluginForkInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let plugin_name = input.plugin_name.trim().to_string();
        let purpose = input.purpose.trim().to_string();
        if plugin_name.is_empty() || purpose.is_empty() {
            return Err(ToolError::ValidationFailed {
                message: "plugin_name and purpose must be non-empty".into(),
            });
        }

        let reference = resolve_target(&self.state, &input.reference_plugin_path)
            .await
            .map_err(tool_error)?;
        let infra = self.state.registry.infra().map_err(tool_error)?;
        let manifest_lock = self.state.registry.manifest_lock(&plugin_name);
        let _guard = manifest_lock.lock().await;
        let reference_plugin = reference.plugin_name.clone();
        let output = tokio::task::spawn_blocking(move || -> Result<ForkOutput, ToolError> {
            let operator = infra
                .fork_plugin(&reference_plugin, &plugin_name, &purpose)
                .map_err(tool_error)?;
            let source_commit = operator
                .manifest()
                .lifecycle
                .source_commit
                .clone()
                .unwrap_or_default();
            Ok(ForkOutput {
                reference_plugin: reference_plugin.clone(),
                target_vfs_path: operator.development_vfs_path().map_err(tool_error)?,
                target_plugin: operator.plugin_name().to_string(),
                source_commit,
            })
        })
        .await
        .map_err(|join| ToolError::ExecutionFailed {
            source: format!("plugin fork task failed: {join}").into(),
        })??;

        Ok(ToolResult::success_json(json!({
            "reference_plugin": output.reference_plugin,
            "target_plugin": output.target_plugin,
            "target_vfs_path": output.target_vfs_path,
            "source_commit": output.source_commit,
        })))
    }
}
