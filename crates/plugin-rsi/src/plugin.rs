//! Long-lived, plugin-scoped development workspaces.
//!
//! A plugin directory is both the git repository and the lifecycle record.
//! Requests point at it, agents edit it in place, and `manifest.toml.status`
//! is the daemon-owned state marker. This avoids separate development repositories
//! copies in the direct-development path.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use container_plugin::manifest::{
    ImageMetadata, PluginInstallationMetadata, PluginInstallationStatus, PluginLifecycleMetadata,
    PluginManifest, PluginStatus,
};
use container_runtime::ImageReference;

use crate::{
    Error, GitInstalledPluginSource, GitRepo, InstalledPluginSource, LocalInstalledPluginSource,
    PluginStateLayout, PluginWorkspace, RequestStore, Result, validate::EnvironmentCatalog,
};

/// Creates and opens plugin repositories under one root.
#[derive(Debug, Clone)]
pub struct PluginStore {
    layout: PluginStateLayout,
    workspace_root: PathBuf,
    runtime_root: PathBuf,
    snapshot_root: PathBuf,
    registry_path: PathBuf,
    default_branch: String,
    author_name: String,
    author_email: String,
}

/// Stable virtual mount containing every long-lived plugin workspace.
pub const PLUGIN_DEVELOPMENT_VFS_ROOT: &str = "/plugins/dev";

/// Materializes an installed source into the unified plugin root.
pub trait PluginSourceFetcher {
    fn fetch(&self, source: &InstalledPluginSource, destination: &Path) -> Result<()>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct GitPluginSourceFetcher;

impl PluginSourceFetcher for GitPluginSourceFetcher {
    fn fetch(&self, source: &InstalledPluginSource, destination: &Path) -> Result<()> {
        match source {
            InstalledPluginSource::Git(source) => {
                GitRepo::clone_at(destination, &source.remote, &source.commit, "origin")?;
                Ok(())
            }
            InstalledPluginSource::Local(_) => Err(Error::Validation(
                "local snapshots use the existing development workspace".into(),
            )),
        }
    }
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
        let layout = PluginStateLayout::open(state_dir);
        Self::open_with_layout(layout, default_branch, author_name, author_email)
    }

    /// Open with an explicit resolved state layout.
    pub fn open_with_layout(
        layout: PluginStateLayout,
        default_branch: &str,
        author_name: &str,
        author_email: &str,
    ) -> Self {
        Self {
            workspace_root: layout.workspace_root(),
            runtime_root: layout.runtime_root(),
            snapshot_root: layout.snapshot_root(),
            registry_path: layout.registry_path(),
            layout,
            default_branch: default_branch.to_string(),
            author_name: author_name.to_string(),
            author_email: author_email.to_string(),
        }
    }

    /// Return the daemon-owned plugin root.
    pub fn root(&self) -> &Path {
        &self.workspace_root
    }

    /// Return the VFS address for one development workspace.
    pub fn development_vfs_path(&self, plugin_name: &str) -> Result<String> {
        crate::validate_plugin_name(plugin_name)?;
        Ok(format!("{PLUGIN_DEVELOPMENT_VFS_ROOT}/{plugin_name}"))
    }

    /// Return the immutable runtime materialization root.
    pub fn runtime_root(&self) -> &Path {
        &self.runtime_root
    }

    /// Return the persistent registry that pins installed plugin sources.
    pub fn registry_path(&self) -> &Path {
        &self.registry_path
    }

    /// Return the resolved state-directory layout.
    pub fn layout(&self) -> &PluginStateLayout {
        &self.layout
    }

    /// Materialize the persistent registry into the plugin root.
    ///
    /// `container-plugin` provides the low-level checkout tooling; this method
    /// is the only system entry point that decides the root and protects an
    /// in-place update from being reset to the old pin.
    pub fn materialize_registry(&self) -> Result<container_plugin::sync::SyncReport> {
        container_plugin::sync::sync(&self.registry_path, &self.runtime_root)
            .map_err(|error| Error::PluginRegistry(error.to_string()))
    }

    /// Read the persistent source declarations indexed by plugin name.
    pub fn registry_sources(
        &self,
    ) -> Result<BTreeMap<String, container_plugin::sync::PluginSource>> {
        let text = match std::fs::read_to_string(&self.registry_path) {
            Ok(text) => text,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Default::default());
            }
            Err(source) => {
                return Err(Error::ReadFile {
                    path: self.registry_path.clone(),
                    source,
                });
            }
        };
        let config: container_plugin::sync::PluginsConfig =
            toml::from_str(&text).map_err(|source| Error::ParseToml {
                path: self.registry_path.clone(),
                source,
            })?;
        Ok(config
            .plugin
            .into_iter()
            .map(|source| (source.name.clone(), source))
            .collect())
    }

    /// Pin one installed plugin in the persistent registry.
    pub fn install_source(&self, plugin_name: &str, remote: &str, commit: &str) -> Result<()> {
        crate::write_git_plugin_source(&self.registry_path, plugin_name, remote, commit)
    }

    /// Read one plugin's immutable installed source.
    pub fn installed_source(&self, plugin_name: &str) -> Result<InstalledPluginSource> {
        crate::read_installed_plugin_source(&self.registry_path, plugin_name)
    }

    /// Uninstall the active runtime source while preserving its audit history.
    ///
    /// The development workspace and immutable snapshots remain on disk. Only
    /// the persistent registry declaration and its current runtime
    /// materialization are removed.
    pub fn uninstall(&self, plugin_name: &str) -> Result<InstalledPluginSource> {
        crate::validate_plugin_name(plugin_name)?;
        let source = self.installed_source(plugin_name)?;
        let mut operator = self
            .develop(plugin_name)?
            .ok_or_else(|| Error::Validation(format!("unknown plugin `{plugin_name}`")))?;
        let repository = operator.repository();
        if !repository.is_clean()? {
            return Err(Error::Validation(format!(
                "plugin `{plugin_name}` has uncommitted development changes; \
                 commit or discard them before uninstalling"
            )));
        }

        operator.mutate_manifest(|manifest| {
            manifest.installation = Default::default();
            manifest.lifecycle.publication_pending = false;
        })?;
        operator.snapshot("plugin: uninstall runtime source")?;

        let runtime_path = self.runtime_root.join(plugin_name);
        match std::fs::symlink_metadata(&runtime_path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                std::fs::remove_file(&runtime_path)?;
            }
            Ok(_) => std::fs::remove_dir_all(&runtime_path)?,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(source.into()),
        }
        if !crate::remove_installed_plugin_source(&self.registry_path, plugin_name)? {
            return Err(Error::Validation(format!(
                "plugin `{plugin_name}` disappeared from the registry during uninstall"
            )));
        }

        self.materialize_registry()?;
        Ok(source)
    }

    /// Install the clean, validated workspace commit as a local snapshot.
    ///
    /// The development workspace remains mutable and keeps its development
    /// status; the DAG always reads the immutable snapshot selected here.
    pub fn install_local(&self, plugin_name: &str) -> Result<LocalInstalledPluginSource> {
        let mut operator = self
            .develop(plugin_name)?
            .ok_or_else(|| Error::Validation(format!("unknown plugin `{plugin_name}`")))?;
        if !(is_editable(operator.status())
            || matches!(
                operator.status(),
                PluginStatus::PendingReview
                    | PluginStatus::Approved
                    | PluginStatus::Published
                    | PluginStatus::PullRequestMerged
            ))
        {
            return Err(Error::Validation(format!(
                "plugin `{plugin_name}` cannot be locally installed from development status {:?}",
                operator.status()
            )));
        }
        let repository = operator.repository();
        if !repository.is_clean()? {
            return Err(Error::Validation(
                "plugin workspace has uncommitted content".into(),
            ));
        }
        let latest_report = operator
            .manifest()
            .lifecycle
            .latest_report
            .clone()
            .ok_or_else(|| Error::Validation("plugin has no validation report".into()))?;
        let report_path = repository.path().join(&latest_report);
        let report_text =
            std::fs::read_to_string(&report_path).map_err(|source| Error::ReadFile {
                path: report_path.clone(),
                source,
            })?;
        let report: crate::ValidationReport = serde_json::from_str(&report_text)
            .map_err(|source| Error::Validation(format!("invalid report JSON: {source}")))?;
        if !report.passed() {
            return Err(Error::Validation(
                "plugin's latest validation report did not pass".into(),
            ));
        }

        operator.mutate_manifest(|manifest| {
            manifest.lifecycle.publication_pending = true;
        })?;
        operator.snapshot("plugin: freeze local activation")?;

        let commit = repository.head()?;
        let tree_digest = repository.tree_digest()?;
        let digest = format!("sha256:{tree_digest}");
        let destination = self.snapshot_root.join(plugin_name).join(&tree_digest);
        if destination.exists() {
            let existing = destination.join("manifest.toml");
            if !existing.is_file() {
                return Err(Error::Validation(format!(
                    "local snapshot directory is incomplete: {}",
                    destination.display()
                )));
            }
        } else {
            GitRepo::clone_at(
                &destination,
                &repository.path().to_string_lossy(),
                &commit,
                "origin",
            )?;
            std::fs::remove_dir_all(destination.join(".git"))?;
        }

        let workspace = PluginWorkspace::new(&destination);
        let mut manifest = load_manifest(&workspace)?;
        manifest.installation = PluginInstallationMetadata {
            status: Some(PluginInstallationStatus::LocalActive),
            local_commit: Some(commit.clone()),
            local_digest: Some(digest.clone()),
            ..Default::default()
        };
        save_manifest(&workspace, &manifest)?;
        crate::write_local_plugin_source(
            &self.registry_path,
            plugin_name,
            &destination,
            &commit,
            &digest,
        )?;
        self.materialize_registry()?;
        Ok(LocalInstalledPluginSource {
            path: destination,
            commit,
            digest,
        })
    }

    /// Roll an updated plugin back to the immutable revision it replaced.
    pub fn rollback(&self, plugin_name: &str) -> Result<InstalledPluginSource> {
        let current = self.installed_source(plugin_name)?;
        let mut operator = self
            .develop(plugin_name)?
            .ok_or_else(|| Error::Validation(format!("plugin `{plugin_name}` is not present")))?;
        let lifecycle = &operator.manifest().lifecycle;
        let Some(base_commit) = lifecycle.base_commit.clone() else {
            return Err(Error::Validation(format!(
                "plugin `{plugin_name}` has no rollback base"
            )));
        };
        let Some(base_remote) = lifecycle.base_remote.clone() else {
            return Err(Error::Validation(format!(
                "plugin `{plugin_name}` has no rollback remote"
            )));
        };
        let InstalledPluginSource::Git(current) = current else {
            return Err(Error::Validation(
                "local plugin rollback history is not implemented yet".into(),
            ));
        };
        if base_remote != current.remote || current.commit == base_commit {
            return Err(Error::Validation(
                "plugin has no prior source to roll back to".into(),
            ));
        }
        let repository = operator.repository();
        if !repository.is_clean()? || repository.head()? != current.commit {
            return Err(Error::Validation(
                "plugin workspace must be clean at the installed revision before rollback".into(),
            ));
        }
        repository.checkout_commit(&base_commit)?;
        repository.switch_forced_branch(&self.default_branch)?;
        self.install_source(plugin_name, &base_remote, &base_commit)?;
        operator.refresh()?;
        Ok(InstalledPluginSource::Git(GitInstalledPluginSource {
            remote: base_remote,
            commit: base_commit,
        }))
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
            lifecycle: PluginLifecycleMetadata {
                request_ids: request_ids.to_vec(),
                rationale: Some(rationale.trim().to_string()),
                ..Default::default()
            },
            image,
            ..PluginManifest::default()
        };
        let workspace = PluginWorkspace::new(&path);
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

    /// Fork an installed reference plugin into a new development workspace.
    ///
    /// The fork keeps the reference's source, tests, and environment while
    /// resetting daemon-owned lifecycle facts. This is the fast development
    /// path: agents change the fork and can activate it locally before the
    /// background distiller publishes it.
    pub fn fork(
        &self,
        reference_plugin_name: &str,
        plugin_name: &str,
        request_ids: &[String],
        rationale: &str,
        requests: &RequestStore,
        catalog: &EnvironmentCatalog,
    ) -> Result<PluginOperator<'_>> {
        crate::validate_plugin_name(reference_plugin_name)?;
        crate::validate_plugin_name(plugin_name)?;
        if reference_plugin_name == plugin_name {
            return Err(Error::InvalidRequest(
                "a plugin fork must use a new plugin name".into(),
            ));
        }
        validate_requests(request_ids, requests, rationale)?;
        let source = self.installed_source(reference_plugin_name)?;
        let path = self.plugin_path(plugin_name);
        if path.exists() {
            return Err(Error::InvalidRequest(format!(
                "plugin directory already exists: {}",
                path.display()
            )));
        }

        let source_commit = source.commit().to_string();
        match &source {
            InstalledPluginSource::Git(source) => {
                GitRepo::clone_at(&path, &source.remote, &source.commit, "origin")?;
            }
            InstalledPluginSource::Local(source) => {
                copy_plugin_tree(&source.path, &path)?;
                GitRepo::init(&path, &self.default_branch)?;
            }
        }

        let workspace = PluginWorkspace::new(&path);
        let mut manifest = load_manifest(&workspace)?;
        let environment_reference = manifest.image.reference.as_str();
        if catalog.find_reference(&environment_reference).is_none() {
            return Err(Error::Validation(
                "reference environment is not approved for RSI".into(),
            ));
        }
        manifest.plugin_name = plugin_name.to_string();
        manifest.status = PluginStatus::Draft;
        manifest.installation = Default::default();
        manifest.lifecycle = PluginLifecycleMetadata {
            source_plugin: Some(reference_plugin_name.to_string()),
            source_commit: Some(source_commit),
            request_ids: request_ids.to_vec(),
            rationale: Some(rationale.trim().to_string()),
            ..Default::default()
        };
        save_manifest(&workspace, &manifest)?;

        let repository = GitRepo::open(&path);
        repository.remove_remote("origin")?;
        repository.switch_forced_branch(&self.default_branch)?;
        repository.snapshot_commit(
            "plugin: fork installed reference",
            &self.author_name,
            &self.author_email,
        )?;
        Ok(PluginOperator {
            store: self,
            plugin_name: plugin_name.to_string(),
            manifest,
        })
    }

    /// Materialize an installed plugin for in-place update.
    pub fn create_update(
        &self,
        plugin_name: &str,
        request_ids: &[String],
        rationale: &str,
        requests: &RequestStore,
        source: &InstalledPluginSource,
        catalog: &EnvironmentCatalog,
        fetcher: &dyn PluginSourceFetcher,
    ) -> Result<PluginOperator<'_>> {
        crate::validate_plugin_name(plugin_name)?;
        validate_requests(request_ids, requests, rationale)?;
        let path = self.plugin_path(plugin_name);
        if path.exists() {
            let mut operator = self
                .develop(plugin_name)?
                .ok_or_else(|| Error::Validation("plugin workspace has no manifest".into()))?;
            let environment_reference = operator.manifest.image.reference.as_str();
            if catalog.find_reference(&environment_reference).is_none() {
                return Err(Error::Validation(
                    "installed environment is not approved for RSI".into(),
                ));
            }
            match source {
                InstalledPluginSource::Git(source) => {
                    if operator.repository().remote_url("origin")?.as_deref()
                        != Some(source.remote.as_str())
                    {
                        return Err(Error::Validation(
                            "installed workspace does not match the requested source".into(),
                        ));
                    }
                }
                InstalledPluginSource::Local(_) => {}
            }
            operator.mutate_manifest(|manifest| {
                manifest.lifecycle.request_ids = request_ids.to_vec();
                manifest.lifecycle.rationale = Some(rationale.trim().to_string());
                manifest.lifecycle.publication_pending = false;
            })?;
            if operator.status() == PluginStatus::Updating {
                operator.snapshot("plugin: attach update feedback")?;
                return Ok(operator);
            }
            operator.mutate_manifest(|manifest| {
                manifest.lifecycle.base_commit = Some(source.commit().to_string());
                manifest.lifecycle.base_remote = match source {
                    InstalledPluginSource::Git(source) => Some(source.remote.clone()),
                    InstalledPluginSource::Local(_) => None,
                };
            })?;
            operator
                .transition_snapshot(PluginStatus::Updating, "plugin: start in-place update")?;
            return Ok(operator);
        }
        fetcher.fetch(source, &path)?;
        let workspace = PluginWorkspace::new(&path);
        let mut manifest = load_manifest(&workspace)?;
        if manifest.plugin_name != plugin_name {
            return Err(Error::Validation(format!(
                "installed manifest plugin_name `{}` does not match `{plugin_name}`",
                manifest.plugin_name
            )));
        }
        let environment_reference = manifest.image.reference.as_str();
        if catalog.find_reference(&environment_reference).is_none() {
            return Err(Error::Validation(
                "installed environment is not approved for RSI".into(),
            ));
        }
        manifest.status = PluginStatus::Updating;
        manifest.lifecycle.request_ids = request_ids.to_vec();
        manifest.lifecycle.rationale = Some(rationale.trim().to_string());
        manifest.lifecycle.base_commit = Some(source.commit().to_string());
        manifest.lifecycle.base_remote = match source {
            InstalledPluginSource::Git(source) => Some(source.remote.clone()),
            InstalledPluginSource::Local(_) => None,
        };
        let mut operator = PluginOperator {
            store: self,
            plugin_name: plugin_name.to_string(),
            manifest,
        };
        operator.mutate_manifest(|manifest| manifest.status = PluginStatus::Updating)?;
        operator.snapshot("plugin: start update")?;
        Ok(operator)
    }

    /// List plugins represented by a readable root manifest.
    pub fn list(&self) -> Result<Vec<PluginManifest>> {
        let entries = match std::fs::read_dir(self.root()) {
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
        let sources = self.registry_sources()?;
        for manifest in &mut manifests {
            match sources.get(&manifest.plugin_name) {
                Some(source) if source.git.is_some() => {
                    manifest.installation.status = Some(PluginInstallationStatus::GithubActive);
                    manifest.installation.remote = source.git.clone();
                    manifest.installation.remote_commit = source.rev.clone();
                }
                Some(source) if source.path.is_some() => {
                    manifest.installation.status = Some(PluginInstallationStatus::LocalActive);
                    manifest.installation.local_commit = source.local_commit.clone();
                    manifest.installation.local_digest = source.local_digest.clone();
                }
                _ => {}
            }
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
        let workspace = PluginWorkspace::new(path);
        let manifest = load_manifest(&workspace)?;
        Ok(Some(PluginOperator {
            store: self,
            plugin_name: plugin_name.to_string(),
            manifest,
        }))
    }

    /// Return the plugin that owns a node kind, if any.
    pub fn owner_of_node_kind(&self, node_kind: &str) -> Result<Option<String>> {
        let matches = self
            .list()?
            .into_iter()
            .filter(|manifest| {
                manifest.nodes.iter().any(|node| {
                    node.kind == node_kind
                        || format!("{}/{}", manifest.plugin_name, node.kind) == node_kind
                })
            })
            .map(|manifest| manifest.plugin_name)
            .collect::<Vec<_>>();
        Ok(matches.first().cloned())
    }

    /// Return local-active plugins awaiting the background publication pass.
    ///
    /// A foreground agent only needs to reach this state to use its result in
    /// the DAG. Review, upstream merge, and GitHub publication can happen later
    /// without blocking feedback-driven development.
    pub fn pending_distillation(&self) -> Result<Vec<PluginManifest>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|manifest| {
                manifest.lifecycle.publication_pending && manifest.installation.is_runtime_active()
            })
            .collect())
    }

    fn plugin_path(&self, plugin_name: &str) -> PathBuf {
        self.workspace_root.join(plugin_name)
    }
}

impl PluginOperator<'_> {
    /// Reload the authoritative manifest from disk.
    pub fn refresh(&mut self) -> Result<()> {
        self.manifest = load_manifest(&self.workspace())?;
        Ok(())
    }

    /// Return this workspace's stable VFS address.
    pub fn development_vfs_path(&self) -> Result<String> {
        self.store.development_vfs_path(&self.plugin_name)
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

    /// Node kinds whose registry collisions are owned by this workspace.
    pub fn owned_node_kinds(&self) -> Vec<String> {
        self.manifest
            .nodes
            .iter()
            .map(|node| node.kind.clone())
            .collect()
    }

    pub(crate) fn repository(&self) -> GitRepo {
        GitRepo::open(self.store.plugin_path(&self.plugin_name))
    }

    pub(crate) fn plugin_path(&self) -> PathBuf {
        self.store.plugin_path(&self.plugin_name)
    }

    pub(crate) fn install_registry_source(&self, remote: &str, commit: &str) -> Result<()> {
        self.store.install_source(&self.plugin_name, remote, commit)
    }

    /// Append-only reports live beside the manifest but outside node loading.
    pub fn reports_dir(&self) -> PathBuf {
        self.plugin_path().join(".rsi").join("reports")
    }

    /// Return the plugin's own git-backed safe workspace.
    pub fn workspace(&self) -> PluginWorkspace {
        PluginWorkspace::new(self.store.plugin_path(&self.plugin_name))
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
        self.manifest.lifecycle.publication_pending = false;
        save_manifest(&self.workspace(), &self.manifest)
    }

    /// Commit the current in-place development state.
    pub fn snapshot(&mut self, message: &str) -> Result<Option<String>> {
        let repo = GitRepo::open(self.store.plugin_path(&self.plugin_name));
        repo.snapshot_commit(message, &self.store.author_name, &self.store.author_email)
    }

    /// Persist one host-owned lifecycle mutation without committing it.
    pub(crate) fn mutate_manifest(
        &mut self,
        mutate: impl FnOnce(&mut PluginManifest),
    ) -> Result<()> {
        let before = self.manifest.status;
        mutate(&mut self.manifest);
        let after = self.manifest.status;
        if before != after {
            ensure_transition(before, after)?;
        }
        save_manifest(&self.workspace(), &self.manifest)
    }

    /// Transition and commit one lifecycle marker change.
    pub fn transition_snapshot(
        &mut self,
        to: PluginStatus,
        message: &str,
    ) -> Result<Option<String>> {
        self.mutate_manifest(|manifest| manifest.status = to)?;
        self.snapshot(message)
    }

    /// Transition the manifest lifecycle marker.
    pub fn transition(&mut self, to: PluginStatus) -> Result<PluginStatus> {
        let from = self.manifest.status;
        ensure_transition(from, to)?;
        self.manifest.status = to;
        save_manifest(&self.workspace(), &self.manifest)?;
        Ok(to)
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

fn load_manifest(workspace: &PluginWorkspace) -> Result<PluginManifest> {
    let path = workspace.path().join("manifest.toml");
    let text = std::fs::read_to_string(&path).map_err(|source| Error::ReadFile {
        path: path.clone(),
        source,
    })?;
    toml::from_str(&text).map_err(|source| Error::ParseToml { path, source })
}

fn save_manifest(workspace: &PluginWorkspace, manifest: &PluginManifest) -> Result<()> {
    let text = toml::to_string_pretty(manifest).map_err(|source| Error::SerializeToml {
        path: workspace.path().join("manifest.toml"),
        source,
    })?;
    workspace.write_text("manifest.toml", &text)
}

fn copy_plugin_tree(source: &Path, destination: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        return Err(Error::UnsafePath {
            path: source.display().to_string(),
        });
    }
    if metadata.is_dir() {
        std::fs::create_dir_all(destination)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            copy_plugin_tree(&entry.path(), &destination.join(entry.file_name()))?;
        }
        return Ok(());
    }
    if !metadata.is_file() {
        return Err(Error::Validation(format!(
            "unsupported plugin file type: {}",
            source.display()
        )));
    }
    std::fs::copy(source, destination)
        .map(|_| ())
        .map_err(Error::Io)
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
        PluginStatus::Draft
            | PluginStatus::Updating
            | PluginStatus::NeedsFix
            | PluginStatus::PublishFailed
    )
}

pub(crate) fn ensure_transition(from: PluginStatus, to: PluginStatus) -> Result<()> {
    let valid = match (from, to) {
        (PluginStatus::Draft, PluginStatus::Validating)
        | (PluginStatus::Draft, PluginStatus::Updating)
        | (PluginStatus::Installed, PluginStatus::Updating)
        | (PluginStatus::Updating, PluginStatus::Validating)
        | (PluginStatus::NeedsFix, PluginStatus::Validating)
        | (PluginStatus::Validating, PluginStatus::NeedsFix)
        | (PluginStatus::Validating, PluginStatus::PendingReview)
        | (PluginStatus::Validating, PluginStatus::Draft)
        | (PluginStatus::Validating, PluginStatus::Updating)
        | (PluginStatus::Draft, PluginStatus::PendingReview)
        | (PluginStatus::Updating, PluginStatus::PendingReview)
        | (PluginStatus::PendingReview, PluginStatus::Updating)
        | (PluginStatus::PendingReview, PluginStatus::Approved)
        | (PluginStatus::PendingReview, PluginStatus::Rejected)
        | (PluginStatus::Approved, PluginStatus::Publishing)
        | (PluginStatus::Publishing, PluginStatus::Published)
        | (PluginStatus::Publishing, PluginStatus::PullRequestOpen)
        | (PluginStatus::Publishing, PluginStatus::PublishFailed)
        | (PluginStatus::PublishFailed, PluginStatus::Updating)
        | (PluginStatus::Published, PluginStatus::Updating)
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
