//! Tool: edit a plugin workspace's `manifest.toml` in place.
//!
//! Mirrors the VFS `edit` op (exact old/new text replacement) but is one of
//! the two write paths for `manifest.toml`, which the VFS deny rules lock.
//! Host-owned manifest fields are guarded, the node set may not grow or
//! shrink, and every candidate manifest is validated and compiled before it
//! lands; a rejected edit leaves the file untouched.

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;
use container_plugin::{manifest::PluginManifest, node_definition};
use serde_json::json;
use toml::Value as TomlValue;

use super::PluginToolState;
use super::helpers::{resolve_target, tool_error};
use super::manifest::{load_manifest, parse_manifest_text, read_manifest_text, save_manifest};

#[tool(
    name = "plugin_node_update",
    description = "Edit a plugin workspace's manifest.toml via exact old_string/new_string text replacement. \
                   Read the current manifest.toml through the VFS first and copy its text exactly; extra context \
                   lines keep the match unambiguous. Daemon-owned fields (schema_version, plugin_name, status, \
                   installation, lifecycle, image, panels) must not change, and the node set cannot grow or \
                   shrink (add nodes with plugin_node_create). Every node is validated and the whole manifest \
                   compiled before the edit lands; a rejected edit leaves the file untouched."
)]
pub(super) struct PluginNodeUpdateInput {
    /// Plugin workspace path, such as `/plugins/dev/hello-world`.
    plugin_path: String,
    /// Text to replace, copied verbatim from the current manifest.toml;
    /// include surrounding lines so exactly one location matches.
    old_string: String,
    /// Replacement text; empty deletes the matched lines.
    new_string: String,
    /// Replace every match instead of requiring exactly one; defaults to false.
    replace_all: Option<bool>,
}

pub(super) struct PluginNodeUpdateTool {
    pub(super) state: PluginToolState,
}

#[async_trait]
impl ToolFunction for PluginNodeUpdateTool {
    type Input = PluginNodeUpdateInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let target = resolve_target(&self.state, &input.plugin_path)
            .await
            .map_err(tool_error)?;
        let manifest_lock = self.state.registry.manifest_lock(&target.plugin_name);
        let _guard = manifest_lock.lock().await;

        let current_text = read_manifest_text(&target).await.map_err(tool_error)?;
        let current = parse_manifest_text(&current_text).map_err(validation)?;
        if !crate::plugin::is_editable(current.status) {
            return Err(validation(format!(
                "plugin `{}` cannot be edited from manifest status {:?}",
                target.plugin_name, current.status
            )));
        }

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
                    "old_string not found in manifest.toml; read the file through the VFS and \
                     copy its exact text, adding context lines to disambiguate",
                ));
            }
            apply_patch::FuzzyEditOutcome::Ambiguous { count } => {
                return Err(validation(format!(
                    "old_string matches {count} locations in manifest.toml; include more \
                     surrounding lines or set replace_all=true"
                )));
            }
        };

        let mut candidate = parse_manifest_text(&edited_text).map_err(validation)?;
        guard_host_owned(&current, &candidate, &target.plugin_name)?;
        guard_node_set(&current, &candidate, &target)?;
        validate_nodes(&candidate).map_err(validation)?;
        crate::validate::registry_compile(&candidate).map_err(validation)?;

        save_manifest(&target.workspace, &mut candidate).map_err(tool_error)?;
        // Reload from disk: what landed must parse (the write is a fresh
        // serialization, so this also guards round-trip drift).
        let reloaded = load_manifest(&target).await.map_err(tool_error)?;
        let node_kinds: Vec<String> = reloaded.nodes.iter().map(|node| node.kind.clone()).collect();
        Ok(ToolResult::success_json(json!({
            "plugin_name": reloaded.plugin_name,
            "plugin_vfs_path": target.virtual_path,
            "node_kinds": node_kinds,
            "node_count": reloaded.nodes.len(),
        })))
    }
}

/// Host-owned fields must survive the edit untouched. `PluginManifest` has
/// no `PartialEq`, so compare toml round-trips with the `nodes` projection
/// emptied — everything outside `[[nodes]]` must be byte-equal semantics.
fn guard_host_owned(
    current: &PluginManifest,
    candidate: &PluginManifest,
    plugin_name: &str,
) -> Result<(), ToolError> {
    let project = |manifest: &PluginManifest| -> std::result::Result<TomlValue, ToolError> {
        let text = toml::to_string(manifest).map_err(|error| ToolError::ExecutionFailed {
            source: format!("cannot encode manifest for the host-owned guard: {error}").into(),
        })?;
        let mut value: TomlValue = toml::from_str(&text).map_err(|error| ToolError::ExecutionFailed {
            source: format!("cannot decode manifest for the host-owned guard: {error}").into(),
        })?;
        value["nodes"] = TomlValue::Array(Vec::new());
        Ok(value)
    };
    if project(current)? != project(candidate)? {
        return Err(validation(format!(
            "only [[nodes]] content may change; schema_version, plugin_name, status, \
             installation, lifecycle, image, and panels are daemon-owned and must stay \
             exactly as read (plugin `{plugin_name}`)"
        )));
    }
    Ok(())
}

/// Update replaces definitions in place: the node count may not change and
/// kinds must stay unique (a rename is fine while it stays collision-free).
fn guard_node_set(
    current: &PluginManifest,
    candidate: &PluginManifest,
    target: &super::PluginTarget,
) -> Result<(), ToolError> {
    if candidate.nodes.len() != current.nodes.len() {
        return Err(validation(format!(
            "plugin_node_update replaces node definitions in place; the node set cannot \
             change ({} -> {} nodes). Add nodes with plugin_node_create",
            current.nodes.len(),
            candidate.nodes.len()
        )));
    }
    let mut kinds: Vec<&str> = candidate.nodes.iter().map(|node| node.kind.as_str()).collect();
    kinds.sort_unstable();
    if let Some(duplicated) = kinds.windows(2).find(|pair| pair[0] == pair[1]) {
        return Err(validation(format!(
            "node kind `{}` appears more than once in plugin `{}`",
            duplicated[0], target.plugin_name
        )));
    }
    Ok(())
}

/// Per-node checks the install-time `manifest` gate repeats. Script files
/// are deliberately not required to exist yet: authoring the manifest before
/// its script is a legitimate order, and `plugin_install` reviews the full
/// script discipline.
fn validate_nodes(candidate: &PluginManifest) -> std::result::Result<(), String> {
    for node in &candidate.nodes {
        node_definition::validate(node)?;
    }
    Ok(())
}

fn validation(message: impl Into<String>) -> ToolError {
    ToolError::ValidationFailed {
        message: message.into(),
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use agentik_core::tools::ToolFunction as _;
    use serde_json::json;

    #[test]
    fn update_schema_is_a_flat_text_edit() {
        let registrations = super::super::plugin_development_tool_registrations("schema-agent");
        let registration = registrations
            .iter()
            .find(|registration| registration.definition.name == "plugin_node_update")
            .unwrap();
        let properties = &registration.definition.input_schema.properties;

        assert!(properties.get("plugin_path").is_some());
        assert!(properties.get("old_string").is_some());
        assert!(properties.get("new_string").is_some());
        assert!(properties.get("replace_all").is_some());
        assert!(
            properties.get("node").is_none(),
            "the nested node parameter must be gone"
        );
    }

    #[tokio::test]
    async fn unresolved_plugin_workspace_is_rejected() {
        let tool = PluginNodeUpdateTool {
            state: super::super::PluginToolState {
                registry: super::super::PluginDevelopmentToolsetRegistry::global(),
                principal: super::super::principal_for_agent("schema-agent"),
            },
        };
        tool.execute(json!({
            "plugin_path": "/plugins/dev/demo-plugin",
            "old_string": "kind = \"demo_node\"",
            "new_string": "kind = \"replacement_node\"",
        }))
        .await
        .expect_err("unconfigured registry must reject the edit");
    }
}
