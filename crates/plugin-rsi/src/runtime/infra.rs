//! High-level facade for daemon-owned plugin lifecycle management.
//!
//! `PluginInfra` is the system-facing authority. It owns the durable stores and
//! routes trusted publishers and runtime registration behind host-only
//! adapters, so callers never reassemble the lifecycle from its pieces.

use std::{
    path::Path,
    sync::{Arc, Mutex, RwLock},
};

use crate::{
    EnvironmentCatalog, EnvironmentRegistry, Error, LocalActivationOutcome, PluginCacheReport,
    PluginLifecycle, PluginPublisher, PluginPullRequestPublisher, PluginStatus, PluginStore,
    Result, ValidationOutcome, plugin::distiller::DistillationCandidate,
};

/// Trusted publisher used for plugin releases and installs.
pub type SharedPluginPublisher = Arc<dyn PluginPublisher + Send + Sync>;
/// Trusted publisher used for plugin update pull requests.
pub type SharedPullRequestPublisher = Arc<dyn PluginPullRequestPublisher + Send + Sync>;

/// Runtime-side bridge for rebuilding or hot-swapping node registration.
pub trait PluginRegistryControl: Send + Sync {
    /// Return node kinds currently visible to the live engine.
    fn installed_node_addresses(&self) -> Result<Vec<String>>;

    /// Replace the live implementation of one plugin's registered kinds.
    fn reload_plugin(&self, plugin_name: &str) -> Result<()>;

    /// Remove the live factories contributed by one plugin.
    fn uninstall_plugin(&self, plugin_name: &str) -> Result<()>;
}

/// One trusted plugin subsystem.
#[derive(Clone)]
pub struct PluginInfra {
    pub(crate) activation_lock: Arc<Mutex<()>>,
    store: Arc<PluginStore>,
    environments: EnvironmentRegistry,
    publisher: SharedPluginPublisher,
    pull_request_publisher: SharedPullRequestPublisher,
    registry: Arc<RwLock<Option<Arc<dyn PluginRegistryControl>>>>,
    dev_execution: Arc<RwLock<Option<Arc<container_runtime::ContainerExecutionInfra>>>>,
}

impl PluginInfra {
    /// Open RSI with a persistent, daemon-owned environment registry.
    #[allow(clippy::too_many_arguments)]
    pub fn open_with_environments(
        state_dir: &std::path::Path,
        default_branch: &str,
        author_name: &str,
        author_email: &str,
        environments: EnvironmentRegistry,
        publisher: SharedPluginPublisher,
        pull_request_publisher: SharedPullRequestPublisher,
    ) -> Result<Self> {
        let store = Arc::new(PluginStore::open(
            state_dir,
            default_branch,
            author_name,
            author_email,
        ));
        Ok(Self {
            activation_lock: Arc::new(Mutex::new(())),
            store: Arc::clone(&store),
            environments,
            publisher,
            pull_request_publisher,
            registry: Arc::new(RwLock::new(None)),
            dev_execution: Arc::new(RwLock::new(None)),
        })
    }

    pub fn configure_registry(&self, control: Arc<dyn PluginRegistryControl>) {
        *self
            .registry
            .write()
            .expect("registry control lock poisoned") = Some(control);
    }

    /// Attach the container execution infra used by development
    /// sessions (same install-later pattern as [`Self::configure_registry`]).
    pub fn configure_dev_execution(
        &self,
        execution: Arc<container_runtime::ContainerExecutionInfra>,
    ) {
        *self
            .dev_execution
            .write()
            .expect("dev execution lock poisoned") = Some(execution);
    }

    /// Development-session manager for plugin workspaces.
    pub fn dev_sessions(&self) -> Result<crate::runtime::dev_session::DevSessionManager> {
        let execution = self
            .dev_execution
            .read()
            .expect("dev execution lock poisoned")
            .clone()
            .ok_or_else(|| {
                Error::Validation("plugin development containers are not configured".into())
            })?;
        Ok(crate::runtime::dev_session::DevSessionManager::new(
            execution,
            Arc::clone(&self.store),
        ))
    }

    pub fn store(&self) -> Arc<PluginStore> {
        Arc::clone(&self.store)
    }

    /// Inspect the immutable runtime source and compare it with development.
    ///
    /// The returned report is deliberately file-based rather than inferred
    /// from in-memory factory state: callers see exactly what the plugin
    /// loader reads from the active materialization.
    pub fn inspect_plugin_cache(&self, plugin_name: &str) -> Result<PluginCacheReport> {
        self.store.inspect_plugin_cache(plugin_name)
    }

    /// Return the host-owned mutable environment allow list.
    pub fn environment_registry(&self) -> EnvironmentRegistry {
        self.environments.clone()
    }

    /// Return local-active plugins waiting for background publication.
    pub fn pending_distillation(&self) -> Result<Vec<DistillationCandidate>> {
        crate::plugin::distiller::PluginDistiller::new(self.clone()).pending()
    }

    /// Create an empty Draft plugin workspace bound to an approved environment.
    pub fn create_plugin(
        &self,
        plugin_name: &str,
        environment_id: &str,
        purpose: &str,
    ) -> Result<crate::PluginOperator<'_>> {
        self.store
            .create(plugin_name, environment_id, purpose, &self.catalog())
    }

    /// Create a Draft plugin workspace with the runnable scaffold — the
    /// human entry point (GUI plugin manager).
    pub fn create_plugin_scaffolded(
        &self,
        plugin_name: &str,
        environment_id: &str,
        purpose: &str,
    ) -> Result<crate::PluginOperator<'_>> {
        self.store
            .create_scaffolded(plugin_name, environment_id, purpose, &self.catalog())
    }

    /// Fork an installed reference plugin into a new development workspace.
    ///
    /// This is the fast path for agents: the fork is already runnable in the
    /// reference environment and can be activated locally after validation.
    /// Publication and upstream merge remain background distiller work.
    pub fn fork_plugin(
        &self,
        reference_plugin_name: &str,
        plugin_name: &str,
        purpose: &str,
    ) -> Result<crate::PluginOperator<'_>> {
        self.store
            .fork(reference_plugin_name, plugin_name, purpose, &self.catalog())
    }

    /// Validate a workspace, append evidence, and submit it for review.
    pub fn validate_and_submit(&self, plugin_name: &str) -> Result<ValidationOutcome> {
        let installed_kinds = self.installed_node_addresses()?;
        let mut operator = self
            .store
            .develop(plugin_name)?
            .ok_or_else(|| Error::Validation(format!("unknown plugin `{plugin_name}`")))?;
        let catalog = self.catalog();
        let outcome = {
            let mut lifecycle = PluginLifecycle::new(&mut operator, &catalog, &installed_kinds);
            lifecycle.validate_and_submit()?
        };
        Ok(outcome)
    }

    /// Validate a workspace while leaving it editable for local development.
    pub fn validate_local(&self, plugin_name: &str) -> Result<ValidationOutcome> {
        let installed_kinds = self.installed_node_addresses()?;
        let mut operator = self
            .store
            .develop(plugin_name)?
            .ok_or_else(|| Error::Validation(format!("unknown plugin `{plugin_name}`")))?;
        let catalog = self.catalog();
        let mut lifecycle = PluginLifecycle::new(&mut operator, &catalog, &installed_kinds);
        lifecycle.validate_local()
    }

    pub fn review(&self, plugin_name: &str) -> Result<PluginStatus> {
        let installed_kinds = self.installed_node_addresses()?;
        let mut operator = self
            .store
            .develop(plugin_name)?
            .ok_or_else(|| Error::Validation(format!("unknown plugin `{plugin_name}`")))?;
        let catalog = self.catalog();
        let status = {
            let mut lifecycle = PluginLifecycle::new(&mut operator, &catalog, &installed_kinds);
            lifecycle.review()?
        };
        Ok(status)
    }

    pub fn publish_reviewed(&self, plugin_name: &str) -> Result<PluginStatus> {
        let installed_kinds = self.installed_node_addresses()?;
        let mut operator = self
            .store
            .develop(plugin_name)?
            .ok_or_else(|| Error::Validation(format!("unknown plugin `{plugin_name}`")))?;
        let catalog = self.catalog();
        let mut lifecycle = PluginLifecycle::new(&mut operator, &catalog, &installed_kinds);
        lifecycle.publish_reviewed(self.publisher.as_ref())
    }

    pub fn open_update_pull_request(&self, plugin_name: &str) -> Result<PluginStatus> {
        let installed_kinds = self.installed_node_addresses()?;
        let mut operator = self
            .store
            .develop(plugin_name)?
            .ok_or_else(|| Error::Validation(format!("unknown plugin `{plugin_name}`")))?;
        let catalog = self.catalog();
        let mut lifecycle = PluginLifecycle::new(&mut operator, &catalog, &installed_kinds);
        lifecycle.open_update_pull_request(self.pull_request_publisher.as_ref())
    }

    pub fn merge_update_pull_request(&self, plugin_name: &str) -> Result<PluginStatus> {
        let installed_kinds = self.installed_node_addresses()?;
        let mut operator = self
            .store
            .develop(plugin_name)?
            .ok_or_else(|| Error::Validation(format!("unknown plugin `{plugin_name}`")))?;
        let catalog = self.catalog();
        let mut lifecycle = PluginLifecycle::new(&mut operator, &catalog, &installed_kinds);
        lifecycle.merge_update_pull_request(self.pull_request_publisher.as_ref())
    }

    /// Install the reviewed result and ask the runtime to refresh its registry.
    pub fn install(&self, plugin_name: &str) -> Result<PluginStatus> {
        let installed_kinds = self.installed_node_addresses()?;
        let mut operator = self
            .store
            .develop(plugin_name)?
            .ok_or_else(|| Error::Validation(format!("unknown plugin `{plugin_name}`")))?;
        let catalog = self.catalog();
        let status = {
            let mut lifecycle = PluginLifecycle::new(&mut operator, &catalog, &installed_kinds);
            lifecycle.install(self.publisher.as_ref())?
        };
        self.reload_plugin(plugin_name)?;
        Ok(status)
    }

    /// Install the clean, validated workspace as a local runtime snapshot.
    ///
    /// This integrates the plugin with the DAG before GitHub publication.
    /// The development workspace stays independent of the immutable snapshot.
    pub fn install_local(&self, plugin_name: &str) -> Result<crate::LocalInstalledPluginSource> {
        let _guard = self
            .activation_lock
            .lock()
            .map_err(|_| Error::Validation("activation lock poisoned".into()))?;
        let expected = crate::content_fingerprint(&self.store.root().join(plugin_name))?;
        self.activate_candidate(plugin_name, &expected)
    }

    fn activate_candidate(
        &self,
        plugin_name: &str,
        expected: &str,
    ) -> Result<crate::LocalInstalledPluginSource> {
        let registry_path = self.store.registry_path();
        let previous = match std::fs::read(registry_path) {
            Ok(b) => Some(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        self.bind_author_suite_if_present(plugin_name)?;
        let suite = self.behavior_suite(plugin_name)?;
        let result = self
            .store
            .install_local_checked(plugin_name, |candidate| {
                if crate::content_fingerprint(candidate)? != expected
                    || crate::content_fingerprint(&self.store.root().join(plugin_name))? != expected
                {
                    return Err(Error::Validation(
                        "workspace changed during validation; revalidate before activation".into(),
                    ));
                }
                let workspace = crate::PluginWorkspace::new(candidate);
                let manifest: container_plugin::manifest::PluginManifest =
                    toml::from_str(&workspace.read_text("manifest.toml")?)
                        .map_err(|e| Error::Validation(e.to_string()))?;
                let owned: Vec<_> = manifest.nodes.iter().map(|n| n.kind.clone()).collect();
                let report = crate::validate_workspace(
                    plugin_name,
                    &workspace,
                    &self.catalog(),
                    &self.installed_node_addresses()?,
                    &owned,
                    1,
                );
                if !report.passed() {
                    return Err(Error::Validation(format!(
                        "frozen candidate engineering validation failed: {:?}",
                        report
                    )));
                }
                self.validate_behavior_candidate(plugin_name, candidate, suite.as_ref())?;
                if crate::content_fingerprint(candidate)? != expected
                    || crate::content_fingerprint(&self.store.root().join(plugin_name))? != expected
                {
                    return Err(Error::Validation(
                        "content changed while behavior tests ran".into(),
                    ));
                }
                Ok(())
            })
            .and_then(|source| {
                self.reload_plugin(plugin_name)?;
                Ok(source)
            });
        if let Err(error) = result {
            let restore = (|| -> Result<()> {
                if let Some(bytes) = previous {
                    std::fs::write(registry_path, bytes)?;
                } else if registry_path.exists() {
                    std::fs::remove_file(registry_path)?;
                }
                self.store.materialize_registry()?;
                if self.store.registry_sources()?.contains_key(plugin_name) {
                    self.reload_plugin(plugin_name)?;
                } else {
                    self.unload_plugin(plugin_name)?;
                }
                Ok(())
            })();
            return match restore {
                Ok(()) => Err(error),
                Err(recovery) => Err(Error::Validation(format!(
                    "ACTIVATION_INCONSISTENT: {error}; restoration failed: {recovery}"
                ))),
            };
        }
        result
    }

    pub fn validate_and_activate_local(&self, plugin_name: &str) -> Result<LocalActivationOutcome> {
        self.validate_and_activate_local_expected(plugin_name, None)
    }

    pub fn validate_and_activate_local_expected(
        &self,
        plugin_name: &str,
        expected: Option<&str>,
    ) -> Result<LocalActivationOutcome> {
        let _guard = self
            .activation_lock
            .lock()
            .map_err(|_| Error::Validation("activation lock poisoned".into()))?;
        let fingerprint = crate::content_fingerprint(&self.store.root().join(plugin_name))?;
        if expected.is_some_and(|e| e != fingerprint) {
            return Err(Error::Validation(
                "development fingerprint mismatch; query plugin_status and revalidate".into(),
            ));
        }
        match self.validate_local(plugin_name)? {
            ValidationOutcome::Passed(report) => {
                let source = self.activate_candidate(plugin_name, &fingerprint)?;
                Ok(LocalActivationOutcome::Activated(report, source))
            }
            ValidationOutcome::NeedsFix(report) => Ok(LocalActivationOutcome::NeedsFix(report)),
            ValidationOutcome::Submitted(_) => {
                Err(Error::Validation("unexpected review submission".into()))
            }
        }
    }

    /// Roll back to the previous immutable source and refresh registration.
    pub fn rollback(&self, plugin_name: &str) -> Result<crate::InstalledPluginSource> {
        let source = self.store.rollback(plugin_name)?;
        self.reload_plugin(plugin_name)?;
        Ok(source)
    }

    /// Uninstall a plugin from the live registry and persistent runtime source.
    ///
    /// Development and snapshot history are retained for audit and later
    /// reinstallation.
    pub fn uninstall(&self, plugin_name: &str) -> Result<crate::InstalledPluginSource> {
        let source = self.store.uninstall(plugin_name)?;
        self.unload_plugin(plugin_name)?;
        Ok(source)
    }

    pub fn installed_node_addresses(&self) -> Result<Vec<String>> {
        if let Some(control) = self
            .registry
            .read()
            .expect("registry control lock poisoned")
            .as_ref()
        {
            return control.installed_node_addresses();
        }
        Ok(self
            .store
            .list()?
            .into_iter()
            .filter(|manifest| manifest.installation.is_runtime_active())
            .flat_map(|manifest| {
                let plugin = manifest.plugin_name;
                manifest
                    .nodes
                    .into_iter()
                    .map(move |node| format!("{plugin}/{}", node.kind))
            })
            .collect())
    }

    fn reload_plugin(&self, plugin_name: &str) -> Result<()> {
        self.registry
            .read()
            .expect("registry control lock poisoned")
            .as_ref()
            .ok_or_else(|| {
                Error::Validation("runtime plugin registry control is not configured".into())
            })?
            .reload_plugin(plugin_name)
    }

    fn unload_plugin(&self, plugin_name: &str) -> Result<()> {
        self.registry
            .read()
            .expect("registry control lock poisoned")
            .as_ref()
            .ok_or_else(|| {
                Error::Validation("runtime plugin registry control is not configured".into())
            })?
            .uninstall_plugin(plugin_name)
    }

    fn catalog(&self) -> Arc<EnvironmentCatalog> {
        Arc::new(self.environments.snapshot())
    }
}
