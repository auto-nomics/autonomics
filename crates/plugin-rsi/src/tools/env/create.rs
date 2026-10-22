//! Tools: create and fork environment development workspaces.

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;
use serde_json::json;

use super::EnvToolState;
use super::{environment_manifest_lock, resolve_target, tool_error};

#[tool(
    name = "environment_create",
    description = "Create a new environment image development workspace bound to an approved base environment. \
                   The base_environment_id must come from plugin_environments_list. Edit the returned \
                   workspace's Containerfile through the VFS."
)]
pub(super) struct EnvironmentCreateInput {
    /// New environment id in lowercase kebab-case.
    environment_id: String,
    /// Approved base environment id from plugin_environments_list.
    base_environment_id: String,
    /// Why this environment is being developed.
    purpose: String,
}

pub(super) struct EnvironmentCreateTool {
    pub(super) state: EnvToolState,
}

#[derive(Debug)]
struct CreateOutput {
    environment_id: String,
    environment_vfs_path: String,
    base_environment_id: String,
    base_reference: String,
    interpreters: Vec<String>,
}

#[async_trait]
impl ToolFunction for EnvironmentCreateTool {
    type Input = EnvironmentCreateInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let environment_id = input.environment_id.trim().to_string();
        let base_environment_id = input.base_environment_id.trim().to_string();
        let purpose = input.purpose.trim().to_string();
        if environment_id.is_empty() || base_environment_id.is_empty() || purpose.is_empty() {
            return Err(ToolError::ValidationFailed {
                message: "environment_id, base_environment_id, and purpose must be non-empty"
                    .into(),
            });
        }

        let environments = self.state.registry.environments().map_err(tool_error)?;
        if environments.get(&base_environment_id).is_none() {
            return Err(ToolError::ValidationFailed {
                message: format!(
                    "environment `{base_environment_id}` is not approved; use plugin_environments_list"
                ),
            });
        }
        let infra = self
            .state
            .registry
            .environment_infra()
            .map_err(tool_error)?;
        let manifest_lock = environment_manifest_lock(&self.state.registry, &environment_id);
        let _guard = manifest_lock.lock().await;
        let output = tokio::task::spawn_blocking(move || -> Result<CreateOutput, ToolError> {
            let operator = infra
                .create_environment(&environment_id, &base_environment_id, &purpose)
                .map_err(tool_error)?;
            let manifest = operator.manifest();
            Ok(CreateOutput {
                environment_vfs_path: operator.development_vfs_path().map_err(tool_error)?,
                environment_id: operator.environment_id().to_string(),
                base_environment_id,
                base_reference: manifest.base.reference.clone(),
                interpreters: manifest.interpreters.clone(),
            })
        })
        .await
        .map_err(|join| ToolError::ExecutionFailed {
            source: format!("environment create task failed: {join}").into(),
        })??;

        Ok(ToolResult::success_json(json!({
            "environment_id": output.environment_id,
            "environment_vfs_path": output.environment_vfs_path,
            "base_environment_id": output.base_environment_id,
            "base_reference": output.base_reference,
            "interpreters": output.interpreters,
        })))
    }
}

#[tool(
    name = "environment_fork",
    description = "Fork an existing environment workspace under a new id. Edit only the returned target path."
)]
pub(super) struct EnvironmentForkInput {
    /// Existing environment workspace path, such as `/environments/dev/python`.
    source_environment_path: String,
    /// New environment id in lowercase kebab-case.
    environment_id: String,
    /// Why this environment fork is being developed.
    purpose: String,
}

pub(super) struct EnvironmentForkTool {
    pub(super) state: EnvToolState,
}

#[derive(Debug)]
struct ForkOutput {
    source_environment: String,
    environment_id: String,
    environment_vfs_path: String,
}

#[async_trait]
impl ToolFunction for EnvironmentForkTool {
    type Input = EnvironmentForkInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let environment_id = input.environment_id.trim().to_string();
        let purpose = input.purpose.trim().to_string();
        if environment_id.is_empty() || purpose.is_empty() {
            return Err(ToolError::ValidationFailed {
                message: "environment_id and purpose must be non-empty".into(),
            });
        }
        let source = resolve_target(&self.state, &input.source_environment_path)
            .await
            .map_err(tool_error)?;
        let infra = self
            .state
            .registry
            .environment_infra()
            .map_err(tool_error)?;
        let manifest_lock = environment_manifest_lock(&self.state.registry, &environment_id);
        let _guard = manifest_lock.lock().await;
        let source_environment = source.environment_id.clone();
        let output = tokio::task::spawn_blocking(move || -> Result<ForkOutput, ToolError> {
            let operator = infra
                .fork_environment(&source_environment, &environment_id, &purpose)
                .map_err(tool_error)?;
            Ok(ForkOutput {
                source_environment: source_environment.clone(),
                environment_vfs_path: operator.development_vfs_path().map_err(tool_error)?,
                environment_id: operator.environment_id().to_string(),
            })
        })
        .await
        .map_err(|join| ToolError::ExecutionFailed {
            source: format!("environment fork task failed: {join}").into(),
        })??;

        Ok(ToolResult::success_json(json!({
            "source_environment": output.source_environment,
            "environment_id": output.environment_id,
            "environment_vfs_path": output.environment_vfs_path,
        })))
    }
}
