//! Long-lived, plugin-scoped development workspaces.
//!
//! A plugin directory is both the git repository and the lifecycle record.
//! Requests point at it, agents edit it in place, and `manifest.toml.status`
//! is the daemon-owned state marker. This avoids proposal-specific repository
//! copies in the direct-development path.

use std::{collections::BTreeSet, path::PathBuf};

use container_plugin::manifest::{ImageMetadata, PluginManifest, PluginStatus};
use container_runtime::ImageReference;

use crate::{
    Error, GitRepo, PluginDevelopmentToolsetRegistry, ProposalWorkspace, RequestStore, Result,
    validate::EnvironmentCatalog,
};

/// Creates and opens plugin repositories under one root.
#[derive(Debug, Clone)]
pub struct PluginStore {
    root: PathBuf,
    default_branch: String,
    author_name: String,
    author_email: String,
}

/// A host-owned handle to one long-lived plugin workspace.
#[derive(Debug)]
pub struct PluginOperator<'a> {
    store: &'a PluginStore,
    plugin_name: String,
    manifest: PluginManifest,
}

impl PluginStore {
    /// Open `<state_dir>/plugins` as the unified plugin workspace root.
    pub fn open(
        state_dir: &std::path::Path,
        default_branch: &str,
        author_name: &str,
        author_email: &str,
    ) -> Self {
        Self {
            root: state_dir.join("plugins"),
            default_branch: default_branch.to_string(),
            author_name: author_name.to_string(),
            author_email: author_email.to_string(),
        }
    }

    /// Create one named plugin repository and bind its approved environment.
    pub fn create(
        &self,
        plugin_name: &str,
        environment_id: &str,
        request_ids: &[String],
        rationale: &str,
        requests: &RequestStore,
        catalog: &EnvironmentCatalog,
    ) -> Result<PluginOperator<'_>> {
        crate::validate_plugin_name(plugin_name)?;
        validate_requests(request_ids, requests, rationale)?;
        let environment = catalog.get(environment_id).ok_or_else(|| {
            Error::Validation(format!(
                "environment {environment_id:?} is not approved for RSI"
            ))
        })?;
        let reference = ImageReference::parse(&environment.reference).map_err(|error| {
            Error::Validation(format!("invalid environment reference: {error}"))
        })?;

        let path = self.plugin_path(plugin_name);
        if path.exists() {
            return Err(Error::InvalidRequest(format!(
                "plugin directory already exists: {}",
                path.display()
            )));
        }
        GitRepo::init(&path, &self.default_branch)?;
        let mut image = ImageMetadata::default();
        image.reference = reference;
        let manifest = PluginManifest {
            plugin_name: plugin_name.to_string(),
            status: PluginStatus::Draft,
            image,
            ..PluginManifest::default()
        };
        let workspace = ProposalWorkspace::new(&path);
        save_manifest(&workspace, &manifest)?;
        let repo = GitRepo::open(path);
        repo.snapshot_commit(
            "plugin: create draft",
            &self.author_name,
            &self.author_email,
        )?;
        Ok(PluginOperator {
            store: self,
            plugin_name: plugin_name.to_string(),
            manifest,
        })
    }

    /// List plugins represented by a readable root manifest.
    pub fn list(&self) -> Result<Vec<PluginManifest>> {
        let entries = match std::fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => return Err(source.into()),
        };
        let mut manifests = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path().join("manifest.toml");
            if !path.is_file() {
                continue;
            }
            let text = std::fs::read_to_string(&path).map_err(|source| Error::ReadFile {
                path: path.clone(),
                source,
            })?;
            let manifest: PluginManifest =
                toml::from_str(&text).map_err(|source| Error::ParseToml {
                    path: path.clone(),
                    source,
                })?;
            let expected = entry.file_name().to_string_lossy().to_string();
            if manifest.plugin_name != expected {
                return Err(Error::Validation(format!(
                    "manifest plugin_name `{}` does not match directory `{expected}`",
                    manifest.plugin_name
                )));
            }
            manifests.push(manifest);
        }
        manifests.sort_by(|a, b| a.plugin_name.cmp(&b.plugin_name));
        Ok(manifests)
    }

    /// Open one existing plugin workspace.
    pub fn develop(&self, plugin_name: &str) -> Result<Option<PluginOperator<'_>>> {
        let path = self.plugin_path(plugin_name);
        if !path.join("manifest.toml").is_file() {
            return Ok(None);
        }
        let workspace = ProposalWorkspace::new(path);
        let manifest = load_manifest(&workspace)?;
        Ok(Some(PluginOperator {
            store: self,
            plugin_name: plugin_name.to_string(),
            manifest,
        }))
    }

    fn plugin_path(&self, plugin_name: &str) -> PathBuf {
        self.root.join(plugin_name)
    }
}

impl PluginOperator<'_> {
    /// Reload the authoritative manifest from disk.
    pub fn refresh(&mut self) -> Result<()> {
        self.manifest = load_manifest(&self.workspace())?;
        Ok(())
    }

    pub fn plugin_name(&self) -> &str {
        &self.plugin_name
    }

    pub fn status(&self) -> PluginStatus {
        self.manifest.status
    }

    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    /// Return the plugin's own git-backed safe workspace.
    pub fn workspace(&self) -> ProposalWorkspace {
        ProposalWorkspace::new(self.store.plugin_path(&self.plugin_name))
    }

    /// Bind a different approved environment while development is editable.
    pub fn bind_environment(
        &mut self,
        environment_id: &str,
        catalog: &EnvironmentCatalog,
    ) -> Result<()> {
        self.ensure_editable()?;
        let environment = catalog.get(environment_id).ok_or_else(|| {
            Error::Validation(format!(
                "environment {environment_id:?} is not approved for RSI"
            ))
        })?;
        self.manifest.image.reference =
            ImageReference::parse(&environment.reference).map_err(|error| {
                Error::Validation(format!("invalid environment reference: {error}"))
            })?;
        save_manifest(&self.workspace(), &self.manifest)
    }

    /// Commit the current in-place development state.
    pub fn snapshot(&mut self, message: &str) -> Result<Option<String>> {
        let repo = GitRepo::open(self.store.plugin_path(&self.plugin_name));
        repo.snapshot_commit(message, &self.store.author_name, &self.store.author_email)
    }

    /// Transition the manifest lifecycle marker.
    pub fn transition(&mut self, to: PluginStatus) -> Result<PluginStatus> {
        let from = self.manifest.status;
        ensure_transition(from, to)?;
        self.manifest.status = to;
        save_manifest(&self.workspace(), &self.manifest)?;
        Ok(to)
    }

    /// Bind one agent directly to this plugin workspace.
    ///
    /// There is no detached candidate copy. Exclusive agent leasing and the
    /// manifest's editable status are the development guards.
    pub fn bind_agent(&mut self, agent_id: &str, run_id: &str) -> Result<()> {
        self.ensure_editable()?;
        let environment_reference = self.manifest.image.reference.as_str();
        PluginDevelopmentToolsetRegistry::global().bind_plugin_workspace(
            agent_id,
            &self.plugin_name,
            self.store.plugin_path(&self.plugin_name),
            &environment_reference,
            run_id,
        )?;
        Ok(())
    }

    fn ensure_editable(&self) -> Result<()> {
        if is_editable(self.manifest.status) {
            Ok(())
        } else {
            Err(Error::Validation(format!(
                "plugin `{}` cannot be developed from manifest status {:?}",
                self.plugin_name, self.manifest.status
            )))
        }
    }
}

fn load_manifest(workspace: &ProposalWorkspace) -> Result<PluginManifest> {
    let path = workspace.path().join("manifest.toml");
    let text = std::fs::read_to_string(&path).map_err(|source| Error::ReadFile {
        path: path.clone(),
        source,
    })?;
    toml::from_str(&text).map_err(|source| Error::ParseToml { path, source })
}

fn save_manifest(workspace: &ProposalWorkspace, manifest: &PluginManifest) -> Result<()> {
    let text = toml::to_string_pretty(manifest).map_err(|source| Error::SerializeToml {
        path: workspace.path().join("manifest.toml"),
        source,
    })?;
    workspace.write_text("manifest.toml", &text)
}

fn validate_requests(
    request_ids: &[String],
    requests: &RequestStore,
    rationale: &str,
) -> Result<()> {
    let requested = BTreeSet::from_iter(request_ids.iter().cloned());
    if requested.is_empty() {
        return Err(Error::InvalidRequest(
            "at least one request id is required".into(),
        ));
    }
    let missing = requests.missing_ids(&requested);
    if !missing.is_empty() {
        return Err(Error::InvalidRequest(format!(
            "unknown request ids: {}",
            missing.join(", ")
        )));
    }
    if rationale.trim().is_empty() {
        return Err(Error::InvalidRequest("rationale is required".into()));
    }
    Ok(())
}

pub(crate) fn is_editable(status: PluginStatus) -> bool {
    matches!(
        status,
        PluginStatus::Draft | PluginStatus::NeedsFix | PluginStatus::PublishFailed
    )
}

pub(crate) fn ensure_transition(from: PluginStatus, to: PluginStatus) -> Result<()> {
    let valid = match (from, to) {
        (PluginStatus::Draft, PluginStatus::Validating)
        | (PluginStatus::NeedsFix, PluginStatus::Validating)
        | (PluginStatus::Validating, PluginStatus::NeedsFix)
        | (PluginStatus::Validating, PluginStatus::PendingReview)
        | (PluginStatus::PendingReview, PluginStatus::Approved)
        | (PluginStatus::PendingReview, PluginStatus::Rejected)
        | (PluginStatus::Approved, PluginStatus::Publishing)
        | (PluginStatus::Publishing, PluginStatus::Published)
        | (PluginStatus::Publishing, PluginStatus::PullRequestOpen)
        | (PluginStatus::Publishing, PluginStatus::PublishFailed)
        | (PluginStatus::PublishFailed, PluginStatus::Validating)
        | (PluginStatus::PullRequestOpen, PluginStatus::PullRequestMerged)
        | (PluginStatus::Published, PluginStatus::InstallPending)
        | (PluginStatus::PullRequestMerged, PluginStatus::InstallPending)
        | (PluginStatus::InstallPending, PluginStatus::Installed) => true,
        (from, to) if from == to => true,
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidTransition {
            from: format!("{from:?}").to_lowercase(),
            to: format!("{to:?}").to_lowercase(),
        })
    }
}
