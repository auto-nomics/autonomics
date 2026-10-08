//! Manifest I/O shared by the manifest-mutating plugin tools.
//!
//! `load_manifest` is also used by `container_run` to discover the image
//! reference; the other helpers are only used by the manifest-mutating tools.

use container_plugin::{manifest::PluginManifest, node_definition::NodeDefinition};
use vfs::OpendalFileStorage;

use crate::{Error, PluginWorkspace, Result as RsiResult, plugin::is_editable};

use super::PluginTarget;

const MANIFEST_FILE: &str = "manifest.toml";

fn vfs_error(error: impl std::fmt::Display) -> Error {
    Error::Validation(format!("VFS operation failed: {error}"))
}

fn manifest_path(target: &PluginTarget) -> String {
    format!(
        "{}/{}",
        target.virtual_path.trim_end_matches('/'),
        MANIFEST_FILE
    )
}

async fn read_virtual_text(vfs: &OpendalFileStorage, path: &str) -> RsiResult<String> {
    let length = vfs.content_length(path).await.map_err(vfs_error)?;
    let bytes = vfs
        .read_range(path, 0..length)
        .await
        .map_err(vfs_error)?
        .to_vec();
    String::from_utf8(bytes)
        .map_err(|error| Error::Validation(format!("VFS file `{path}` is not UTF-8: {error}")))
}

pub(super) async fn read_manifest_text(target: &PluginTarget) -> RsiResult<String> {
    read_virtual_text(&target.vfs, &manifest_path(target)).await
}

/// Parse manifest text with the same grammar as [`load_manifest`]; shared by
/// the manifest gates and the manifest-editing tools.
pub(super) fn parse_manifest_text(text: &str) -> std::result::Result<PluginManifest, String> {
    toml::from_str(text).map_err(|error| format!("invalid {MANIFEST_FILE}: {error}"))
}

pub(super) async fn load_manifest(target: &PluginTarget) -> RsiResult<PluginManifest> {
    let text = read_manifest_text(target).await?;
    let manifest = parse_manifest_text(&text).map_err(Error::Validation)?;
    if manifest.plugin_name != target.plugin_name {
        return Err(Error::Validation(format!(
            "workspace plugin_name `{}` does not match path plugin `{}`",
            manifest.plugin_name, target.plugin_name
        )));
    }
    Ok(manifest)
}

pub(super) async fn editable_manifest(target: &PluginTarget) -> RsiResult<PluginManifest> {
    let manifest = load_manifest(target).await?;
    if !is_editable(manifest.status) {
        return Err(Error::Validation(format!(
            "plugin `{}` cannot be edited from manifest status {:?}",
            target.plugin_name, manifest.status
        )));
    }
    Ok(manifest)
}

pub(super) fn save_manifest(
    workspace: &PluginWorkspace,
    manifest: &mut PluginManifest,
) -> RsiResult<()> {
    manifest.lifecycle.publication_pending = false;
    toml::to_string_pretty(manifest)
        .map_err(|error| Error::Validation(format!("cannot encode manifest: {error}")))
        .and_then(|text| workspace.write_text(MANIFEST_FILE, &text))
}

pub(super) fn selected_node(
    manifest: &PluginManifest,
    node_kind: &str,
) -> RsiResult<NodeDefinition> {
    manifest
        .nodes
        .iter()
        .find(|node| node.kind == node_kind)
        .cloned()
        .ok_or_else(|| {
            Error::Validation(format!(
                "selected node `{}` is missing from the plugin manifest",
                node_kind
            ))
        })
}
