//! Tool: fork an installed reference plugin into a new development workspace.

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;
use serde_json::json;

use crate::{RequestIntent, RequestRecord, RequestSource, RequestStatus};

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
    /// Change intent: new_node, optimize_node, fix_node, or clarify_contract.
    intent: Option<String>,
    /// One-line summary of the requested improvement.
    summary: String,
    /// Full demand, constraints, observed failure, or requested behavior.
    body: String,
}

pub(super) struct PluginForkTool {
    pub(super) state: PluginToolState,
}

#[derive(Debug)]
struct ForkOutput {
    request_id: String,
    reference_plugin: String,
    target_plugin: String,
    target_vfs_path: String,
    source_commit: String,
}

#[async_trait]
impl ToolFunction for PluginForkTool {
    type Input = PluginForkInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let summary = input.summary.trim().to_string();
        let body = input.body.trim().to_string();
        if summary.is_empty() || body.is_empty() {
            return Err(ToolError::ValidationFailed {
                message: "summary and body must be non-empty".into(),
            });
        }
        let intent = parse_intent(input.intent.as_deref())?;

        let reference = resolve_target(&self.state, &input.reference_plugin_path)
            .await
            .map_err(tool_error)?;
        let registry = self.state.registry.clone();
        let infra = registry.infra().map_err(tool_error)?;
        let request = RequestRecord {
            id: String::new(),
            created_at: 0,
            source: RequestSource::Agent,
            intent,
            summary,
            body,
            plugin_name: Some(input.plugin_name),
            evidence_ids: Vec::new(),
            status: RequestStatus::Open,
        };
        let reference_plugin = reference.plugin_name;
        let output =
            tokio::task::spawn_blocking(move || -> std::result::Result<ForkOutput, ToolError> {
                let operator = infra
                    .fork_plugin(&reference_plugin, request)
                    .map_err(tool_error)?;
                let manifest = operator.manifest();
                let request_id =
                    manifest
                        .lifecycle
                        .request_ids
                        .first()
                        .cloned()
                        .ok_or_else(|| ToolError::ExecutionFailed {
                            source: "fork completed without its lifecycle request id".into(),
                        })?;
                let source_commit = manifest.lifecycle.source_commit.clone().unwrap_or_default();
                Ok(ForkOutput {
                    request_id,
                    reference_plugin,
                    target_plugin: operator.plugin_name().to_string(),
                    target_vfs_path: operator.development_vfs_path().map_err(tool_error)?,
                    source_commit,
                })
            })
            .await
            .map_err(|join| ToolError::ExecutionFailed {
                source: format!("plugin fork task failed: {join}").into(),
            })??;

        Ok(ToolResult::success_json(json!({
            "request_id": output.request_id,
            "reference_plugin": output.reference_plugin,
            "target_plugin": output.target_plugin,
            "target_vfs_path": output.target_vfs_path,
            "source_commit": output.source_commit,
        })))
    }
}

fn parse_intent(value: Option<&str>) -> Result<RequestIntent, ToolError> {
    match value.map(str::trim) {
        None | Some("") | Some("optimize_node") => Ok(RequestIntent::OptimizeNode),
        Some("new_node") => Ok(RequestIntent::NewNode),
        Some("fix_node") => Ok(RequestIntent::FixNode),
        Some("clarify_contract") => Ok(RequestIntent::ClarifyContract),
        Some(other) => Err(ToolError::ValidationFailed {
            message: format!(
                "unknown intent {other:?}; use new_node, optimize_node, fix_node, or clarify_contract"
            ),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_requested_lifecycle_intents() {
        assert_eq!(
            parse_intent(Some("new_node")).unwrap(),
            RequestIntent::NewNode
        );
        assert_eq!(parse_intent(None).unwrap(), RequestIntent::OptimizeNode);
        assert!(parse_intent(Some("release")).is_err());
    }
}
