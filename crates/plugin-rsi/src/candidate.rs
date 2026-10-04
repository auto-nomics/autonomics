use std::{collections::BTreeSet, path::Path};

use container_plugin::{manifest::PluginManifest, node_definition};

use crate::{Error, ProposalWorkspace, Result};

/// Text-level changes between a source proposal and a development candidate.
#[derive(Debug, Clone, Default)]
pub struct DevelopmentChanges {
    /// Safe relative paths present only in the candidate.
    pub added: Vec<String>,
    /// Safe relative paths whose text differs between source and candidate.
    pub changed: Vec<String>,
    /// Safe relative paths present only in the source.
    pub removed: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct WorkspaceChanges {
    added: Vec<String>,
    changed: Vec<String>,
    removed: Vec<String>,
}

impl WorkspaceChanges {
    pub(crate) fn is_empty(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty() && self.removed.is_empty()
    }
}

impl From<WorkspaceChanges> for DevelopmentChanges {
    fn from(value: WorkspaceChanges) -> Self {
        Self {
            added: value.added,
            changed: value.changed,
            removed: value.removed,
        }
    }
}

pub(crate) fn prepare_candidate_workspace(
    destination_root: &Path,
    source: &ProposalWorkspace,
) -> Result<ProposalWorkspace> {
    if destination_root.exists() {
        return Err(Error::Validation(format!(
            "development candidate `{}` already exists",
            destination_root.display()
        )));
    }
    let workspace_root = destination_root.to_path_buf().join("workspace");
    std::fs::create_dir_all(&workspace_root)?;
    let workspace = ProposalWorkspace::new(workspace_root);
    for relative in source.list_files()? {
        workspace.write_text(&relative, &source.read_text(&relative)?)?;
    }
    Ok(workspace)
}

pub(crate) fn workspace_changes(
    source: &ProposalWorkspace,
    candidate: &ProposalWorkspace,
) -> Result<WorkspaceChanges> {
    let before = BTreeSet::from_iter(source.list_files()?);
    let after = BTreeSet::from_iter(candidate.list_files()?);
    let mut changes = WorkspaceChanges::default();
    for relative in &after {
        if !before.contains(relative.as_str()) {
            changes.added.push(relative.clone());
        } else if source.read_text(relative)? != candidate.read_text(relative)? {
            changes.changed.push(relative.clone());
        }
    }
    for relative in before {
        if !after.contains(&relative) {
            changes.removed.push(relative);
        }
    }
    Ok(changes)
}

pub(crate) fn validate_candidate(
    candidate: &ProposalWorkspace,
    plugin_name: &str,
    environment_reference: &str,
) -> Result<PluginManifest> {
    let files = candidate.list_files()?;
    if files.iter().any(|path| {
        Path::new(path)
            .file_name()
            .is_some_and(|name| name == "Dockerfile")
    }) {
        return Err(Error::Validation(
            "development candidate cannot add a Dockerfile; use an approved environment".into(),
        ));
    }
    let manifest: PluginManifest = toml::from_str(&candidate.read_text("manifest.toml")?)
        .map_err(|error| crate::Error::Validation(format!("invalid manifest.toml: {error}")))?;
    if manifest.plugin_name != plugin_name {
        return Err(crate::Error::Validation(
            "development candidate cannot change manifest plugin_name".into(),
        ));
    }
    if manifest.image.reference.as_str() != environment_reference {
        return Err(crate::Error::Validation(
            "development candidate cannot change the bound environment".into(),
        ));
    }
    if !manifest.panels.is_empty() {
        return Err(crate::Error::Validation(
            "development candidate cannot add panels in the MVP".into(),
        ));
    }
    if manifest.nodes.is_empty() {
        return Err(crate::Error::Validation(
            "development candidate cannot remove the last node".into(),
        ));
    }
    let mut kinds = BTreeSet::new();
    for node in &manifest.nodes {
        node_definition::validate(node).map_err(|error| {
            crate::Error::Validation(format!("invalid node `{}`: {error}", node.kind))
        })?;
        if !kinds.insert(node.kind.clone()) {
            return Err(crate::Error::Validation(format!(
                "duplicate node kind `{}`",
                node.kind
            )));
        }
        if node.command.script.is_some() || node.command.script_file.is_none() {
            return Err(crate::Error::Validation(format!(
                "node `{}` must declare only a safe script_file",
                node.kind
            )));
        }
        let script_file = node.command.script_file.as_deref().expect("checked above");
        candidate.read_text(script_file)?;
    }
    Ok(manifest)
}

pub(crate) fn adopt_candidate(
    source: &ProposalWorkspace,
    candidate: &ProposalWorkspace,
    changes: &WorkspaceChanges,
) -> Result<()> {
    for relative in changes.added.iter().chain(&changes.changed) {
        source.write_text(relative, &candidate.read_text(relative)?)?;
    }
    for relative in &changes.removed {
        source.remove_file(relative)?;
    }
    Ok(())
}
