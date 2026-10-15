//! High-level facade for plugin-based recursive self-improvement.
//!
//! `RsiInfra` is the system-facing authority. It owns the durable stores and
//! routes trusted publishers and runtime registration behind host-only
//! adapters, so callers never reassemble the lifecycle from its pieces.

use std::{
    path::Path,
    sync::{Arc, RwLock},
};

use evolution_core::observation::ObservationInput;

use crate::{
    EnvironmentCatalog, EnvironmentRegistry, Error, LocalActivationOutcome, ObservationRequest,
    PluginCacheReport, PluginLifecycle, PluginPublisher, PluginPullRequestPublisher, PluginStatus,
    PluginStore, RequestIntent, RequestRecord, RequestStatus, RequestStore, Result,
    ValidationOutcome, distill::DistillationCandidate, feedback::observation_request,
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
pub struct RsiInfra {
    store: Arc<PluginStore>,
    requests: RequestStore,
    /// Shared observation evidence, resolved through the process-wide
    /// hub — upstream of both plugins and skills. Plugin RSI never
    /// touches the skill manager for evidence.
    observations: Arc<evolution_core::ObservationHub>,
    environments: EnvironmentRegistry,
    publisher: SharedPluginPublisher,
    pull_request_publisher: SharedPullRequestPublisher,
    registry: Arc<RwLock<Option<Arc<dyn PluginRegistryControl>>>>,
}

impl RsiInfra {
    /// Open durable stores beneath one state directory.
    pub fn open(
        state_dir: &std::path::Path,
        default_branch: &str,
        author_name: &str,
        author_email: &str,
        observations: Arc<evolution_core::ObservationHub>,
        environments: EnvironmentCatalog,
        publisher: SharedPluginPublisher,
        pull_request_publisher: SharedPullRequestPublisher,
    ) -> Result<Self> {
        Self::open_with_environments(
            state_dir,
            default_branch,
            author_name,
            author_email,
            observations,
            EnvironmentRegistry::ephemeral(environments),
            publisher,
            pull_request_publisher,
        )
    }

    /// Open RSI with a persistent, daemon-owned environment registry.
    #[allow(clippy::too_many_arguments)]
    pub fn open_with_environments(
        state_dir: &std::path::Path,
        default_branch: &str,
        author_name: &str,
        author_email: &str,
        observations: Arc<evolution_core::ObservationHub>,
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
            store: Arc::clone(&store),
            requests: RequestStore::open(state_dir),
            observations,
            environments,
            publisher,
            pull_request_publisher,
            registry: Arc::new(RwLock::new(None)),
        })
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

    /// Return local-active plugins waiting for background distillation.
    pub fn pending_distillation(&self) -> Result<Vec<DistillationCandidate>> {
        crate::distill::PluginDistiller::new(self.clone()).pending()
    }

    pub fn requests(&self) -> &RequestStore {
        &self.requests
    }

    /// Record shared observation evidence without creating plugin demand.
    pub async fn record_observation(
        &self,
        input: ObservationInput,
    ) -> Result<evolution_core::Observation> {
        self.observations
            .record(input)
            .await
            .map_err(|error| Error::Validation(format!("cannot record observation: {error}")))
    }

    /// Promote an existing shared observation into plugin demand.
    ///
    /// The observation id becomes the request's first evidence id, so later
    /// skill distillation and plugin review reference the same evidence.
    pub async fn create_plugin_request_from_observation(
        &self,
        observation_id: &str,
        intent: RequestIntent,
        plugin_name: Option<&str>,
    ) -> Result<ObservationRequest> {
        observation_request(
            &self.requests,
            self.observations.storage().as_ref(),
            observation_id,
            intent,
            plugin_name,
        )
        .await
    }

    /// Record one observation and immediately promote it to plugin demand.
    pub async fn record_plugin_observation(
        &self,
        input: ObservationInput,
        intent: RequestIntent,
        plugin_name: &str,
    ) -> Result<ObservationRequest> {
        let observation = self.record_observation(input).await?;
        self.create_plugin_request_from_observation(&observation.id, intent, Some(plugin_name))
            .await
    }

    pub fn configure_registry(&self, control: Arc<dyn PluginRegistryControl>) {
        *self
            .registry
            .write()
            .expect("registry control lock poisoned") = Some(control);
    }

    /// Record demand and materialize a new plugin workspace.
    pub fn create_plugin(
        &self,
        request: RequestRecord,
        environment_id: &str,
    ) -> Result<crate::PluginOperator<'_>> {
        let request = self.requests.record(request)?;
        self.ensure_request_plugin(&request, request.plugin_name.as_deref())?;
        let operator = self.store.create(
            request.plugin_name.as_deref().ok_or_else(|| {
                Error::InvalidRequest("new plugin requests must name their plugin".into())
            })?,
            environment_id,
            std::slice::from_ref(&request.id),
            &request.body,
            &self.requests,
            &self.catalog(),
        )?;
        self.requests
            .set_status(&request.id, RequestStatus::Working)?;
        Ok(operator)
    }

    /// Start an in-place update from the plugin's installed source.
    pub fn start_update(
        &self,
        plugin_name: &str,
        request: RequestRecord,
    ) -> Result<crate::PluginOperator<'_>> {
        let request = self.requests.record(request)?;
        self.ensure_request_plugin(&request, Some(plugin_name))?;
        let source = self.store.installed_source(plugin_name)?;
        self.store.create_update(
            plugin_name,
            std::slice::from_ref(&request.id),
            &request.body,
            &self.requests,
            &source,
            &self.catalog(),
            &crate::GitPluginSourceFetcher,
        )?;
        self.requests
            .set_status(&request.id, RequestStatus::Working)?;
        self.store
            .develop(plugin_name)?
            .ok_or_else(|| Error::Validation("plugin disappeared during update startup".into()))
    }

    /// Fork an installed reference plugin and claim the request for development.
    ///
    /// This is the fast path for agents: the fork is already runnable in the
    /// reference environment and can be activated locally after validation.
    /// Publication and upstream merge remain background distiller work.
    pub fn fork_plugin(
        &self,
        reference_plugin_name: &str,
        request: RequestRecord,
    ) -> Result<crate::PluginOperator<'_>> {
        let request = self.requests.record(request)?;
        let target_name = request
            .plugin_name
            .clone()
            .ok_or_else(|| Error::InvalidRequest("fork requests must name their plugin".into()))?;
        self.ensure_request_plugin(&request, Some(&target_name))?;
        self.store.fork(
            reference_plugin_name,
            &target_name,
            std::slice::from_ref(&request.id),
            &request.body,
            &self.requests,
            &self.catalog(),
        )?;
        self.requests
            .set_status(&request.id, RequestStatus::Working)?;
        self.store
            .develop(&target_name)?
            .ok_or_else(|| Error::Validation("plugin fork disappeared during startup".into()))
    }

    /// Validate, record evidence, submit, and propagate request state.
    pub fn validate_and_submit(&self, plugin_name: &str) -> Result<ValidationOutcome> {
        let installed_kinds = self.installed_node_addresses()?;
        let mut operator = self
            .store
            .develop(plugin_name)?
            .ok_or_else(|| Error::Validation(format!("unknown plugin `{plugin_name}`")))?;
        let request_ids = operator.manifest().lifecycle.request_ids.clone();
        let catalog = self.catalog();
        let outcome = {
            let mut lifecycle = PluginLifecycle::new(&mut operator, &catalog, &installed_kinds);
            lifecycle.validate_and_submit()?
        };
        if matches!(outcome, ValidationOutcome::Submitted(_)) {
            for id in request_ids {
                self.requests
                    .set_status(&id, RequestStatus::ReviewPending)?;
            }
        }
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

    pub fn review(&self, plugin_name: &str, approved: bool) -> Result<PluginStatus> {
        let installed_kinds = self.installed_node_addresses()?;
        let mut operator = self
            .store
            .develop(plugin_name)?
            .ok_or_else(|| Error::Validation(format!("unknown plugin `{plugin_name}`")))?;
        let request_ids = operator.manifest().lifecycle.request_ids.clone();
        let catalog = self.catalog();
        let status = {
            let mut lifecycle = PluginLifecycle::new(&mut operator, &catalog, &installed_kinds);
            lifecycle.review(approved)?
        };
        let request_status = if approved {
            RequestStatus::Consumed
        } else {
            RequestStatus::Rejected
        };
        for id in request_ids {
            self.requests.set_status(&id, request_status)?;
        }
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
        let source = self.store.install_local(plugin_name)?;
        self.reload_plugin(plugin_name)?;
        Ok(source)
    }

    /// Validate a workspace and immediately activate its immutable snapshot.
    ///
    /// Human review and GitHub publication are intentionally not on this path.
    /// The manifest remains editable while the DAG reads an immutable local
    /// snapshot, and the background distiller can later publish or merge it.
    pub fn validate_and_activate_local(&self, plugin_name: &str) -> Result<LocalActivationOutcome> {
        match self.validate_local(plugin_name)? {
            ValidationOutcome::Passed(report) => {
                let source = self.install_local(plugin_name)?;
                let request_ids = self
                    .store
                    .develop(plugin_name)?
                    .ok_or_else(|| {
                        Error::Validation("plugin disappeared during local activation".into())
                    })?
                    .manifest()
                    .lifecycle
                    .request_ids
                    .clone();
                for id in request_ids {
                    self.requests
                        .set_status(&id, RequestStatus::ReviewPending)?;
                }
                Ok(LocalActivationOutcome::Activated(report, source))
            }
            ValidationOutcome::NeedsFix(report) => Ok(LocalActivationOutcome::NeedsFix(report)),
            ValidationOutcome::Submitted(_) => Err(Error::Validation(
                "local validation returned the review submission state".into(),
            )),
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

    fn ensure_request_plugin(
        &self,
        request: &RequestRecord,
        plugin_name: Option<&str>,
    ) -> Result<()> {
        if request.plugin_name.as_deref() == plugin_name {
            Ok(())
        } else {
            Err(Error::InvalidRequest(format!(
                "request `{}` targets a different plugin",
                request.id
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Environment, RequestIntent, RequestSource};
    use evolution_core::observation::{
        ObservationKind, ObservationSource as SkillObservationSource,
    };

    #[test]
    fn creating_plugin_claims_request_through_the_unified_facade() {
        let state = tempfile::tempdir().unwrap();
        let observations = std::sync::Arc::new(evolution_core::ObservationHub::new(state.path()));
        let mut catalog = EnvironmentCatalog::default();
        catalog.insert(
            "alpine",
            Environment {
                reference: "docker.io/library/alpine@sha256:0123456789012345678901234567890123456789012345678901234567890123".into(),
                interpreters: vec!["sh".into()],
            },
        );
        let publisher = Arc::new(crate::GhPublisher::new(crate::GhPublisherConfig {
            enabled: false,
            ..Default::default()
        }));
        let infra = RsiInfra::open(
            state.path(),
            "main",
            "Test",
            "test@example.com",
            observations,
            catalog,
            publisher.clone(),
            publisher,
        )
        .unwrap();
        let request = infra
            .requests()
            .record(RequestRecord {
                id: String::new(),
                created_at: 0,
                source: RequestSource::User,
                intent: RequestIntent::NewNode,
                summary: "Create a demo adapter".into(),
                body: "Copy one input file.".into(),
                plugin_name: Some("demo-plugin".into()),
                evidence_ids: Vec::new(),
                status: RequestStatus::Open,
            })
            .unwrap();
        let operator = infra.create_plugin(request.clone(), "alpine").unwrap();
        assert_eq!(operator.status(), PluginStatus::Draft);
        assert_eq!(
            infra.requests().find(&request.id).unwrap().unwrap().status,
            RequestStatus::Working
        );
    }

    #[tokio::test]
    async fn observations_feed_both_skill_and_plugin_feedback_systems() {
        let state = tempfile::tempdir().unwrap();
        let observations = std::sync::Arc::new(evolution_core::ObservationHub::new(state.path()));
        let catalog = EnvironmentCatalog::default();
        let publisher = Arc::new(crate::GhPublisher::new(crate::GhPublisherConfig {
            enabled: false,
            ..Default::default()
        }));
        let infra = RsiInfra::open(
            state.path(),
            "main",
            "Test",
            "test@example.com",
            observations.clone(),
            catalog,
            publisher.clone(),
            publisher,
        )
        .unwrap();

        let feedback = infra
            .record_plugin_observation(
                ObservationInput {
                    kind: ObservationKind::Failure,
                    source: SkillObservationSource::Eval,
                    summary: "adapter rejects empty input".into(),
                    body: "The current script silently copies empty inputs.".into(),
                    node_kind: Some("demo_adapter".into()),
                    error: Some("empty input accepted".into()),
                },
                RequestIntent::FixNode,
                "demo-plugin",
            )
            .await
            .unwrap();

        assert_eq!(
            feedback.request.evidence_ids,
            vec![feedback.observation.id.clone()]
        );
        assert_eq!(feedback.request.source, RequestSource::Eval);
        assert_eq!(feedback.request.plugin_name.as_deref(), Some("demo-plugin"));
        assert!(
            observations
                .storage()
                .list()
                .await
                .iter()
                .any(|observation| observation.id == feedback.observation.id)
        );
    }
}
