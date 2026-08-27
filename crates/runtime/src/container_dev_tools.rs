//! Agent-facing tools for persistent container development workspaces.

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use container_runtime::{ContainerExecutionInfra, ContainerRuntimeError};
use container_runtime::{DevExecRequest, DevImageBuildRequest, DevWorkspaceCreate};
use serde_json::json;

pub const DEFAULT_DEV_IMAGE: &str = "docker.io/library/autonomics-codex-agent:latest";
const DEFAULT_CREATE_TIMEOUT_SECS: u64 = 300;
const DEFAULT_EXEC_TIMEOUT_SECS: u64 = 120;
const DEFAULT_BUILD_TIMEOUT_SECS: u64 = 1800;

fn execution_failed(error: ContainerRuntimeError) -> ToolError {
    ToolError::ExecutionFailed {
        source: Box::new(error),
    }
}

#[tool(
    name = "container_workspace_create",
    description = "Create or attach to a persistent k3s development workspace. The source workspace survives Pod deletion. This tool requires the k3s backend."
)]
pub struct ContainerWorkspaceCreateInput {
    #[desc = "Stable workspace id: 1-48 lowercase letters, digits, or dashes"]
    pub workspace_id: String,
    #[desc = "OCI image. Defaults to the locally built autonomics-codex-agent image."]
    pub image: Option<String>,
    #[desc = "KEY=VALUE environment variables for the long-running Pod."]
    pub env: Option<Vec<String>>,
    #[desc = "Network profile: egress (default), cluster, or isolated."]
    pub network: Option<String>,
    #[desc = "CPU request/limit in cores."]
    pub cpus: Option<f64>,
    #[desc = "Memory request/limit, for example 4Gi."]
    pub memory: Option<String>,
    #[desc = "Seconds to wait for the Pod to become ready. Default 300."]
    pub timeout_secs: Option<u64>,
}

pub struct ContainerWorkspaceCreateTool {
    infra: Arc<ContainerExecutionInfra>,
}

#[tool(
    name = "container_exec",
    description = "Execute one captured command in a persistent development workspace. Use argv for exact argument passing, or command for a POSIX shell command."
)]
pub struct ContainerExecInput {
    #[desc = "Development workspace id"]
    pub workspace_id: String,
    #[desc = "Exact argv, for example [\"git\",\"status\"]."]
    pub argv: Option<Vec<String>>,
    #[desc = "POSIX shell command. Ignored when argv is provided."]
    pub command: Option<String>,
    #[desc = "Optional absolute working directory inside the container."]
    pub workdir: Option<String>,
    #[desc = "Seconds before the exec is aborted. Default 120."]
    pub timeout_secs: Option<u64>,
}

pub struct ContainerExecTool {
    infra: Arc<ContainerExecutionInfra>,
}

#[tool(
    name = "container_workspace_status",
    description = "List development workspaces, or inspect one workspace when workspace_id is set."
)]
pub struct ContainerWorkspaceStatusInput {
    #[desc = "Optional workspace id."]
    pub workspace_id: Option<String>,
}

pub struct ContainerWorkspaceStatusTool {
    infra: Arc<ContainerExecutionInfra>,
}

#[tool(
    name = "container_workspace_stop",
    description = "Stop a development Pod while retaining its persistent PVC workspace."
)]
pub struct ContainerWorkspaceStopInput {
    #[desc = "Workspace id to stop."]
    pub workspace_id: String,
}

pub struct ContainerWorkspaceStopTool {
    infra: Arc<ContainerExecutionInfra>,
}

#[tool(
    name = "container_image_build",
    description = "Build an OCI image tar from a development workspace. The generated image uses the original image as its base and copies the workspace to /workspace."
)]
pub struct ContainerImageBuildInput {
    #[desc = "Workspace id to package."]
    pub workspace_id: String,
    #[desc = "Optional base image override. Defaults to the image used to create the workspace."]
    pub base_image: Option<String>,
    #[desc = "Seconds before the build is aborted. Default 1800."]
    pub timeout_secs: Option<u64>,
}

pub struct ContainerImageBuildTool {
    infra: Arc<ContainerExecutionInfra>,
}

fn k3s_backend(
    infra: &ContainerExecutionInfra,
) -> Result<&container_runtime::K3sRuntime, ToolError> {
    infra
        .k3s
        .as_deref()
        .ok_or_else(|| ToolError::ValidationFailed {
            message: "persistent development workspaces require the k3s backend".into(),
        })
}

#[async_trait]
impl ToolFunction for ContainerWorkspaceCreateTool {
    type Input = ContainerWorkspaceCreateInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let runtime = k3s_backend(&self.infra)?;
        let status = runtime
            .create_dev_workspace(DevWorkspaceCreate {
                id: input.workspace_id,
                image: input.image.unwrap_or_else(|| DEFAULT_DEV_IMAGE.to_string()),
                env: parse_env(input.env.as_deref())?,
                network: input.network.unwrap_or_else(|| "egress".into()),
                cpus: input.cpus,
                memory: input.memory,
                timeout_secs: input.timeout_secs.unwrap_or(DEFAULT_CREATE_TIMEOUT_SECS),
            })
            .await
            .map_err(execution_failed)?;
        Ok(ToolResult::success_json(json!({ "status": status })))
    }
}

#[async_trait]
impl ToolFunction for ContainerExecTool {
    type Input = ContainerExecInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let command = if let Some(argv) = input.argv {
            if argv.is_empty() {
                return Err("argv cannot be empty".into());
            }
            argv
        } else if let Some(command) = input.command {
            if command.trim().is_empty() {
                return Err("command cannot be empty".into());
            }
            vec!["/bin/sh".into(), "-lc".into(), command]
        } else {
            return Err("one of argv or command is required".into());
        };
        let runtime = k3s_backend(&self.infra)?;
        let result = runtime
            .exec_in_dev_workspace(DevExecRequest {
                workspace_id: input.workspace_id,
                command,
                workdir: input.workdir,
                timeout_secs: input.timeout_secs.unwrap_or(DEFAULT_EXEC_TIMEOUT_SECS),
            })
            .await
            .map_err(execution_failed)?;
        Ok(ToolResult::success_json(json!({ "result": result })))
    }
}

#[async_trait]
impl ToolFunction for ContainerWorkspaceStatusTool {
    type Input = ContainerWorkspaceStatusInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let statuses = if let Some(id) = input.workspace_id {
            vec![
                k3s_backend(&self.infra)?
                    .dev_workspace_status(&id)
                    .await
                    .map_err(execution_failed)?,
            ]
        } else {
            k3s_backend(&self.infra)?
                .list_dev_workspaces()
                .await
                .map_err(execution_failed)?
        };
        Ok(ToolResult::success_json(json!({ "workspaces": statuses })))
    }
}

#[async_trait]
impl ToolFunction for ContainerWorkspaceStopTool {
    type Input = ContainerWorkspaceStopInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        k3s_backend(&self.infra)?
            .stop_dev_workspace(&input.workspace_id)
            .await
            .map_err(execution_failed)?;
        Ok(ToolResult::success_json(json!({
            "workspace_id": input.workspace_id,
            "stopped": true,
            "workspace_retained": true
        })))
    }
}

#[async_trait]
impl ToolFunction for ContainerImageBuildTool {
    type Input = ContainerImageBuildInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let runtime = k3s_backend(&self.infra)?;
        let result = runtime
            .build_dev_workspace_image(DevImageBuildRequest {
                workspace_id: input.workspace_id,
                base_image: input.base_image,
                builder_image: runtime.config().image_builder.clone(),
                timeout_secs: input.timeout_secs.unwrap_or(DEFAULT_BUILD_TIMEOUT_SECS),
            })
            .await
            .map_err(execution_failed)?;
        Ok(ToolResult::success_json(json!({ "result": result })))
    }
}

pub fn container_dev_registrations(infra: Arc<ContainerExecutionInfra>) -> Vec<ToolRegistration> {
    vec![
        ToolRegistration::from(ContainerWorkspaceCreateTool {
            infra: Arc::clone(&infra),
        }),
        ToolRegistration::from(ContainerExecTool {
            infra: Arc::clone(&infra),
        }),
        ToolRegistration::from(ContainerWorkspaceStatusTool {
            infra: Arc::clone(&infra),
        }),
        ToolRegistration::from(ContainerWorkspaceStopTool {
            infra: Arc::clone(&infra),
        }),
        ToolRegistration::from(ContainerImageBuildTool { infra }),
    ]
}

fn parse_env(values: Option<&[String]>) -> Result<Vec<(String, String)>, ToolError> {
    let Some(values) = values else {
        return Ok(Vec::new());
    };
    values
        .iter()
        .map(|value| {
            let (name, value) =
                value
                    .split_once('=')
                    .ok_or_else(|| ToolError::ValidationFailed {
                        message: format!("environment entry must be KEY=VALUE, got `{value}`"),
                    })?;
            if name.is_empty() || name.contains('\0') || value.contains('\0') {
                return Err(ToolError::ValidationFailed {
                    message: format!("invalid environment entry `{name}={value}`"),
                });
            }
            Ok((name.to_string(), value.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_five_container_development_tools() {
        let infra = Arc::new(ContainerExecutionInfra::from_config(test_config()));
        let registrations = container_dev_registrations(infra);
        let names: Vec<&str> = registrations
            .iter()
            .map(|registration| registration.definition.name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "container_workspace_create",
                "container_exec",
                "container_workspace_status",
                "container_workspace_stop",
                "container_image_build"
            ]
        );
    }

    #[test]
    fn parses_environment_entries() {
        assert_eq!(
            parse_env(Some(&["A=1".into(), "B=x=y".into()])).unwrap(),
            vec![("A".into(), "1".into()), ("B".into(), "x=y".into())]
        );
        assert!(parse_env(Some(&["INVALID".into()])).is_err());
    }

    fn test_config() -> container_runtime::K3sConfig {
        container_runtime::K3sConfig {
            namespace: "autonomics".into(),
            context: None,
            workspace_pvc: "workspace".into(),
            workspace_root: "/tmp/autonomics-workspace".into(),
            panel_pvc: "panels".into(),
            panel_cache_root: "/tmp/autonomics-panels".into(),
            panel_pvc_prefix: String::new(),
            service_account: None,
            poll_interval_ms: 10,
            image_builder: "kaniko.example/executor".into(),
        }
    }
}
