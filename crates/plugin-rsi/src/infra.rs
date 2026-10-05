//! High-level facade for plugin-based recursive self-improvement.
//!
//! `RsiInfra` is the system-facing authority. It owns the durable stores and
//! routes trusted publishers and runtime registration behind host-only
//! adapters, so callers never reassemble the lifecycle from its pieces.

use std::sync::{Arc, RwLock};

use crate::{
    EnvironmentCatalog, Error, PluginLifecycle, PluginPublisher, PluginPullRequestPublisher,
    PluginStatus, PluginStore, RequestRecord, RequestStatus, RequestStore, Result,
    ValidationOutcome,
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
    environments: Arc<RwLock<Arc<EnvironmentCatalog>>>,
    publisher: Arc<RwLock<Option<SharedPluginPublisher>>>,
    pull_request_publisher: Arc<RwLock<Option<SharedPullRequestPublisher>>>,
    registry: Arc<RwLock<Option<Arc<dyn PluginRegistryControl>>>>,
}

impl RsiInfra {
    /// Open durable stores beneath one state directory.
    pub fn open(
        state_dir: &std::path::Path,
        default_branch: &str,
        author_name: &str,
        author_email: &str,
    ) -> Self {
        Self {
            store: Arc::new(PluginStore::open(
                state_dir,
                default_branch,
                author_name,
                author_email,
            )),
            requests: RequestStore::open(state_dir),
            environments: Arc::new(RwLock::new(Arc::new(EnvironmentCatalog::default()))),
            publisher: Arc::new(RwLock::new(None)),
            pull_request_publisher: Arc::new(RwLock::new(None)),
            registry: Arc::new(RwLock::new(None)),
        }
    }

    pub fn store(&self) -> Arc<PluginStore> {
        Arc::clone(&self.store)
    }

    pub fn requests(&self) -> &RequestStore {
        &self.requests
    }

    pub fn configure_environments(&self, catalog: EnvironmentCatalog) {
        *self
            .environments
            .write()
            .expect("RSI environment lock poisoned") = Arc::new(catalog);
    }

    pub fn configure_publisher(&self, publisher: SharedPluginPublisher) {
        *self.publisher.write().expect("publisher lock poisoned") = Some(publisher);
    }

    pub fn configure_pull_request_publisher(&self, publisher: SharedPullRequestPublisher) {
        *self
            .pull_request_publisher
            .write()
            .expect("pull-request publisher lock poisoned") = Some(publisher);
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
        lifecycle.publish_reviewed(self.publisher()?.as_ref())
    }

    pub fn open_update_pull_request(&self, plugin_name: &str) -> Result<PluginStatus> {
        let installed_kinds = self.installed_node_kinds()?;
        let mut operator = self
            .store
            .develop(plugin_name)?
            .ok_or_else(|| Error::Validation(format!("unknown plugin `{plugin_name}`")))?;
        let catalog = self.catalog();
        let mut lifecycle = PluginLifecycle::new(&mut operator, &catalog, &installed_kinds);
        lifecycle.open_update_pull_request(self.pull_request_publisher()?.as_ref())
    }

    pub fn merge_update_pull_request(&self, plugin_name: &str) -> Result<PluginStatus> {
        let installed_kinds = self.installed_node_kinds()?;
        let mut operator = self
            .store
            .develop(plugin_name)?
            .ok_or_else(|| Error::Validation(format!("unknown plugin `{plugin_name}`")))?;
        let catalog = self.catalog();
        let mut lifecycle = PluginLifecycle::new(&mut operator, &catalog, &installed_kinds);
        lifecycle.merge_update_pull_request(self.pull_request_publisher()?.as_ref())
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
            lifecycle.install(self.publisher()?.as_ref())?
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

    fn publisher(&self) -> Result<SharedPluginPublisher> {
        self.publisher
            .read()
            .expect("publisher lock poisoned")
            .clone()
            .ok_or_else(|| Error::Validation("plugin publisher is not configured".into()))
    }

    fn pull_request_publisher(&self) -> Result<SharedPullRequestPublisher> {
        self.pull_request_publisher
            .read()
            .expect("pull-request publisher lock poisoned")
            .clone()
            .ok_or_else(|| Error::Validation("pull-request publisher is not configured".into()))
    }

    fn catalog(&self) -> Arc<EnvironmentCatalog> {
        Arc::clone(
            &*self
                .environments
                .read()
                .expect("RSI environment lock poisoned"),
        )
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

    #[test]
    fn creating_plugin_claims_request_through_the_unified_facade() {
        let state = tempfile::tempdir().unwrap();
        let infra = RsiInfra::open(state.path(), "main", "Test", "test@example.com");
        let mut catalog = EnvironmentCatalog::default();
        catalog.insert(
            "alpine",
            Environment {
                reference: "docker.io/library/alpine@sha256:0123456789012345678901234567890123456789012345678901234567890123".into(),
                interpreters: vec!["sh".into()],
            },
        );
        infra.configure_environments(catalog);
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
}
