//! Tool: edit an environment workspace's `manifest.toml` in place.
//!
//! Mirrors the VFS `edit` op (exact old/new text replacement) but is the one
//! write path for `manifest.toml`, which the VFS deny rules lock. Every edit
//! is parsed and validated against the same gates as `environment_validate`
//! before it lands; a rejected edit leaves the file untouched.

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;
use serde_json::json;

use crate::{
    EnvironmentManifest,
    env_store::{load_manifest, save_manifest},
    env_validate::{check_manifest, gate_base_policy, parse_manifest_text},
};

use super::EnvToolState;
use super::{ensure_editable, environment_manifest_lock, resolve_target, tool_error};

#[tool(
    name = "environment_manifest_update",
    description = "Edit an environment workspace's manifest.toml via exact old_string/new_string text \
                   replacement. Read the current manifest.toml through the VFS first and copy its text \
                   exactly; extra context lines keep the match unambiguous. Host-owned fields \
                   (environment_id, status, lifecycle) must not change, and base.reference must stay \
                   digest-pinned. The edited manifest is validated (structure plus base policy) before \
                   it is written; a rejected edit leaves the file untouched."
)]
pub(super) struct EnvironmentManifestUpdateInput {
    /// Environment workspace path, such as `/environments/dev/bioconductor-extra`.
    environment_path: String,
    /// Text to replace, copied verbatim from the current manifest.toml;
    /// include surrounding lines so exactly one location matches.
    old_string: String,
    /// Replacement text; empty deletes the matched lines.
    new_string: String,
    /// Replace every match instead of requiring exactly one; defaults to false.
    replace_all: Option<bool>,
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
        let (virtual_path, manifest) = tokio::task::spawn_blocking(
            move || -> std::result::Result<(String, EnvironmentManifest), ToolError> {
                let workspace = &target.workspace;
                let current_text = workspace.read_text("manifest.toml").map_err(tool_error)?;
                let current = parse_manifest_text(&current_text).map_err(validation)?;
                ensure_editable(&current).map_err(|error| validation(error.to_string()))?;

                let replace_all = input.replace_all.unwrap_or(false);
                let edited_text = match apply_patch::fuzzy_edit(
                    &current_text,
                    &input.old_string,
                    &input.new_string,
                    replace_all,
                ) {
                    apply_patch::FuzzyEditOutcome::Replaced { new_content, .. } => new_content,
                    apply_patch::FuzzyEditOutcome::NotFound => {
                        return Err(validation(
                            "old_string not found in manifest.toml; read the file through the \
                             VFS and copy its exact text, adding context lines to disambiguate",
                        ));
                    }
                    apply_patch::FuzzyEditOutcome::Ambiguous { count } => {
                        return Err(validation(format!(
                            "old_string matches {count} locations in manifest.toml; include \
                             more surrounding lines or set replace_all=true"
                        )));
                    }
                };

                let candidate = parse_manifest_text(&edited_text).map_err(validation)?;

                // Host-owned fields survive edits untouched; lifecycle may
                // differ only in publication_pending, which this tool resets.
                if candidate.environment_id != target.environment_id {
                    return Err(validation(format!(
                        "environment_id `{}` is host-owned and must stay `{}`",
                        candidate.environment_id, target.environment_id
                    )));
                }
                if candidate.status != current.status {
                    return Err(validation(format!(
                        "status is host-owned and cannot change through this tool \
                         (current: {:?}); lifecycle tools move it",
                        current.status
                    )));
                }
                let mut candidate_lifecycle = candidate.lifecycle.clone();
                candidate_lifecycle.publication_pending = current.lifecycle.publication_pending;
                if candidate_lifecycle != current.lifecycle {
                    return Err(validation(
                        "lifecycle is host-owned; revert the changes to the [lifecycle] block",
                    ));
                }

                check_manifest(&candidate, workspace, &target.environment_id)
                    .map_err(validation)?;
                gate_base_policy(&candidate).map_err(validation)?;

                let mut manifest = candidate;
                // A manifest edit invalidates any pending local activation,
                // mirroring the plugin manifest tools.
                manifest.lifecycle.publication_pending = false;
                save_manifest(workspace, &manifest).map_err(tool_error)?;
                // Reload from disk: what landed must parse (the write is a
                // fresh serialization, so this also guards round-trip drift).
                let reloaded = load_manifest(workspace).map_err(tool_error)?;
                Ok((target.virtual_path, reloaded))
            },
        )
        .await
        .map_err(|join| ToolError::ExecutionFailed {
            source: format!("environment manifest update task failed: {join}").into(),
        })??;

        Ok(ToolResult::success_json(json!({
            "environment_id": manifest.environment_id,
            "environment_vfs_path": virtual_path,
            "status": manifest.status,
            "interpreters": manifest.interpreters,
            "base_reference": manifest.base.reference,
            "test_count": manifest.tests.len(),
        })))
    }
}

fn validation(message: impl Into<String>) -> ToolError {
    ToolError::ValidationFailed {
        message: message.into(),
    }
}
