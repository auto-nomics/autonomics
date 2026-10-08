//! Tool: run one argv command in the plugin environment with the addressed
//! VFS workspace mounted at /work.

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;
use container_runtime::{
    ContainerNetwork, ContainerRunRequest, DEFAULT_CONTAINER_WORKDIR, GpuRequest, PullPolicy,
    trusted_workspace_ref, unique_container_name,
};
use serde_json::json;

use super::PluginToolState;
use super::helpers::{resolve_target, tool_error};
use super::manifest::load_manifest;

const DEFAULT_TOOL_TIMEOUT_SECS: u64 = 900;

#[tool(
    name = "plugin_container_run",
    description = "Run one argv command in the plugin environment for node/plugin debugging only. \
                  Do not use this tool for real data analysis; analyze real data through the \
                  intended plugin nodes and supported data-analysis tools."
)]
pub(super) struct PluginContainerRunInput {
    /// Plugin workspace path, such as `/plugins/dev/hello-world`.
    plugin_path: String,
    /// Executable and arguments. No shell string is accepted.
    argv: Vec<String>,
    /// Timeout in seconds; defaults to 120 and is capped at 600.
    timeout_secs: Option<u64>,
}

pub(super) struct PluginContainerRunTool {
    pub(super) state: PluginToolState,
}

#[async_trait]
impl ToolFunction for PluginContainerRunTool {
    type Input = PluginContainerRunInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        if input.argv.is_empty() || input.argv.iter().any(|arg| arg.contains('\0')) {
            return Err(ToolError::ValidationFailed {
                message: "argv must be a nonempty NUL-free array".into(),
            });
        }
        let timeout_secs = input.timeout_secs.unwrap_or(120).clamp(1, 600);
        let target = resolve_target(&self.state, &input.plugin_path)
            .await
            .map_err(tool_error)?;
        let manifest = load_manifest(&target).await.map_err(tool_error)?;
        let runtime = self.state.registry.runtime().map_err(tool_error)?;
        let workspace_ref =
            trusted_workspace_ref(target.workspace.path(), DEFAULT_CONTAINER_WORKDIR)
                .map_err(tool_error)?;
        let request = ContainerRunRequest {
            image: manifest.image.reference.as_str().to_string(),
            command: input.argv,
            workspace: workspace_ref,
            env: vec![
                ("AUTONOMICS_PLUGIN_NAME".into(), target.plugin_name.clone()),
                (
                    "AUTONOMICS_PLUGIN_VFS_PATH".into(),
                    target.virtual_path.clone(),
                ),
            ],
            panels: Vec::new(),
            network: ContainerNetwork::Isolated,
            read_only_rootfs: true,
            pull_policy: PullPolicy::Missing,
            cpus: Some(2.0),
            memory: Some("1g".into()),
            pids_limit: Some(256),
            shm_size: Some("256m".into()),
            gpus: GpuRequest::None,
            user: None,
            timeout_secs,
            name: unique_container_name(),
        };
        let output = runtime.run(request).await.map_err(tool_error)?;
        Ok(ToolResult::success_json(json!({
            "exit_code": output.exit_code,
            "stdout": output.stdout,
            "stderr": output.stderr,
        })))
    }

    fn timeout_seconds(&self) -> u64 {
        DEFAULT_TOOL_TIMEOUT_SECS
    }
}
