//! Tools: validate, build, and locally install environment workspaces.

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;
use serde_json::{Value, json};

use crate::{
    EnvironmentLocalActivationOutcome, EnvironmentValidationOutcome, EnvironmentValidationReport,
};

use super::EnvToolState;
use super::{environment_manifest_lock, resolve_target, tool_error};

fn report_json(report: &EnvironmentValidationReport) -> Result<Value, ToolError> {
    serde_json::to_value(report).map_err(|error| ToolError::ExecutionFailed {
        source: Box::new(error),
    })
}

#[tool(
    name = "environment_validate",
    description = "Run the four deterministic static gates (manifest, base policy, Containerfile review, \
                   secret scan) over an environment workspace. Fast iteration; no image is built."
)]
pub(super) struct EnvironmentValidateInput {
    /// Environment workspace path, such as `/environments/dev/bioconductor-extra`.
    environment_path: String,
}

pub(super) struct EnvironmentValidateTool {
    pub(super) state: EnvToolState,
}

#[async_trait]
impl ToolFunction for EnvironmentValidateTool {
    type Input = EnvironmentValidateInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let target = resolve_target(&self.state, &input.environment_path)
            .await
            .map_err(tool_error)?;
        let infra = self
            .state
            .registry
            .environment_infra()
            .map_err(tool_error)?;
        let manifest_lock = environment_manifest_lock(&self.state.registry, &target.environment_id);
        let _guard = manifest_lock.lock().await;
        let environment_id = target.environment_id.clone();
        let outcome = infra
            .validate_environment_local(&environment_id)
            .await
            .map_err(tool_error)?;
        let (report, passed) = match outcome {
            EnvironmentValidationOutcome::Passed(report) => (report, true),
            EnvironmentValidationOutcome::NeedsFix(report) => (report, false),
        };
        Ok(ToolResult::success_json(json!({
            "environment_id": target.environment_id,
            "environment_vfs_path": target.virtual_path,
            "passed": passed,
            "validation_report": report_json(&report)?,
        })))
    }
}

#[tool(
    name = "environment_build",
    description = "Run all six environment gates, including the image build and smoke tests. Returns the \
                   local digest and tag on success. Requires the host image builder; a missing builder \
                   is reported as a blocked gate, not a pass."
)]
pub(super) struct EnvironmentBuildInput {
    /// Environment workspace path, such as `/environments/dev/bioconductor-extra`.
    environment_path: String,
}

pub(super) struct EnvironmentBuildTool {
    pub(super) state: EnvToolState,
}

#[async_trait]
impl ToolFunction for EnvironmentBuildTool {
    type Input = EnvironmentBuildInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let target = resolve_target(&self.state, &input.environment_path)
            .await
            .map_err(tool_error)?;
        let infra = self
            .state
            .registry
            .environment_infra()
            .map_err(tool_error)?;
        let manifest_lock = environment_manifest_lock(&self.state.registry, &target.environment_id);
        let _guard = manifest_lock.lock().await;
        let environment_id = target.environment_id.clone();
        let outcome = infra
            .build_environment(&environment_id)
            .await
            .map_err(tool_error)?;
        let (report, built) = match outcome {
            EnvironmentValidationOutcome::Passed(report) => (report, true),
            EnvironmentValidationOutcome::NeedsFix(report) => (report, false),
        };
        Ok(ToolResult::success_json(json!({
            "environment_id": target.environment_id,
            "environment_vfs_path": target.virtual_path,
            "built": built,
            "local_tag": report.local_tag,
            "digest": report.digest,
            "validation_report": report_json(&report)?,
        })))
    }
}

#[tool(
    name = "environment_install",
    description = "Validate, build, and locally activate an environment: after this returns the digest-pinned \
                   localhost reference is in the environment catalog and plugins can bind it. Background \
                   publication to the remote registry happens later without blocking."
)]
pub(super) struct EnvironmentInstallInput {
    /// Environment workspace path, such as `/environments/dev/bioconductor-extra`.
    environment_path: String,
}

pub(super) struct EnvironmentInstallTool {
    pub(super) state: EnvToolState,
}

#[async_trait]
impl ToolFunction for EnvironmentInstallTool {
    type Input = EnvironmentInstallInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let target = resolve_target(&self.state, &input.environment_path)
            .await
            .map_err(tool_error)?;
        let infra = self
            .state
            .registry
            .environment_infra()
            .map_err(tool_error)?;
        let manifest_lock = environment_manifest_lock(&self.state.registry, &target.environment_id);
        let _guard = manifest_lock.lock().await;
        let environment_id = target.environment_id.clone();
        let activation = infra
            .install_environment_local(&environment_id)
            .await
            .map_err(tool_error)?;
        let mut reference = None;
        let mut digest = None;
        let report = match activation {
            EnvironmentLocalActivationOutcome::Activated(report, installed) => {
                reference = Some(installed.reference);
                digest = Some(installed.digest);
                report
            }
            EnvironmentLocalActivationOutcome::NeedsFix(report) => report,
        };
        Ok(ToolResult::success_json(json!({
            "activated": reference.is_some(),
            "environment_id": target.environment_id,
            "environment_vfs_path": target.virtual_path,
            "catalog_reference": reference,
            "digest": digest,
            "validation_report": report_json(&report)?,
        })))
    }
}
