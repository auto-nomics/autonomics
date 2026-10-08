//! Tools: create and fork environment development workspaces.

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;
use serde_json::json;

use crate::{RequestIntent, RequestRecord, RequestSource, RequestStatus};

use super::EnvToolState;
use super::{resolve_target, tool_error};

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
    /// Change intent; defaults to new_environment.
    intent: Option<String>,
    /// One-line summary of the requested environment.
    summary: String,
    /// Full demand, constraints, or requested capability.
    body: String,
}

pub(super) struct EnvironmentCreateTool {
    pub(super) state: EnvToolState,
}

#[derive(Debug)]
struct CreateOutput {
    request_id: String,
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
        let summary = input.summary.trim().to_string();
        let body = input.body.trim().to_string();
        if summary.is_empty() || body.is_empty() {
            return Err(ToolError::ValidationFailed {
                message: "summary and body must be non-empty".into(),
            });
        }
        let intent = parse_intent(input.intent.as_deref())?;
        let environments = self.state.registry.environments().map_err(tool_error)?;
        if environments.get(&input.base_environment_id).is_none() {
            return Err(ToolError::ValidationFailed {
                message: format!(
                    "environment `{}` is not approved; use plugin_environments_list",
                    input.base_environment_id
                ),
            });
        }
        let infra = self.state.registry.environment_infra().map_err(tool_error)?;
        let request = RequestRecord {
            id: String::new(),
            created_at: 0,
            source: RequestSource::Agent,
            intent,
            summary,
            body,
            plugin_name: Some(input.environment_id),
            evidence_ids: Vec::new(),
            status: RequestStatus::Open,
        };
        let base_environment_id = input.base_environment_id;
        let output = tokio::task::spawn_blocking(
            move || -> std::result::Result<CreateOutput, ToolError> {
                let operator = infra
                    .create_environment(request, &base_environment_id)
                    .map_err(tool_error)?;
                let manifest = operator.manifest();
                let request_id =
                    manifest
                        .lifecycle
                        .request_ids
                        .first()
                        .cloned()
                        .ok_or_else(|| ToolError::ExecutionFailed {
                            source: "environment creation completed without its request id".into(),
                        })?;
                Ok(CreateOutput {
                    request_id,
                    environment_id: operator.environment_id().to_string(),
                    environment_vfs_path: operator
                        .development_vfs_path()
                        .map_err(tool_error)?,
                    base_environment_id,
                    base_reference: manifest.base.reference.clone(),
                    interpreters: manifest.interpreters.clone(),
                })
            },
        )
        .await
        .map_err(|join| ToolError::ExecutionFailed {
            source: format!("environment create task failed: {join}").into(),
        })??;

        Ok(ToolResult::success_json(json!({
            "request_id": output.request_id,
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
    description = "Fork an existing environment development workspace under a new environment id. \
                   Edit only the returned target path; never copy workspace files with vfs cp."
)]
pub(super) struct EnvironmentForkInput {
    /// Source workspace path, such as `/environments/dev/bioconductor-extra`.
    source_environment_path: String,
    /// New environment id in lowercase kebab-case.
    environment_id: String,
    /// Change intent; defaults to new_environment.
    intent: Option<String>,
    /// One-line summary of the requested improvement.
    summary: String,
    /// Full demand, constraints, or requested behavior.
    body: String,
}

pub(super) struct EnvironmentForkTool {
    pub(super) state: EnvToolState,
}

#[derive(Debug)]
struct ForkOutput {
    request_id: String,
    source_environment: String,
    target_environment: String,
    target_vfs_path: String,
}

#[async_trait]
impl ToolFunction for EnvironmentForkTool {
    type Input = EnvironmentForkInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let summary = input.summary.trim().to_string();
        let body = input.body.trim().to_string();
        if summary.is_empty() || body.is_empty() {
            return Err(ToolError::ValidationFailed {
                message: "summary and body must be non-empty".into(),
            });
        }
        let intent = parse_intent(input.intent.as_deref())?;
        let source = resolve_target(&self.state, &input.source_environment_path)
            .await
            .map_err(tool_error)?;
        let infra = self.state.registry.environment_infra().map_err(tool_error)?;
        let request = RequestRecord {
            id: String::new(),
            created_at: 0,
            source: RequestSource::Agent,
            intent,
            summary,
            body,
            plugin_name: Some(input.environment_id),
            evidence_ids: Vec::new(),
            status: RequestStatus::Open,
        };
        let source_environment = source.environment_id;
        let output = tokio::task::spawn_blocking(
            move || -> std::result::Result<ForkOutput, ToolError> {
                let operator = infra
                    .fork_environment(&source_environment, request)
                    .map_err(tool_error)?;
                let manifest = operator.manifest();
                let request_id =
                    manifest
                        .lifecycle
                        .request_ids
                        .first()
                        .cloned()
                        .ok_or_else(|| ToolError::ExecutionFailed {
                            source: "environment fork completed without its request id".into(),
                        })?;
                Ok(ForkOutput {
                    request_id,
                    source_environment,
                    target_environment: operator.environment_id().to_string(),
                    target_vfs_path: operator
                        .development_vfs_path()
                        .map_err(tool_error)?,
                })
            },
        )
        .await
        .map_err(|join| ToolError::ExecutionFailed {
            source: format!("environment fork task failed: {join}").into(),
        })??;

        Ok(ToolResult::success_json(json!({
            "request_id": output.request_id,
            "source_environment": output.source_environment,
            "target_environment": output.target_environment,
            "target_vfs_path": output.target_vfs_path,
        })))
    }
}

fn parse_intent(value: Option<&str>) -> Result<RequestIntent, ToolError> {
    match value.map(str::trim) {
        None | Some("") | Some("new_environment") => Ok(RequestIntent::NewEnvironment),
        Some("optimize_node") => Ok(RequestIntent::OptimizeNode),
        Some("fix_node") => Ok(RequestIntent::FixNode),
        Some(other) => Err(ToolError::ValidationFailed {
            message: format!(
                "unknown intent {other:?}; use new_environment, optimize_node, or fix_node"
            ),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_environment_lifecycle_intents() {
        assert_eq!(
            parse_intent(Some("new_environment")).unwrap(),
            RequestIntent::NewEnvironment
        );
        assert_eq!(parse_intent(None).unwrap(), RequestIntent::NewEnvironment);
        assert!(parse_intent(Some("release")).is_err());
    }
}
