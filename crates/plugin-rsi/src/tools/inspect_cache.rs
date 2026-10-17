//! Tool: inspect the active plugin materialization and development edits.

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;
use serde_json::json;

use super::PluginToolState;
use super::helpers::{resolve_target, tool_error};

#[tool(
    name = "engine_inspect_cache",
    description = "Inspect the immutable plugin actually materialized for the DAG and diff its \
                   manifest/scripts against the development workspace. Use this after editing \
                   scripts or before reinstalling to distinguish committed runtime state from \
                   pending development changes."
)]
pub(super) struct EngineInspectCacheInput {
    /// Development workspace path, such as `/plugins/dev/hello-world`.
    plugin_path: String,
}

pub(super) struct EngineInspectCacheTool {
    pub(super) state: PluginToolState,
}

#[async_trait]
impl ToolFunction for EngineInspectCacheTool {
    type Input = EngineInspectCacheInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let target = resolve_target(&self.state, &input.plugin_path)
            .await
            .map_err(tool_error)?;
        let infra = self.state.registry.infra().map_err(tool_error)?;
        let plugin_name = target.plugin_name.clone();
        let report = tokio::task::spawn_blocking(move || {
            infra.inspect_plugin_cache(&plugin_name).map_err(tool_error)
        })
        .await
        .map_err(|join| ToolError::ExecutionFailed {
            source: format!("plugin cache inspection task failed: {join}").into(),
        })??;

        let report = serde_json::to_value(&report).map_err(|error| ToolError::ExecutionFailed {
            source: Box::new(error),
        })?;
        Ok(ToolResult::success_json(json!({
            "plugin_name": report["plugin_name"],
            "active": report["active"],
            "active_manifest": report["active_manifest"],
            "development_manifest": report["development_manifest"],
            "scripts": report["scripts"],
            "stale_scripts": report["stale_scripts"],
        })))
    }
}
