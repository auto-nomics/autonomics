//! High-level facade for environment (image) development.
//!
//! `EnvironmentDevInfra` mirrors [`crate::RsiInfra`]: it owns the durable
//! environment store and routes validation, catalog activation, and trusted
//! publication behind host-only adapters. Agents reach it through the
//! `environment_*` tools; they never invoke git, Podman, or the registry.

use std::{
    path::Path,
    sync::{Arc, RwLock},
};

use container_runtime::{ImageBuildConnection, PodmanConnection};

use crate::{
    EnvironmentRegistry, Error, EnvironmentValidationReport, RequestRecord, RequestStatus,
    RequestStore, Result,
    env_manifest::EnvironmentStatus,
    env_store::{EnvironmentOperator, EnvironmentStore, InstalledEnvironment},
    image_registry::SharedImagePublisher,
};

/// Result of one environment validation cycle.
#[derive(Debug, Clone)]
pub enum EnvironmentValidationOutcome {
    /// All requested gates passed and the workspace stayed editable.
    Passed(EnvironmentValidationReport),
    /// A gate failed or was blocked and the workspace returned to repair.
    NeedsFix(EnvironmentValidationReport),
}

/// Result of the fast local-activation path.
#[derive(Debug, Clone)]
pub enum EnvironmentLocalActivationOutcome {
    /// Validation passed and the built image is catalog-active.
    Activated(EnvironmentValidationReport, InstalledEnvironment),
    /// Validation failed or was blocked; the workspace remains repairable.
    NeedsFix(EnvironmentValidationReport),
}

#[derive(Default)]
struct EnvironmentConnections {
    builder: Option<Arc<dyn ImageBuildConnection>>,
    runner: Option<Arc<dyn PodmanConnection>>,
}

/// One trusted environment-development subsystem.
#[derive(Clone)]
pub struct EnvironmentDevInfra {
    store: Arc<EnvironmentStore>,
    environments: EnvironmentRegistry,
    requests: RequestStore,
    publisher: SharedImagePublisher,
    connections: Arc<RwLock<EnvironmentConnections>>,
}

impl EnvironmentDevInfra {
    /// Open durable stores beneath one state directory.
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        state_dir: &Path,
        default_branch: &str,
        author_name: &str,
        author_email: &str,
        local_namespace: &str,
        environments: EnvironmentRegistry,
        publisher: SharedImagePublisher,
    ) -> Result<Self> {
        Ok(Self {
            store: Arc::new(EnvironmentStore::open(
                state_dir,
                default_branch,
                author_name,
                author_email,
                local_namespace,
            )),
            environments,
            requests: RequestStore::open(state_dir),
            publisher,
            connections: Arc::new(RwLock::new(Default::default())),
        })
    }

    pub fn store(&self) -> Arc<EnvironmentStore> {
        Arc::clone(&self.store)
    }

    /// Return the host-owned mutable environment allow list.
    pub fn environment_registry(&self) -> EnvironmentRegistry {
        self.environments.clone()
    }

    pub fn requests(&self) -> &RequestStore {
        &self.requests
    }

    /// Configure the trusted image builder used by validation and push.
    pub fn configure_builder(&self, builder: Arc<dyn ImageBuildConnection>) {
        self.lock_connections(|connections| {
            connections.builder = Some(builder);
        });
    }

    /// Configure the container runner used by the smoke gate.
    pub fn configure_runner(&self, runner: Arc<dyn PodmanConnection>) {
        self.lock_connections(|connections| {
            connections.runner = Some(runner);
        });
    }

    fn builder(&self) -> Option<Arc<dyn ImageBuildConnection>> {
        self.lock_connections(|connections| connections.builder.clone())
    }

    fn runner(&self) -> Option<Arc<dyn PodmanConnection>> {
        self.lock_connections(|connections| connections.runner.clone())
    }

    fn lock_connections<T>(&self, operate: impl FnOnce(&mut EnvironmentConnections) -> T) -> T {
        let mut connections = self
            .connections
            .write()
            .expect("environment connections lock poisoned");
        operate(&mut connections)
    }

    /// Record demand and materialize a new environment workspace.
    pub fn create_environment(
        &self,
        request: RequestRecord,
        base_environment_id: &str,
    ) -> Result<EnvironmentOperator<'_>> {
        let request = self.requests.record(request)?;
        let environment_id = request
            .plugin_name
            .clone()
            .ok_or_else(|| {
                Error::InvalidRequest("environment requests must name their environment".into())
            })?;
        self.ensure_request_environment(&request, &environment_id)?;
        self.store.create(
            &environment_id,
            base_environment_id,
            std::slice::from_ref(&request.id),
            &request.body,
            &self.requests,
            &self.catalog(),
        )?;
        self.requests
            .set_status(&request.id, RequestStatus::Working)?;
        self.develop(&environment_id)
    }

    /// Fork an existing environment workspace and claim the request.
    pub fn fork_environment(
        &self,
        source_environment_id: &str,
        request: RequestRecord,
    ) -> Result<EnvironmentOperator<'_>> {
        let request = self.requests.record(request)?;
        let target_id = request.plugin_name.clone().ok_or_else(|| {
            Error::InvalidRequest("environment fork requests must name their environment".into())
        })?;
        self.ensure_request_environment(&request, &target_id)?;
        self.store.fork(
            source_environment_id,
            &target_id,
            std::slice::from_ref(&request.id),
            &request.body,
            &self.requests,
        )?;
        self.requests
            .set_status(&request.id, RequestStatus::Working)?;
        self.develop(&target_id)
    }

    /// Start an in-place update of a catalog-active environment.
    pub fn start_environment_update(
        &self,
        environment_id: &str,
        request: RequestRecord,
    ) -> Result<EnvironmentOperator<'_>> {
        let request = self.requests.record(request)?;
        self.ensure_request_environment(&request, environment_id)?;
        self.store.create_update(
            environment_id,
            std::slice::from_ref(&request.id),
            &request.body,
            &self.requests,
            &self.environments,
        )?;
        self.requests
            .set_status(&request.id, RequestStatus::Working)?;
        self.develop(environment_id)
    }

    /// Run the four deterministic static gates, leaving the workspace editable.
    pub async fn validate_environment_local(
        &self,
        environment_id: &str,
    ) -> Result<EnvironmentValidationOutcome> {
        self.validate_common(environment_id, false).await
    }

    /// Run all six gates, including the infrastructure build and smoke run.
    pub async fn build_environment(
        &self,
        environment_id: &str,
    ) -> Result<EnvironmentValidationOutcome> {
        self.validate_common(environment_id, true).await
    }

    /// Validate fully and immediately activate the built image in the catalog.
    ///
    /// Human review and registry publication are intentionally not on this
    /// path; the background distiller converts the local activation into a
    /// published reference later.
    pub async fn install_environment_local(
        &self,
        environment_id: &str,
    ) -> Result<EnvironmentLocalActivationOutcome> {
        match self.validate_common(environment_id, true).await? {
            EnvironmentValidationOutcome::NeedsFix(report) => {
                Ok(EnvironmentLocalActivationOutcome::NeedsFix(report))
            }
            EnvironmentValidationOutcome::Passed(report) => {
                let request_ids = self
                    .develop(environment_id)?
                    .manifest()
                    .lifecycle
                    .request_ids
                    .clone();
                let installed = self.store.install_local(environment_id, &self.environments)?;
                for id in request_ids {
                    self.requests
                        .set_status(&id, RequestStatus::ReviewPending)?;
                }
                Ok(EnvironmentLocalActivationOutcome::Activated(report, installed))
            }
        }
    }

    /// Apply a review decision and propagate request state.
    pub fn review_environment(&self, environment_id: &str, approved: bool) -> Result<EnvironmentStatus> {
        let mut operator = self.develop(environment_id)?;
        let request_ids = operator.manifest().lifecycle.request_ids.clone();
        let to = if approved {
            EnvironmentStatus::Approved
        } else {
            EnvironmentStatus::Rejected
        };
        operator.transition_snapshot(to, "environment: review decision")?;
        let request_status = if approved {
            RequestStatus::Consumed
        } else {
            RequestStatus::Rejected
        };
        for id in request_ids {
            self.requests.set_status(&id, request_status)?;
        }
        Ok(to)
    }

    /// Push a reviewed environment and re-pin the catalog to the remote digest.
    pub async fn publish_reviewed_environment(
        &self,
        environment_id: &str,
    ) -> Result<EnvironmentStatus> {
        let mut operator = self.develop(environment_id)?;
        if operator.status() != EnvironmentStatus::Approved {
            return Err(Error::InvalidTransition {
                from: format!("{:?}", operator.status()),
                to: "publishing".into(),
            });
        }
        if !operator.repository().is_clean()? {
            return Err(Error::Validation(
                "environment workspace has uncommitted content".into(),
            ));
        }
        let local_reference = operator
            .manifest()
            .lifecycle
            .local_reference
            .clone()
            .ok_or_else(|| {
                Error::Validation("environment has no locally built image to publish".into())
            })?;
        operator.transition_snapshot(
            EnvironmentStatus::Publishing,
            "environment: start publication",
        )?;

        let published = match self
            .publisher
            .push_environment(environment_id, &local_reference)
            .await
        {
            Ok(published) => published,
            Err(error) => {
                let _ = operator.mutate_manifest(|manifest| {
                    manifest.status = EnvironmentStatus::PublishFailed;
                });
                operator
                    .snapshot("environment: record publication failure")?;
                return Err(error);
            }
        };
        let previous = self.environments.get(environment_id)?;
        operator.mutate_manifest(|manifest| {
            manifest.status = EnvironmentStatus::Published;
            manifest.lifecycle.published_reference = Some(published.reference.clone());
            if manifest.lifecycle.base_reference.is_none() {
                manifest.lifecycle.base_reference =
                    previous.as_ref().map(|environment| environment.reference.clone());
            }
        })?;
        self.environments.approve(
            environment_id,
            crate::Environment {
                reference: published.reference,
                interpreters: operator.manifest().interpreters.clone(),
            },
        )?;
        operator.snapshot("environment: publish reviewed image")?;
        operator.transition_snapshot(
            EnvironmentStatus::InstallPending,
            "environment: finalize publication",
        )?;
        operator.transition_snapshot(
            EnvironmentStatus::Installed,
            "environment: install published reference",
        )?;
        operator.mutate_manifest(|manifest| {
            manifest.lifecycle.publication_pending = false;
        })?;
        operator.snapshot("environment: clear pending publication")?;
        Ok(EnvironmentStatus::Installed)
    }

    /// Roll the catalog back to the reference the current activation replaced.
    pub fn rollback_environment(&self, environment_id: &str) -> Result<String> {
        self.store.rollback(environment_id, &self.environments)
    }

    /// Remove the environment from the catalog, retaining its workspace.
    pub fn uninstall_environment(&self, environment_id: &str) -> Result<crate::Environment> {
        self.store.uninstall(environment_id, &self.environments)
    }

    fn develop(&self, environment_id: &str) -> Result<EnvironmentOperator<'_>> {
        self.store.develop(environment_id)?.ok_or_else(|| {
            Error::Validation(format!("unknown environment `{environment_id}`"))
        })
    }

    async fn validate_common(
        &self,
        environment_id: &str,
        full: bool,
    ) -> Result<EnvironmentValidationOutcome> {
        let store = self.store();
        let mut operator = self.develop(environment_id)?;
        let development_status = operator.status();
        operator.transition_snapshot(
            EnvironmentStatus::Validating,
            "environment: start validation",
        )?;
        let (attempt, _tag) = operator.next_attempt_tag()?;

        let builder = if full {
            self.builder()
        } else {
            None
        };
        let builder = builder.as_deref();
        let runner = if full {
            self.runner()
        } else {
            None
        };
        let runner = runner.as_deref();
        let report = crate::validate_environment(
            environment_id,
            &operator.workspace(),
            store.local_namespace(),
            if full {
                crate::env_validate::ValidationDepth::Full
            } else {
                crate::env_validate::ValidationDepth::Static
            },
            builder,
            runner,
            attempt,
        )
        .await;
        report.write(&operator.reports_dir())?;
        // `local_reference` is the digest-pinned local catalog form — the
        // push target and rollback input — not the mutable build tag.
        let pinned_local = match &report.digest {
            Some(digest) => Some(store.local_reference(environment_id, digest)?),
            None => None,
        };
        operator.mutate_manifest(|manifest| {
            manifest.lifecycle.latest_report = Some(format!(".rsi/reports/{0}.json", report.report_id));
            if let Some(reference) = &pinned_local {
                manifest.lifecycle.local_reference = Some(reference.clone());
            }
        })?;

        if !report.passed() {
            operator.mutate_manifest(|manifest| {
                manifest.status = EnvironmentStatus::NeedsFix;
                manifest.lifecycle.publication_pending = false;
            })?;
            return Ok(EnvironmentValidationOutcome::NeedsFix(report));
        }

        operator
            .snapshot(&format!("validation: {}", report.report_id))?;
        if !operator.repository().is_clean()? {
            return Err(Error::Validation(
                "environment workspace has uncommitted content".into(),
            ));
        }
        let restore = if development_status == EnvironmentStatus::Updating {
            EnvironmentStatus::Updating
        } else {
            EnvironmentStatus::Draft
        };
        operator.transition_snapshot(
            restore,
            "environment: validation passed; remain editable",
        )?;
        Ok(EnvironmentValidationOutcome::Passed(report))
    }

    fn catalog(&self) -> crate::EnvironmentCatalog {
        self.environments.snapshot()
    }

    fn ensure_request_environment(
        &self,
        request: &RequestRecord,
        environment_id: &str,
    ) -> Result<()> {
        if request.plugin_name.as_deref() == Some(environment_id) {
            Ok(())
        } else {
            Err(Error::InvalidRequest(format!(
                "request `{}` targets a different environment",
                request.id
            )))
        }
    }
}
