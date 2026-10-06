//! High-level facade for plugin-based recursive self-improvement.
//!
//! `RsiInfra` is the system-facing authority. It owns the durable stores and
//! routes trusted publishers and runtime registration behind host-only
//! adapters, so callers never reassemble the lifecycle from its pieces.

use std::{
    path::Path,
    sync::{Arc, RwLock},
};

use evolution_core::routing::observation_route;
use skills::observation::ObservationInput;

use crate::{
    EnvironmentCatalog, Error, ObservationAudience, ObservationRequest, ObservationRoute,
    ObservationRouteStatus, ObservationRouteStore, PluginLifecycle, PluginPublisher,
    PluginPullRequestPublisher, PluginStatus, PluginStore, RequestIntent, RequestRecord,
    RequestStatus, RequestStore, Result, ValidationOutcome,
    feedback::{default_request_intent, observation_request},
};

/// Trusted publisher used for plugin releases and installs.
pub type SharedPluginPublisher = Arc<dyn PluginPublisher + Send + Sync>;
/// Trusted publisher used for plugin update pull requests.
pub type SharedPullRequestPublisher = Arc<dyn PluginPullRequestPublisher + Send + Sync>;

/// Runtime-side bridge for rebuilding or hot-swapping node registration.
pub trait PluginRegistryControl: Send + Sync {
    /// Return node kinds currently visible to the live engine.
    fn installed_node_kinds(&self) -> Result<Vec<String>>;

    /// Replace the live implementation of one plugin's registered kinds.
    fn reload_plugin(&self, plugin_name: &str) -> Result<()>;
}

/// One trusted plugin subsystem.
#[derive(Clone)]
pub struct RsiInfra {
    store: Arc<PluginStore>,
    requests: RequestStore,
    skills: Arc<skills::SkillManager>,
    routes: ObservationRouteStore,
    environments: Arc<EnvironmentCatalog>,
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
        skills: Arc<skills::SkillManager>,
        environments: EnvironmentCatalog,
        publisher: SharedPluginPublisher,
        pull_request_publisher: SharedPullRequestPublisher,
    ) -> Result<Self> {
        let store = Arc::new(PluginStore::open(
            state_dir,
            default_branch,
            author_name,
            author_email,
        ));
        let state_root = store
            .root()
            .parent()
            .ok_or_else(|| Error::Validation("plugin root has no parent state directory".into()))?;
        let routes = ObservationRouteStore::open(state_root);
        Ok(Self {
            store: Arc::clone(&store),
            requests: RequestStore::open(state_dir),
            skills,
            routes,
            environments: Arc::new(environments),
            publisher,
            pull_request_publisher,
            registry: Arc::new(RwLock::new(None)),
        })
    }

    /// Return every durable routing decision for shared observations.
    pub fn observation_routes(&self) -> Vec<ObservationRoute> {
        self.routes.list()
    }

    pub fn store(&self) -> Arc<PluginStore> {
        Arc::clone(&self.store)
    }

    pub fn requests(&self) -> &RequestStore {
        &self.requests
    }

    /// Record shared observation evidence without creating plugin demand.
    pub fn record_observation(
        &self,
        input: ObservationInput,
    ) -> Result<skills::observation::Observation> {
        self.skills
            .record_observation(input)
            .map_err(|error| Error::Validation(format!("cannot record observation: {error}")))
    }

    /// Promote an existing shared observation into plugin demand.
    ///
    /// The observation id becomes the request's first evidence id, so later
    /// skill distillation and plugin review reference the same evidence.
    pub fn create_plugin_request_from_observation(
        &self,
        observation_id: &str,
        intent: RequestIntent,
        plugin_name: Option<&str>,
    ) -> Result<ObservationRequest> {
        observation_request(
            &self.requests,
            &self.skills.observations(),
            observation_id,
            intent,
            plugin_name,
        )
    }

    /// Record one observation and immediately promote it to plugin demand.
    pub fn record_plugin_observation(
        &self,
        input: ObservationInput,
        intent: RequestIntent,
        plugin_name: &str,
    ) -> Result<ObservationRequest> {
        let observation = self.record_observation(input)?;
        self.create_plugin_request_from_observation(&observation.id, intent, Some(plugin_name))
    }

    /// Record shared evidence, classify it, and create plugin demand when owned.
    pub fn record_and_route_observation(
        &self,
        input: ObservationInput,
        plugin_name: Option<&str>,
        intent: Option<RequestIntent>,
    ) -> Result<ObservationRouting> {
        let observation = self.record_observation(input)?;
        self.route_existing_observation(&observation.id, plugin_name, intent)
    }

    /// Route an observation that already exists in the shared store.
    pub fn route_existing_observation(
        &self,
        observation_id: &str,
        plugin_name: Option<&str>,
        intent: Option<RequestIntent>,
    ) -> Result<ObservationRouting> {
        let observation = self
            .skills
            .observations()
            .list()
            .into_iter()
            .find(|observation| observation.id == observation_id)
            .ok_or_else(|| {
                Error::InvalidRequest(format!("unknown observation {observation_id:?}"))
            })?;
        let plugin_owner = if plugin_name.is_some() {
            None
        } else {
            observation
                .node_kind
                .as_deref()
                .map(|kind| self.store.owner_of_node_kind(kind))
                .transpose()?
                .flatten()
        };
        let (_, route) = observation_route(&observation, plugin_name, plugin_owner.as_deref());
        let route = self.routes.record(route)?;
        let mut request = None;
        let mut route = route;
        if matches!(
            route.audience,
            ObservationAudience::Plugin | ObservationAudience::Both
        ) {
            let target = route.plugin_name.as_deref().ok_or_else(|| {
                Error::Validation("plugin route is missing its target plugin".into())
            })?;
            let promoted = observation_request(
                &self.requests,
                &self.skills.observations(),
                &route.observation_id,
                intent.unwrap_or_else(|| default_request_intent(observation.kind)),
                Some(target),
            )?;
            self.routes.consume(&route.id, &promoted.request.id)?;
            route.request_id = Some(promoted.request.id.clone());
            route.status = ObservationRouteStatus::Consumed;
            request = Some(promoted);
        }

        Ok(ObservationRouting {
            observation,
            decision_reason: route.reason.clone(),
            route,
            request,
        })
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

    /// Validate, record evidence, submit, and propagate request state.
    pub fn validate_and_submit(&self, plugin_name: &str) -> Result<ValidationOutcome> {
        let installed_kinds = self.installed_node_kinds()?;
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

    pub fn review(&self, plugin_name: &str, approved: bool) -> Result<PluginStatus> {
        let installed_kinds = self.installed_node_kinds()?;
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
        let installed_kinds = self.installed_node_kinds()?;
        let mut operator = self
            .store
            .develop(plugin_name)?
            .ok_or_else(|| Error::Validation(format!("unknown plugin `{plugin_name}`")))?;
        let catalog = self.catalog();
        let mut lifecycle = PluginLifecycle::new(&mut operator, &catalog, &installed_kinds);
        lifecycle.publish_reviewed(self.publisher.as_ref())
    }

    pub fn open_update_pull_request(&self, plugin_name: &str) -> Result<PluginStatus> {
        let installed_kinds = self.installed_node_kinds()?;
        let mut operator = self
            .store
            .develop(plugin_name)?
            .ok_or_else(|| Error::Validation(format!("unknown plugin `{plugin_name}`")))?;
        let catalog = self.catalog();
        let mut lifecycle = PluginLifecycle::new(&mut operator, &catalog, &installed_kinds);
        lifecycle.open_update_pull_request(self.pull_request_publisher.as_ref())
    }

    pub fn merge_update_pull_request(&self, plugin_name: &str) -> Result<PluginStatus> {
        let installed_kinds = self.installed_node_kinds()?;
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
        let installed_kinds = self.installed_node_kinds()?;
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

    /// Roll back to the previous immutable source and refresh registration.
    pub fn rollback(&self, plugin_name: &str) -> Result<crate::InstalledPluginSource> {
        let source = self.store.rollback(plugin_name)?;
        self.reload_plugin(plugin_name)?;
        Ok(source)
    }

    pub fn installed_node_kinds(&self) -> Result<Vec<String>> {
        if let Some(control) = self
            .registry
            .read()
            .expect("registry control lock poisoned")
            .as_ref()
        {
            return control.installed_node_kinds();
        }
        Ok(self
            .store
            .list()?
            .into_iter()
            .filter(|manifest| {
                matches!(
                    manifest.status,
                    PluginStatus::Published | PluginStatus::Installed
                )
            })
            .flat_map(|manifest| manifest.nodes.into_iter().map(|node| node.kind))
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

    fn catalog(&self) -> Arc<EnvironmentCatalog> {
        Arc::clone(&self.environments)
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

/// The result of routing one shared observation.
#[derive(Debug, Clone, PartialEq)]
pub struct ObservationRouting {
    /// The canonical observation retained for skill evolution.
    pub observation: skills::observation::Observation,
    /// Why the router selected the audience.
    pub decision_reason: String,
    /// The durable route record.
    pub route: ObservationRoute,
    /// Plugin demand, present only for plugin/both routes.
    pub request: Option<ObservationRequest>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Environment, RequestIntent, RequestSource};
    use skills::observation::{ObservationKind, ObservationSource as SkillObservationSource};

    #[test]
    fn creating_plugin_claims_request_through_the_unified_facade() {
        let state = tempfile::tempdir().unwrap();
        let skills = skills::SkillManager::init(skills::SkillManager::new(state.path()));
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
            skills,
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

    #[test]
    fn observations_feed_both_skill_and_plugin_feedback_systems() {
        let state = tempfile::tempdir().unwrap();
        let skills = skills::SkillManager::init(skills::SkillManager::new(state.path()));
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
            skills.clone(),
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
            .unwrap();

        assert_eq!(
            feedback.request.evidence_ids,
            vec![feedback.observation.id.clone()]
        );
        assert_eq!(feedback.request.source, RequestSource::Eval);
        assert_eq!(feedback.request.plugin_name.as_deref(), Some("demo-plugin"));
        assert!(
            skills
                .observations()
                .list()
                .iter()
                .any(|observation| observation.id == feedback.observation.id)
        );
        let routing = infra
            .record_and_route_observation(
                ObservationInput {
                    kind: ObservationKind::Failure,
                    source: SkillObservationSource::Eval,
                    summary: "adapter rejects malformed input".into(),
                    body: "The adapter should fail with a useful validation error.".into(),
                    node_kind: Some("demo_adapter".into()),
                    error: Some("malformed input".into()),
                },
                Some("demo-plugin"),
                Some(RequestIntent::FixNode),
            )
            .unwrap();

        assert_eq!(routing.route.audience, ObservationAudience::Plugin);
        assert_eq!(
            routing.route.request_id,
            Some(routing.request.as_ref().unwrap().request.id.clone())
        );
        assert_eq!(
            infra.observation_routes().len(),
            1,
            "routing must be idempotent across shared observations"
        );
        let persisted = infra
            .observation_routes()
            .into_iter()
            .find(|route| route.id == routing.route.id)
            .unwrap();
        assert_eq!(persisted.status, ObservationRouteStatus::Consumed);
        assert_eq!(
            persisted.request_id,
            routing
                .request
                .as_ref()
                .map(|feedback| feedback.request.id.clone())
        );
    }
}
