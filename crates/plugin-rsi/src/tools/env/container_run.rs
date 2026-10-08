//! Tool: run one argv command inside an environment's built image.

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;
use container_runtime::{
    ContainerNetwork, ContainerRunRequest, DEFAULT_CONTAINER_WORKDIR, GpuRequest, PullPolicy,
    trusted_workspace_ref, unique_container_name,
};
use serde_json::json;

use crate::env_store::load_manifest;

use super::EnvToolState;
use super::{resolve_target, tool_error};

const DEFAULT_TOOL_TIMEOUT_SECS: u64 = 900;

#[tool(
    name = "environment_container_run",
    description = "Run one argv command in the environment's last built image (or its base when none was \
                   built) with the workspace mounted at /work, for environment debugging only. Do not use \
                   this tool for real data analysis."
)]
pub(super) struct EnvironmentContainerRunInput {
    /// Environment workspace path, such as `/environments/dev/bioconductor-extra`.
    environment_path: String,
    /// Executable and arguments. No shell string is accepted.
    argv: Vec<String>,
    /// Timeout in seconds; defaults to 120 and is capped at 600.
    timeout_secs: Option<u64>,
}

pub(super) struct EnvironmentContainerRunTool {
    pub(super) state: EnvToolState,
}

#[async_trait]
impl ToolFunction for EnvironmentContainerRunTool {
    type Input = EnvironmentContainerRunInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        if input.argv.is_empty() || input.argv.iter().any(|arg| arg.contains('\0')) {
            return Err(ToolError::ValidationFailed {
                message: "argv must be a nonempty NUL-free array".into(),
            });
        }
        let timeout_secs = input.timeout_secs.unwrap_or(120).clamp(1, 600);
        let target = resolve_target(&self.state, &input.environment_path)
            .await
            .map_err(tool_error)?;
        let manifest = load_manifest(&target.workspace).map_err(tool_error)?;
        // Prefer the last locally built image; fall back to the declared base.
        let image = manifest
            .lifecycle
            .local_reference
            .clone()
            .unwrap_or_else(|| manifest.base.reference.clone());
        let runtime = self.state.registry.runtime().map_err(tool_error)?;
        let workspace_ref =
            trusted_workspace_ref(target.workspace.path(), DEFAULT_CONTAINER_WORKDIR)
                .map_err(tool_error)?;
        let request = ContainerRunRequest {
            image,
            command: input.argv,
            workspace: workspace_ref,
            env: vec![
                (
                    "AUTONOMICS_ENVIRONMENT_ID".into(),
                    target.environment_id.clone(),
                ),
                (
                    "AUTONOMICS_ENVIRONMENT_VFS_PATH".into(),
                    target.virtual_path.clone(),
                ),
            ],
            panels: Vec::new(),
            input_mounts: Vec::new(),
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
