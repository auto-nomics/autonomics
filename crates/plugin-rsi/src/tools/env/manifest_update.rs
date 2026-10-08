//! Tool: update the editable parts of an environment manifest.

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;
use serde_json::json;

use crate::{
    EnvironmentBase, EnvironmentManifest, EnvironmentSmokeTest,
    env_manifest::validate_interpreters,
    env_store::{load_manifest, save_manifest},
};

use super::EnvToolState;
use super::{ensure_editable, environment_manifest_lock, resolve_target, tool_error};

/// Fields of the replacement `[base]` block.
#[derive(Debug, schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
pub(super) struct EnvironmentBaseFields {
    /// Digest-pinned base image reference (`host/path@sha256:…`).
    pub reference: String,
    /// Display-only base tag.
    pub tag: Option<String>,
    /// Upstream source description.
    pub upstream: Option<String>,
    /// SPDX license identifier.
    pub license: Option<String>,
}

/// One replacement smoke test.
#[derive(Debug, schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
pub(super) struct EnvironmentSmokeTestInput {
    /// Stable test name.
    pub name: String,
    /// Executable and arguments. No shell string is accepted.
    pub argv: Vec<String>,
    /// Optional substring assertion on the test's stdout.
    pub expected_stdout_contains: Option<String>,
}

#[tool(
    name = "environment_manifest_update",
    description = "Update the editable parts of an environment manifest: interpreters, the digest-pinned \
                   base block, or the complete smoke-test list. manifest.toml cannot be written through \
                   the VFS; this tool is the only path."
)]
pub(super) struct EnvironmentManifestUpdateInput {
    /// Environment workspace path, such as `/environments/dev/bioconductor-extra`.
    environment_path: String,
    /// Complete replacement interpreter list, e.g. ["Rscript", "sh"].
    interpreters: Option<Vec<String>>,
    /// Complete replacement `[base]` block.
    base: Option<EnvironmentBaseFields>,
    /// Complete replacement smoke-test list; at least one test is required.
    tests: Option<Vec<EnvironmentSmokeTestInput>>,
}

pub(super) struct EnvironmentManifestUpdateTool {
    pub(super) state: EnvToolState,
}

#[async_trait]
impl ToolFunction for EnvironmentManifestUpdateTool {
    type Input = EnvironmentManifestUpdateInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let target = resolve_target(&self.state, &input.environment_path)
            .await
            .map_err(tool_error)?;
        let manifest_lock = environment_manifest_lock(&self.state.registry, &target.environment_id);
        let _guard = manifest_lock.lock().await;
        let output = tokio::task::spawn_blocking(
            move || -> std::result::Result<EnvironmentManifest, ToolError> {
                let mut manifest = load_manifest(&target.workspace).map_err(tool_error)?;
                ensure_editable(&manifest).map_err(tool_error)?;

                if let Some(interpreters) = input.interpreters {
                    validate_interpreters(&interpreters).map_err(tool_error)?;
                    manifest.interpreters = interpreters;
                }
                if let Some(base) = input.base {
                    container_runtime::ImageReference::parse(&base.reference).map_err(|error| {
                        ToolError::ValidationFailed {
                            message: format!("base reference is not digest-pinned: {error}"),
                        }
                    })?;
                    manifest.base = EnvironmentBase {
                        reference: base.reference,
                        tag: base.tag,
                        upstream: base.upstream,
                        license: base.license,
                    };
                }
                if let Some(tests) = input.tests {
                    if tests.is_empty() {
                        return Err(ToolError::ValidationFailed {
                            message: "at least one smoke test is required".into(),
                        });
                    }
                    for test in &tests {
                        validate_smoke_test(test)
                            .map_err(|error| ToolError::ValidationFailed { message: error })?;
                    }
                    manifest.tests = tests
                        .into_iter()
                        .map(|test| EnvironmentSmokeTest {
                            name: test.name,
                            argv: test.argv,
                            expected_stdout_contains: test.expected_stdout_contains,
                        })
                        .collect();
                }

                // A manifest edit invalidates any pending local activation,
                // mirroring the plugin manifest tools.
                manifest.lifecycle.publication_pending = false;
                save_manifest(&target.workspace, &manifest).map_err(tool_error)?;
                Ok(manifest)
            },
        )
        .await
        .map_err(|join| ToolError::ExecutionFailed {
            source: format!("environment manifest update task failed: {join}").into(),
        })??;

        Ok(ToolResult::success_json(json!({
            "environment_id": output.environment_id,
            "environment_vfs_path": target.virtual_path,
            "status": output.status,
            "interpreters": output.interpreters,
            "base_reference": output.base.reference,
            "test_count": output.tests.len(),
        })))
    }
}

fn validate_smoke_test(test: &EnvironmentSmokeTestInput) -> std::result::Result<(), String> {
    if test.name.trim().is_empty() {
        return Err("smoke tests must have a name".into());
    }
    if test.argv.is_empty() || test.argv.iter().any(|arg| arg.contains('\0')) {
        return Err(format!(
            "smoke test `{}` must declare a nonempty NUL-free argv",
            test.name
        ));
    }
    Ok(())
}
