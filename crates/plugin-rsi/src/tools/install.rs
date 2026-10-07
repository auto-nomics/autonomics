//! Tool: validate a development workspace and activate it in the local DAG.

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;
use serde_json::json;

use crate::{LocalActivationOutcome, PluginStatus};

use super::PluginToolState;
use super::helpers::{resolve_target, tool_error};

#[tool(
    name = "plugin_install",
    description = "Validate a developed plugin and install its immutable local snapshot into the DAG registry. \
                   This is required after fork/edit before the node can run; it does not publish to GitHub."
)]
pub(super) struct PluginInstallInput {
    /// Developed workspace path, such as `/plugins/dev/hello-world`.
    plugin_path: String,
}

pub(super) struct PluginInstallTool {
    pub(super) state: PluginToolState,
}

#[derive(Debug)]
struct InstallOutput {
    plugin_name: String,
    plugin_vfs_path: String,
    activated: bool,
    report: crate::ValidationReport,
    status: PluginStatus,
    node_kinds: Vec<String>,
    node_addresses: Vec<String>,
    commit: Option<String>,
    digest: Option<String>,
}

#[async_trait]
impl ToolFunction for PluginInstallTool {
    type Input = PluginInstallInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let target = resolve_target(&self.state, &input.plugin_path)
            .await
            .map_err(tool_error)?;
        let infra = self.state.registry.infra().map_err(tool_error)?;
        let plugin_name = target.plugin_name;
        let output = tokio::task::spawn_blocking(
            move || -> std::result::Result<InstallOutput, ToolError> {
                let activation = infra
                    .validate_and_activate_local(&plugin_name)
                    .map_err(tool_error)?;
                let mut commit = None;
                let mut digest = None;
                let report = match activation {
                    LocalActivationOutcome::Activated(report, source) => {
                        commit = Some(source.commit);
                        digest = Some(source.digest);
                        report
                    }
                    LocalActivationOutcome::NeedsFix(report) => report,
                };
                let store = infra.store();
                let operator = store
                    .develop(&plugin_name)
                    .map_err(tool_error)?
                    .ok_or_else(|| ToolError::ExecutionFailed {
                        source: format!("plugin `{plugin_name}` disappeared during install").into(),
                    })?;
                Ok(InstallOutput {
                    plugin_vfs_path: operator.development_vfs_path().map_err(tool_error)?,
                    activated: commit.is_some(),
                    status: operator.status(),
                    node_kinds: operator.owned_node_kinds(),
                    node_addresses: operator
                        .owned_node_kinds()
                        .into_iter()
                        .map(|kind| format!("{}/{}", operator.plugin_name(), kind))
                        .collect(),
                    commit,
                    digest,
                    plugin_name,
                    report,
                })
            },
        )
        .await
        .map_err(|join| ToolError::ExecutionFailed {
            source: format!("plugin install task failed: {join}").into(),
        })??;

        let report =
            serde_json::to_value(&output.report).map_err(|error| ToolError::ExecutionFailed {
                source: Box::new(error),
            })?;
        Ok(ToolResult::success_json(json!({
            "activated": output.activated,
            "plugin_name": output.plugin_name,
            "plugin_vfs_path": output.plugin_vfs_path,
            "status": output.status,
            "node_kinds": output.node_kinds,
            "node_addresses": output.node_addresses,
            "validation_report": report,
            "local_commit": output.commit,
            "local_digest": output.digest,
        })))
    }
}
