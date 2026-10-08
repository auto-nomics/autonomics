//! Long-lived, environment-scoped development workspaces.
//!
//! A directory under `<state_dir>/environments/dev/<id>/` is both the git
//! repository and the lifecycle record, mirroring the plugin store. The one
//! structural difference: the build artifact is already content-addressed by
//! its image digest, so local activation registers that digest in the
//! environment catalog instead of snapshotting a tree.

use std::path::{Path, PathBuf};

use container_runtime::ImageReference;

use crate::{
    Environment, EnvironmentRegistry, Error, GitRepo, PluginWorkspace, Result,
    env_manifest::{
        EnvironmentBase, EnvironmentLifecycleMetadata, EnvironmentManifest, EnvironmentStatus,
        EnvironmentSmokeTest, ensure_environment_transition, environment_is_editable,
    },
    lifecycle::next_validation_attempt,
    plugin::{copy_plugin_tree, validate_requests},
};

/// Stable virtual mount containing every long-lived environment workspace.
pub const ENVIRONMENT_DEVELOPMENT_VFS_ROOT: &str = "/environments/dev";

/// Local registry namespace used before an image is pushed.
pub const DEFAULT_LOCAL_NAMESPACE: &str = "auto-nomics/environments";

/// A locally activated environment registered in the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledEnvironment {
    /// Digest-pinned `localhost/…` catalog reference.
    pub reference: String,
    pub digest: String,
}

/// Creates and opens environment repositories under one root.
#[derive(Debug, Clone)]
pub struct EnvironmentStore {
    root: PathBuf,
    default_branch: String,
    author_name: String,
    author_email: String,
    local_namespace: String,
}

/// A host-owned handle to one long-lived environment workspace.
#[derive(Debug)]
pub struct EnvironmentOperator<'a> {
    store: &'a EnvironmentStore,
    environment_id: String,
    manifest: EnvironmentManifest,
}

impl EnvironmentStore {
    /// Open `<state_dir>/environments/dev` as the environment workspace root.
    pub fn open(
        state_dir: &Path,
        default_branch: &str,
        author_name: &str,
        author_email: &str,
        local_namespace: &str,
    ) -> Self {
        Self {
            root: state_dir.join("environments").join("dev"),
            default_branch: default_branch.to_string(),
            author_name: author_name.to_string(),
            author_email: author_email.to_string(),
            local_namespace: local_namespace.to_string(),
        }
    }

    /// Return the daemon-owned environment root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Return the local registry namespace used for build tags.
    pub fn local_namespace(&self) -> &str {
        &self.local_namespace
    }

    /// Return the VFS address for one development workspace.
    pub fn development_vfs_path(&self, environment_id: &str) -> Result<String> {
        crate::validate_plugin_name(environment_id)?;
        Ok(format!("{ENVIRONMENT_DEVELOPMENT_VFS_ROOT}/{environment_id}"))
    }

    /// Local build tag for one validation attempt.
    pub fn local_tag(&self, environment_id: &str, attempt: u32) -> Result<String> {
        crate::validate_plugin_name(environment_id)?;
        Ok(format!(
            "localhost/{}/{}:rsi-{attempt}",
            self.local_namespace, environment_id
        ))
    }

    /// Digest-pinned local catalog reference for one built image.
    pub fn local_reference(&self, environment_id: &str, digest: &str) -> Result<String> {
        crate::validate_plugin_name(environment_id)?;
        ImageReference::new(
            "localhost",
            &format!("{}/{}", self.local_namespace, environment_id),
            digest,
        )
        .map(|reference| reference.as_str())
        .map_err(|error| Error::Validation(format!("invalid local image reference: {error}")))
    }

    /// Create one named environment repository derived from an approved base.
    pub fn create(
        &self,
        environment_id: &str,
        base_environment_id: &str,
        request_ids: &[String],
        rationale: &str,
        requests: &crate::RequestStore,
        catalog: &crate::EnvironmentCatalog,
    ) -> Result<EnvironmentOperator<'_>> {
        crate::validate_plugin_name(environment_id)?;
        validate_requests(request_ids, requests, rationale)?;
        let base = catalog.get(base_environment_id).ok_or_else(|| {
            Error::Validation(format!(
                "environment {base_environment_id:?} is not approved for RSI"
            ))
        })?;
        ImageReference::parse(&base.reference)
            .map_err(|error| Error::Validation(format!("invalid base reference: {error}")))?;

        let path = self.environment_path(environment_id);
        if path.exists() {
            return Err(Error::InvalidRequest(format!(
                "environment directory already exists: {}",
                path.display()
            )));
        }
        GitRepo::init(&path, &self.default_branch)?;

        let workspace = PluginWorkspace::new(&path);
        // Seed the Containerfile with the approved digest-pinned FROM so a
        // fresh workspace starts structurally valid; agents rewrite it freely.
        workspace.write_text(
            "Containerfile",
            &format!("FROM {}\n", base.reference),
        )?;
        let manifest = EnvironmentManifest {
            environment_id: environment_id.to_string(),
            status: EnvironmentStatus::Draft,
            interpreters: base.interpreters.clone(),
            base: EnvironmentBase {
                reference: base.reference.clone(),
                ..Default::default()
            },
            containerfile: default_containerfile(),
            tests: vec![EnvironmentSmokeTest {
                name: "base-interpreters".into(),
                argv: vec!["true".into()],
                expected_stdout_contains: None,
            }],
            lifecycle: EnvironmentLifecycleMetadata {
                request_ids: request_ids.to_vec(),
                rationale: Some(rationale.trim().to_string()),
                ..Default::default()
            },
        };
        save_manifest(&workspace, &manifest)?;
        GitRepo::open(&path).snapshot_commit(
            "environment: create draft",
            &self.author_name,
            &self.author_email,
        )?;
        Ok(EnvironmentOperator {
            store: self,
            environment_id: environment_id.to_string(),
            manifest,
        })
    }

    /// Fork an existing environment workspace under a new id.
    ///
    /// The fork keeps the Containerfile, tests, and interpreters while
    /// resetting daemon-owned lifecycle facts.
    pub fn fork(
        &self,
        source_environment_id: &str,
        environment_id: &str,
        request_ids: &[String],
        rationale: &str,
        requests: &crate::RequestStore,
    ) -> Result<EnvironmentOperator<'_>> {
        crate::validate_plugin_name(source_environment_id)?;
        crate::validate_plugin_name(environment_id)?;
        if source_environment_id == environment_id {
            return Err(Error::InvalidRequest(
                "an environment fork must use a new environment id".into(),
            ));
        }
        validate_requests(request_ids, requests, rationale)?;
        let source = self
            .develop(source_environment_id)?
            .ok_or_else(|| {
                Error::Validation(format!(
                    "environment `{source_environment_id}` has no development workspace"
                ))
            })?
            .environment_path();
        let path = self.environment_path(environment_id);
        if path.exists() {
            return Err(Error::InvalidRequest(format!(
                "environment directory already exists: {}",
                path.display()
            )));
        }

        copy_plugin_tree(&source, &path)?;
        GitRepo::init(&path, &self.default_branch)?;
        let workspace = PluginWorkspace::new(&path);
        let mut manifest = load_manifest(&workspace)?;
        manifest.environment_id = environment_id.to_string();
        manifest.status = EnvironmentStatus::Draft;
        manifest.lifecycle = EnvironmentLifecycleMetadata {
            source_environment: Some(source_environment_id.to_string()),
            request_ids: request_ids.to_vec(),
            rationale: Some(rationale.trim().to_string()),
            ..Default::default()
        };
        save_manifest(&workspace, &manifest)?;
        GitRepo::open(&path).snapshot_commit(
            "environment: fork development workspace",
            &self.author_name,
            &self.author_email,
        )?;
        Ok(EnvironmentOperator {
            store: self,
            environment_id: environment_id.to_string(),
            manifest,
        })
    }

    /// Start an in-place update of a catalog-active environment.
    ///
    /// The workspace must already exist: unlike plugin sources, an image
    /// cannot be materialized back into a repository, so updates always
    /// continue from the retained development workspace.
    pub fn create_update(
        &self,
        environment_id: &str,
        request_ids: &[String],
        rationale: &str,
        requests: &crate::RequestStore,
        registry: &EnvironmentRegistry,
    ) -> Result<EnvironmentOperator<'_>> {
        crate::validate_plugin_name(environment_id)?;
        validate_requests(request_ids, requests, rationale)?;
        let mut operator = self
            .develop(environment_id)?
            .ok_or_else(|| {
                Error::Validation(format!(
                    "environment `{environment_id}` has no development workspace; \
                     create one with environment_create or environment_fork"
                ))
            })?;
        let active = registry.get(environment_id)?.ok_or_else(|| {
            Error::Validation(format!(
                "environment `{environment_id}` is not active in the catalog"
            ))
        })?;

        operator.mutate_manifest(|manifest| {
            manifest.lifecycle.request_ids = request_ids.to_vec();
            manifest.lifecycle.rationale = Some(rationale.trim().to_string());
            manifest.lifecycle.publication_pending = false;
        })?;
        if operator.status() == EnvironmentStatus::Updating {
            operator.snapshot("environment: attach update feedback")?;
            return Ok(operator);
        }
        operator.mutate_manifest(|manifest| {
            manifest.lifecycle.base_reference = Some(active.reference.clone());
        })?;
        operator.transition_snapshot(
            EnvironmentStatus::Updating,
            "environment: start in-place update",
        )?;
        Ok(operator)
    }

    /// List environments represented by a readable, correctly named manifest.
    ///
    /// Malformed or orphaned workspaces are skipped so background scans keep
    /// working; explicit operations still fail when opened.
    pub fn list(&self) -> Result<Vec<EnvironmentManifest>> {
        let entries = match std::fs::read_dir(self.root()) {
            Ok(entries) => entries,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => return Err(source.into()),
        };
        let mut manifests = Vec::new();
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|file_type| file_type.is_dir()) {
                continue;
            }
            let path = entry.path().join("manifest.toml");
            if !path.is_file() {
                continue;
            }
            let parsed = std::fs::read_to_string(&path)
                .map_err(|source| Error::ReadFile {
                    path: path.clone(),
                    source,
                })
                .and_then(|text| {
                    toml::from_str::<EnvironmentManifest>(&text).map_err(|source| {
                        Error::ParseToml {
                            path: path.clone(),
                            source,
                        }
                    })
                });
            let manifest = match parsed {
                Ok(manifest) => manifest,
                Err(_) => continue,
            };
            let expected = entry.file_name().to_string_lossy().to_string();
            if manifest.environment_id != expected {
                continue;
            }
            manifests.push(manifest);
        }
        manifests.sort_by(|a, b| a.environment_id.cmp(&b.environment_id));
        Ok(manifests)
    }

    /// Open one existing environment workspace.
    pub fn develop(&self, environment_id: &str) -> Result<Option<EnvironmentOperator<'_>>> {
        let path = self.environment_path(environment_id);
        if !path.join("manifest.toml").is_file() {
            return Ok(None);
        }
        let workspace = PluginWorkspace::new(path);
        let manifest = load_manifest(&workspace)?;
        if manifest.environment_id != environment_id {
            return Err(Error::Validation(format!(
                "manifest environment_id `{}` does not match workspace `{environment_id}`",
                manifest.environment_id
            )));
        }
        Ok(Some(EnvironmentOperator {
            store: self,
            environment_id: environment_id.to_string(),
            manifest,
        }))
    }

    /// Install the latest validated build into the environment catalog.
    ///
    /// The image was already built by the validation gate; its digest is the
    /// content address, so activation is a catalog approval rather than a
    /// snapshot copy. The previous catalog reference (when any) is retained
    /// in `lifecycle.base_reference` for rollback.
    pub fn install_local(
        &self,
        environment_id: &str,
        registry: &EnvironmentRegistry,
    ) -> Result<InstalledEnvironment> {
        let mut operator = self.develop(environment_id)?.ok_or_else(|| {
            Error::Validation(format!("unknown environment `{environment_id}`"))
        })?;
        if !(environment_is_editable(operator.status())
            || matches!(
                operator.status(),
                EnvironmentStatus::PendingReview | EnvironmentStatus::Approved
            ))
        {
            return Err(Error::Validation(format!(
                "environment `{environment_id}` cannot be locally installed from status {:?}",
                operator.status()
            )));
        }
        let repository = operator.repository();
        if !repository.is_clean()? {
            return Err(Error::Validation(
                "environment workspace has uncommitted content".into(),
            ));
        }
        let report = operator.latest_report_payload()?;
        if !report.passed() {
            return Err(Error::Validation(
                "environment's latest validation report did not pass".into(),
            ));
        }
        let digest = report.digest.clone().ok_or_else(|| {
            Error::Validation("latest validation report did not produce an image".into())
        })?;
        let reference = self.local_reference(environment_id, &digest)?;
        let previous = registry.get(environment_id)?;

        operator.mutate_manifest(|manifest| {
            manifest.lifecycle.publication_pending = true;
            if let Some(previous) = &previous {
                manifest.lifecycle.base_reference = Some(previous.reference.clone());
            }
        })?;
        operator.snapshot("environment: freeze local activation")?;
        registry.approve(
            environment_id,
            Environment {
                reference: reference.clone(),
                interpreters: operator.manifest().interpreters.clone(),
            },
        )?;
        Ok(InstalledEnvironment { reference, digest })
    }

    /// Roll the catalog back to the reference the current activation replaced.
    ///
    /// The rollback target is restored in the catalog and the replaced
    /// reference becomes the new rollback base, so a local update chain can
    /// step backwards repeatedly.
    pub fn rollback(&self, environment_id: &str, registry: &EnvironmentRegistry) -> Result<String> {
        let mut operator = self.develop(environment_id)?.ok_or_else(|| {
            Error::Validation(format!("environment `{environment_id}` is not present"))
        })?;
        let base = operator
            .manifest()
            .lifecycle
            .base_reference
            .clone()
            .ok_or_else(|| {
                Error::Validation(format!(
                    "environment `{environment_id}` has no rollback base"
                ))
            })?;
        let current = registry
            .get(environment_id)?
            .ok_or_else(|| {
                Error::Validation(format!(
                    "environment `{environment_id}` is not active in the catalog"
                ))
            })?;
        if current.reference == base {
            return Err(Error::Validation(
                "environment has no prior reference to roll back to".into(),
            ));
        }
        if !operator.repository().is_clean()? {
            return Err(Error::Validation(
                "environment workspace must be clean before rollback".into(),
            ));
        }
        registry.approve(
            environment_id,
            Environment {
                reference: base.clone(),
                interpreters: operator.manifest().interpreters.clone(),
            },
        )?;
        operator.mutate_manifest(|manifest| {
            manifest.lifecycle.base_reference = Some(current.reference);
        })?;
        operator.snapshot("environment: rollback catalog reference")?;
        Ok(base)
    }

    /// Remove the environment from the catalog while retaining its workspace.
    pub fn uninstall(&self, environment_id: &str, registry: &EnvironmentRegistry) -> Result<Environment> {
        crate::validate_plugin_name(environment_id)?;
        let mut operator = self.develop(environment_id)?.ok_or_else(|| {
            Error::Validation(format!("unknown environment `{environment_id}`"))
        })?;
        if !operator.repository().is_clean()? {
            return Err(Error::Validation(format!(
                "environment `{environment_id}` has uncommitted development changes; \
                 commit or discard them before uninstalling"
            )));
        }
        let removed = registry.get(environment_id)?.ok_or_else(|| {
            Error::Validation(format!(
                "environment `{environment_id}` is not registered in the catalog"
            ))
        })?;
        operator.mutate_manifest(|manifest| {
            manifest.lifecycle.publication_pending = false;
        })?;
        operator.snapshot("environment: uninstall catalog entry")?;
        registry.remove(environment_id)?;
        Ok(removed)
    }

    fn environment_path(&self, environment_id: &str) -> PathBuf {
        self.root.join(environment_id)
    }
}

impl EnvironmentOperator<'_> {
    /// Reload the authoritative manifest from disk.
    pub fn refresh(&mut self) -> Result<()> {
        self.manifest = load_manifest(&self.workspace())?;
        Ok(())
    }

    /// Return this workspace's stable VFS address.
    pub fn development_vfs_path(&self) -> Result<String> {
        self.store.development_vfs_path(&self.environment_id)
    }

    pub fn environment_id(&self) -> &str {
        &self.environment_id
    }

    pub fn status(&self) -> EnvironmentStatus {
        self.manifest.status
    }

    pub fn manifest(&self) -> &EnvironmentManifest {
        &self.manifest
    }

    pub(crate) fn environment_path(&self) -> PathBuf {
        self.store.environment_path(&self.environment_id)
    }

    pub(crate) fn repository(&self) -> GitRepo {
        GitRepo::open(self.environment_path())
    }

    /// Append-only reports live beside the manifest but outside image builds.
    pub fn reports_dir(&self) -> PathBuf {
        self.environment_path().join(".rsi").join("reports")
    }

    /// Return the environment's git-backed safe workspace.
    pub fn workspace(&self) -> PluginWorkspace {
        PluginWorkspace::new(self.environment_path())
    }

    /// Commit the current in-place development state.
    pub fn snapshot(&mut self, message: &str) -> Result<Option<String>> {
        let repo = self.repository();
        repo.snapshot_commit(message, &self.store.author_name, &self.store.author_email)
    }

    /// Persist one host-owned lifecycle mutation without committing it.
    pub(crate) fn mutate_manifest(
        &mut self,
        mutate: impl FnOnce(&mut EnvironmentManifest),
    ) -> Result<()> {
        let before = self.manifest.status;
        mutate(&mut self.manifest);
        let after = self.manifest.status;
        if before != after {
            ensure_environment_transition(before, after)?;
        }
        save_manifest(&self.workspace(), &self.manifest)
    }

    /// Transition and commit one lifecycle marker change.
    pub fn transition_snapshot(
        &mut self,
        to: EnvironmentStatus,
        message: &str,
    ) -> Result<Option<String>> {
        self.mutate_manifest(|manifest| manifest.status = to)?;
        self.snapshot(message)
    }

    /// Local build tag for the next validation attempt.
    pub fn next_attempt_tag(&self) -> Result<(u32, String)> {
        let attempt = next_validation_attempt(&self.reports_dir())?;
        let tag = self.store.local_tag(&self.environment_id, attempt)?;
        Ok((attempt, tag))
    }

    /// Read and parse the report referenced by `lifecycle.latest_report`.
    fn latest_report_payload(&self) -> Result<crate::EnvironmentValidationReport> {
        let relative = self
            .manifest
            .lifecycle
            .latest_report
            .clone()
            .ok_or_else(|| Error::Validation("environment has no validation report".into()))?;
        let path = self.environment_path().join(&relative);
        let text = std::fs::read_to_string(&path).map_err(|source| Error::ReadFile {
            path: path.clone(),
            source,
        })?;
        serde_json::from_str(&text)
            .map_err(|source| Error::Validation(format!("invalid report JSON: {source}")))
    }
}

pub(crate) fn load_manifest(
    workspace: &PluginWorkspace,
) -> Result<EnvironmentManifest> {
    let path = workspace.path().join("manifest.toml");
    let text = std::fs::read_to_string(&path).map_err(|source| Error::ReadFile {
        path: path.clone(),
        source,
    })?;
    toml::from_str(&text).map_err(|source| Error::ParseToml { path, source })
}

pub(crate) fn save_manifest(
    workspace: &PluginWorkspace,
    manifest: &EnvironmentManifest,
) -> Result<()> {
    let text = toml::to_string_pretty(manifest).map_err(|source| Error::SerializeToml {
        path: workspace.path().join("manifest.toml"),
        source,
    })?;
    workspace.write_text("manifest.toml", &text)
}

fn default_containerfile() -> String {
    "Containerfile".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_references_are_digest_pinned_localhost_addresses() {
        let state = tempfile::tempdir().unwrap();
        let store = EnvironmentStore::open(
            state.path(),
            "main",
            "Test",
            "test@example.com",
            DEFAULT_LOCAL_NAMESPACE,
        );
        let digest = format!("sha256:{}", "ab".repeat(32));
        assert_eq!(
            store.local_reference("demo-env", &digest).unwrap(),
            format!("localhost/auto-nomics/environments/demo-env@{digest}")
        );
        assert_eq!(
            store.local_tag("demo-env", 3).unwrap(),
            "localhost/auto-nomics/environments/demo-env:rsi-3"
        );
        // The same kebab-case grammar as plugin names applies.
        assert!(store.local_reference("Demo_Env", &digest).is_err());
        assert!(store
            .local_reference("demo-env", "sha256:short")
            .is_err());
    }
}
