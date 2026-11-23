//! Operator-owned compute selection. Storage namespaces have an independent lifetime.
mod backend;
mod cluster_connection;
mod data_source;
mod domain;
mod enroll;
mod readiness;
mod routing;
pub use readiness::ReadinessRequest;
#[cfg(test)]
mod tests;
pub mod tools;
mod worker;

mod built;
mod persistence;
mod state;

pub use enroll::{
    EnrollBackend, EnrollContainerRuntime, EnrollProgress, EnrollProgressEvent, EnrollRequest,
    EnrollStage,
};
pub use worker::RepairWorkerRequest;

use crate::{RuntimeConfig, config::SlurmConfig};
use dag_core::{RemoteTaskExecutor, TaskExecutor, dag::execution::slurm_backed_executor};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
};

pub(crate) use backend::same_job_connection;
pub use backend::{ComputeBackend, ComputeTarget};
pub use built::{ComputeImageEvidence, ComputeStatus, ExecutionBinding, SaveTarget, SelectTarget};
pub(crate) use data_source::source_address_domain;
pub use data_source::{DataBackend, DataSource, SaveDataSource, SaveTargetDataSource};
pub use domain::DataDomain;
use domain::is_domain_absolute;
pub use routing::ComputeExecutor;
pub(crate) use routing::ContainerRouter;
#[cfg(test)]
pub(crate) use routing::ImageBuilder as ComputeImageBuilder;

pub(crate) use built::BuiltTarget;
use persistence::{
    MAX_CONFIGURATION_BYTES, read_owned, read_private, write_binding, write_bytes_new, write_state,
};
pub(crate) use persistence::{identifier, private_dir, write_new};
pub(crate) use state::startup_config;
pub use state::{ComputePolicy, ComputeRegistryState};
use state::{decode_state, read_registry_file};

pub struct ComputeRegistry {
    root: PathBuf,
    config: RuntimeConfig,
    storage: Arc<vfs::OpendalFileStorage>,
    vfs: Arc<vfs::MountedObjectStore>,
    local: Arc<dyn container_runtime::PodmanConnection>,
    local_executor: Arc<dyn TaskExecutor>,
    state: RwLock<ComputeRegistryState>,
    built: RwLock<BTreeMap<String, Arc<BuiltTarget>>>,
    mutation: Mutex<()>,
    poisoned: std::sync::atomic::AtomicBool,
    pub monitors: Arc<crate::hpc_monitor::MonitorHub>,
}

impl ComputeRegistry {
    /// Hold the owning target while attaching a monitor, including the period
    /// before the monitor's durable record exists.
    pub async fn watch_existing_job(
        &self,
        target_id: &str,
        cluster: Option<&str>,
        submission_key: &str,
        owner: crate::hpc_monitor::Owner,
        plan: serde_json::Value,
    ) -> Result<hpc_tools::monitor::MonitorRecord, String> {
        let target = self.target(target_id)?;
        let hub = self.monitors.target_hub(&target.target.id)?;
        let result = hub
            .watch_existing(cluster, submission_key, owner, plan)
            .await;
        drop(target);
        result
    }

    /// Inspect the current in-memory runtime and its admitted image mapping.
    /// Never reread a mutable inventory or expose private profile contents.
    pub fn image_evidence(
        &self,
        id: &str,
        reference: &str,
    ) -> Result<ComputeImageEvidence, String> {
        let target = self.target(id)?;
        let resolution = target
            .containers
            .image_resolution(reference)
            .map_err(|e| e.to_string())?;
        Ok(ComputeImageEvidence {
            target_id: target.target.id.clone(),
            target_sha256: format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&target.target).map_err(|e| e.to_string())?)
            ),
            capsule_sha256: target.target.capsule_sha256.clone(),
            resolution,
            evidence_scope: "RESIDENT_RUNTIME_MAPPING_ONLY".into(),
            sif_bytes_verified: false,
            node_execution_performed: false,
        })
    }
    pub(crate) fn open(
        config: &RuntimeConfig,
        storage: Arc<vfs::OpendalFileStorage>,
        vfs: Arc<vfs::MountedObjectStore>,
        containers: &container_runtime::ContainerExecutionInfra,
        monitors: Arc<crate::hpc_monitor::MonitorHub>,
    ) -> Result<Arc<Self>, String> {
        let root = config.state_dir.join("compute");
        private_dir(&root)?;
        for dir in ["runs", "targets"] {
            private_dir(&root.join(dir))?;
        }

        let saved = read_registry_file(&root)?;
        let is_new = saved.is_none();
        let mut state = match saved {
            Some(bytes) => decode_state(&bytes)?,
            None => ComputeRegistryState::initial(config),
        };

        // A single gateway already owns state_dir; mutations additionally serialize here.
        let registry = Arc::new(Self {
            root,
            config: config.clone(),
            storage,
            vfs,
            local: containers.runtime.clone(),
            local_executor: Arc::new(
                dag_core::dag::execution::LimitedTaskExecutor::new(
                    Arc::new(dag_core::LocalTaskExecutor::with_workspace_root(
                        dag_core::dag_tasks_root_for_state_dir(&config.state_dir),
                    )),
                    std::thread::available_parallelism()
                        .map(|n| n.get().saturating_sub(2).max(1).min(1024))
                        .unwrap_or(1),
                )
                .map_err(|e| e.to_string())?,
            ),
            state: RwLock::new(state.clone()),
            built: RwLock::new(BTreeMap::new()),
            mutation: Mutex::new(()),
            poisoned: std::sync::atomic::AtomicBool::new(false),
            monitors,
        });
        for source in state.all_data_sources() {
            registry
                .vfs
                .add_data_mount(&source.manifest()?)
                .map_err(|e| e.to_string())?;
        }
        let mut built_targets = BTreeMap::new();
        let mut target_ids = std::collections::BTreeSet::new();
        for target in &mut state.targets {
            if is_new {
                let pin_endpoints = target.id != "legacy-slurm";
                registry.freeze_profile(target, pin_endpoints, None)?;
            }
            identifier(&target.id)?;
            if target.name.trim().is_empty() {
                return Err("invalid compute target".into());
            }
            if !target_ids.insert(target.id.clone()) {
                return Err("duplicate compute target id".into());
            }
            if state.deleted_target_ids.contains(&target.id) {
                registry.restore_deleted_target_evidence(target)?;
                if let ComputeBackend::Slurm { settings } = &target.backend {
                    registry.monitors.add(
                        &target.id,
                        registry.slurm_account_client(target, settings)?,
                        std::time::Duration::from_secs(settings.poll_interval_secs),
                    )?;
                }
                continue;
            }
            let built = registry.build(target.clone())?;
            registry.observe(&built)?;
            built_targets.insert(target.id.clone(), built);
        }
        *registry.built.write().unwrap() = built_targets;
        *registry.state.write().unwrap() = state.clone();
        registry.target(&state.policy.target_id)?;
        if is_new {
            registry.persist(&state)?;
        }
        Ok(registry)
    }

    fn healthy(&self) -> Result<(), String> {
        if self.poisoned.load(std::sync::atomic::Ordering::Acquire) {
            return Err("compute persistence outcome uncertain; restart and reconcile before admitting work".into());
        }
        Ok(())
    }
    fn persist(&self, state: &ComputeRegistryState) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(state).map_err(|error| error.to_string())?;
        if bytes.len() as u64 > MAX_CONFIGURATION_BYTES {
            return Err("compute registry exceeds 4 MiB".into());
        }
        let result = write_state(&self.root, &bytes);
        if result.is_err() {
            // A storage failure must not let new work outlive a damaged or
            // non-durable registry state. Already-bound work retains its own backend.
            self.poisoned
                .store(true, std::sync::atomic::Ordering::Release);
        }
        result
    }
    pub fn status(&self) -> Result<ComputeStatus, String> {
        self.healthy()?;
        let (state, deployment) = {
            let state = self.state.read().unwrap();
            let target = self
                .built
                .read()
                .unwrap()
                .get(&state.policy.target_id)
                .cloned()
                .ok_or("selected target is missing")?;
            (state.clone(), target.deployment())
        };
        Ok(ComputeStatus {
            deployment,
            state,
            mounts: self.vfs.mount_definitions(),
        })
    }
    pub fn state(&self) -> ComputeRegistryState {
        self.state.read().unwrap().clone()
    }

    /// Read-only connection inspection. Never allocates a compute job.
    pub async fn check_target(&self, id: &str) -> Result<serde_json::Value, String> {
        let target = self.target(id)?;
        let deployment = target.deployment();
        if let Err(error) = target.ensure_source_compatible() {
            return Ok(serde_json::json!({"target_id": id,
                "configuration_valid": false, "deployment": deployment, "error": error,
                "connection_observation": null, "worker_and_image_execution_verified": false,
                "new_job_submitted": false, "scientific_admission_granted": false}));
        }
        let observation = if let Some(client) = &target.hpc {
            Some(client.run_json("import os,socket,json; print(json.dumps({'hostname':socket.gethostname(),'uid':os.getuid()}))", &serde_json::json!({}),false).await?)
        } else if let Some(ssh) = &target.ssh_probe {
            let probe = ssh
                .run("hostname && id -u")
                .await
                .map_err(|e| format!("direct host connection check failed: {e}"))?;
            let text = String::from_utf8_lossy(&probe.stdout);
            let mut lines = text.lines();
            let hostname = lines.next().unwrap_or_default().trim().to_owned();
            let uid: i64 = lines
                .next()
                .and_then(|line| line.trim().parse().ok())
                .unwrap_or_default();
            if hostname.is_empty() {
                return Err("direct host connection check returned no identity".into());
            }
            Some(serde_json::json!({"hostname": hostname, "uid": uid}))
        } else {
            None
        };
        Ok(
            serde_json::json!({"target_id":target.target.id,"configuration_valid":true,"deployment":deployment,"connection_observation":observation,"worker_and_image_execution_verified":false,"new_job_submitted":false,"scientific_admission_granted":false}),
        )
    }

    pub(crate) fn selected(&self) -> Result<(Arc<BuiltTarget>, ComputePolicy), String> {
        self.healthy()?;
        let state = self.state.read().unwrap();
        let target = self
            .built
            .read()
            .unwrap()
            .get(&state.policy.target_id)
            .cloned()
            .ok_or("selected target is missing")?;
        if !target.target.enabled || state.deleted_target_ids.contains(&target.target.id) {
            return Err("selected compute target is disabled; select another target".into());
        }
        target.ensure_source_compatible()?;
        Ok((target, state.policy.clone()))
    }
    pub(crate) fn target(&self, id: &str) -> Result<Arc<BuiltTarget>, String> {
        self.healthy()?;
        let _state = self.state.read().unwrap();
        self.built
            .read()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or("unknown target".into())
    }
    pub fn select(&self, request: SelectTarget) -> Result<ComputeRegistryState, String> {
        let _guard = self.mutation.lock().map_err(|_| "compute lock poisoned")?;
        self.healthy()?;
        let mut next = self.state();
        if next.deleted_target_ids.contains(&request.target_id) {
            return Err("target is deleted".into());
        }
        let t = next
            .targets
            .iter()
            .rev()
            .find(|t| t.id == request.target_id && t.enabled)
            .ok_or("target is missing or disabled")?;
        self.target(&t.id)?.ensure_source_compatible()?;
        next.policy = ComputePolicy {
            target_id: t.id.clone(),
        };
        self.persist(&next)?;
        *self.state.write().unwrap() = next.clone();
        Ok(next)
    }
    pub fn save_target(&self, request: SaveTarget) -> Result<ComputeRegistryState, String> {
        self.save_target_with_source(request, None, None)
    }

    fn save_target_with_source(
        &self,
        request: SaveTarget,
        mounts: Option<(DataSource, Option<DataSource>)>,
        expected_target: Option<&ComputeTarget>,
    ) -> Result<ComputeRegistryState, String> {
        let _guard = self.mutation.lock().map_err(|_| "compute lock poisoned")?;
        self.healthy()?;
        identifier(&request.id)?;
        if request.name.trim().is_empty() {
            return Err("target name is required".into());
        }
        let mut next = self.state();
        if next.deleted_target_ids.contains(&request.id) {
            return Err("target id is reserved by deleted history".into());
        }
        if request.data_domains.len() > 1 {
            return Err("at most one data domain per target is supported".into());
        }
        for domain in &request.data_domains {
            domain.validate()?;
        }
        let existing = next.targets.iter().find(|target| target.id == request.id);
        if let Some(expected) = expected_target
            && existing
                .map(serde_json::to_value)
                .transpose()
                .map_err(|e| e.to_string())?
                != Some(serde_json::to_value(expected).map_err(|e| e.to_string())?)
        {
            return Err(
                "target changed while preparing the mount; reopen the editor and retry".into(),
            );
        }
        let previous_source = existing.and_then(|target| target.data_source.clone());
        let (data_source, retained) = match mounts {
            Some((source, retained)) => {
                if !source.matches_target_domain(&request.data_domains) {
                    return Err("data source does not match the target domain".into());
                }
                (Some(source), retained)
            }
            None => (previous_source.clone(), None),
        };
        let mut target = ComputeTarget {
            id: request.id,
            name: request.name,
            enabled: request.enabled,
            backend: request.backend,
            data_source,
            retained_data_sources: existing
                .map(|target| target.retained_data_sources.clone())
                .unwrap_or_default(),
            data_domains: request.data_domains,
            capsule_sha256: BTreeMap::new(),
            endpoints: BTreeMap::new(),
        };
        if let Some(existing) = existing {
            next.data_sources.retain(|source| {
                if source.is_retained_mount_for(existing) {
                    target.retained_data_sources.push(source.clone());
                    false
                } else {
                    true
                }
            });
        }
        self.freeze_profile(&mut target, true, existing)?;
        // Block admission until the new configuration is published. Existing
        // holders of the old executor remain independent of this lock.
        let mut state = self.state.write().unwrap();
        self.check_job_connection(&target)?;
        let mut added_paths = Vec::new();
        let built = (|| {
            if let Some(source) = &target.data_source
                && previous_source
                    .as_ref()
                    .is_none_or(|previous| previous.id != source.id)
            {
                self.mount_source(&next, source)?;
                added_paths.push(source.mount_path());
                target.retained_data_sources.extend(previous_source);
            }
            if let Some(source) = retained {
                self.mount_source(&next, &source)?;
                added_paths.push(source.mount_path());
                target.retained_data_sources.push(source);
            }
            let built = self.build(target.clone())?;
            if let Some(existing) = next
                .targets
                .iter_mut()
                .find(|existing| existing.id == target.id)
            {
                *existing = target;
            } else {
                next.targets.push(target);
            }
            self.persist(&next)?;
            Ok::<_, String>(built)
        })()
        .inspect_err(|_| {
            if !added_paths.is_empty() && self.vfs.remove_data_mounts(&added_paths).is_err() {
                self.poisoned
                    .store(true, std::sync::atomic::Ordering::Release);
            }
        })?;
        self.observe(&built).inspect_err(|_| {
            self.poisoned
                .store(true, std::sync::atomic::Ordering::Release);
        })?;
        self.built
            .write()
            .unwrap()
            .insert(built.target.id.clone(), built);
        *state = next.clone();
        Ok(next)
    }

    /// Disable selection while preserving access to existing job records.
    pub fn delete_target(&self, id: &str) -> Result<ComputeRegistryState, String> {
        let _guard = self.mutation.lock().map_err(|_| "compute lock poisoned")?;
        self.healthy()?;
        let mut next = self.state();
        if id == "local" {
            return Err("the local target cannot be deleted".into());
        }
        if next.policy.target_id == id {
            return Err("select another target before deleting the active target".into());
        }
        let target = next
            .targets
            .iter()
            .find(|target| target.id == id)
            .filter(|_| !next.deleted_target_ids.contains(id))
            .ok_or("unknown target")?;
        let paths: Vec<_> = target
            .data_source
            .iter()
            .chain(&target.retained_data_sources)
            .chain(
                next.data_sources
                    .iter()
                    .filter(|source| source.is_retained_mount_for(target)),
            )
            .map(DataSource::mount_path)
            .collect();
        if next.targets.iter().any(|other| {
            other.id != id
                && !next.deleted_target_ids.contains(&other.id)
                && other.data_domains.iter().any(|domain| {
                    std::iter::once(domain.vfs_prefix.as_str())
                        .chain(domain.retained_vfs_prefix.as_deref())
                        .any(|prefix| paths.iter().any(|path| Path::new(prefix).starts_with(path)))
                })
        }) {
            return Err("another compute target uses this data mount".into());
        }
        let mut state = self.state.write().unwrap();
        let mut built = self.built.write().unwrap();
        if !paths.is_empty()
            && built
                .get(id)
                .is_some_and(|target| Arc::strong_count(target) > 1)
        {
            return Err("target is in use; wait for current operations before deleting it".into());
        }
        next.deleted_target_ids.insert(id.to_string());
        self.persist(&next)?;
        if !paths.is_empty() {
            self.vfs.remove_data_mounts(&paths).map_err(|error| {
                self.poisoned
                    .store(true, std::sync::atomic::Ordering::Release);
                error.to_string()
            })?;
        }
        built.remove(id);
        *state = next.clone();
        Ok(next)
    }
    fn freeze_profile(
        &self,
        target: &mut ComputeTarget,
        pin_endpoints: bool,
        previous: Option<&ComputeTarget>,
    ) -> Result<(), String> {
        let settings = match &mut target.backend {
            ComputeBackend::Slurm { settings } | ComputeBackend::Ssh { settings } => settings,
            ComputeBackend::Local => return Ok(()),
        };
        if let Some(path) = settings.typed_worker_config.take() {
            if !path.is_absolute() {
                return Err("typed-worker profile must be absolute".into());
            }
            let bytes = read_owned(&path, false)?;
            settings.typed_worker =
                Some(serde_json::from_slice(&bytes).map_err(|error| error.to_string())?);
        }
        let target_dir = self.root.join("targets").join(&target.id);
        private_dir(&target_dir)?;
        // Resolve and freeze non-secret endpoints for each saved configuration.
        // Data-domain aliases are pinned with the same treatment: in-place
        // references re-verify identity against these frozen endpoints.
        let domain_aliases: Vec<&String> = target
            .data_domains
            .iter()
            .flat_map(|domain| domain.ssh_aliases.iter())
            .collect();
        for alias in std::iter::once(&settings.ssh_host)
            .chain(settings.data_ssh_host.iter())
            .chain(settings.pi_ssh_host.iter())
            .chain(domain_aliases.iter().copied())
            .filter(|_| pin_endpoints)
        {
            target.endpoints.insert(
                alias.clone(),
                vfs::SshEndpoint::capture(&settings.ssh_command, alias)?,
            );
        }
        if let Some(profile) = &settings.typed_worker {
            let canonical = serde_json::to_vec(profile).map_err(|error| error.to_string())?;
            crate::host::hpc_placement::decode_profile(&canonical)
                .map_err(|error| error.to_string())?;
            target.capsule_sha256.insert(
                "worker.json".into(),
                format!("{:x}", Sha256::digest(canonical)),
            );
        }
        if let Some(source) = &mut settings.apptainer_manifest {
            if let Some(previous) = previous
                && let ComputeBackend::Slurm { settings } = &previous.backend
                && settings.apptainer_manifest.as_ref() == Some(source)
                && let Some(digest) = previous.capsule_sha256.get("images.json")
            {
                target
                    .capsule_sha256
                    .insert("images.json".into(), digest.clone());
                return Ok(());
            }
            let bytes = read_owned(source, false)?;
            let _: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            let dir = target_dir.join(format!("profile-{}", uuid::Uuid::new_v4()));
            private_dir(&dir)?;
            let frozen = dir.join("images.json");
            write_bytes_new(&frozen, &bytes)?;
            target.capsule_sha256.insert(
                "images.json".into(),
                format!("{:x}", Sha256::digest(&bytes)),
            );
            *source = frozen;
        }
        Ok(())
    }

    fn mount_execution_evidence(&self, root: &std::path::Path) -> Result<(), String> {
        if root == self.config.state_dir || root == self.root {
            return Err("execution evidence requires a dedicated directory".into());
        }
        let mut manifest = vfs::VfsManifest::default();
        manifest
            .ensure_read_only_local_identity_mount(root)
            .map_err(|error| error.to_string())?;
        let path = root.to_string_lossy();
        if self.storage.local_host_path(&path).as_deref() == Some(root)
            && self.storage.check_writable(&path).is_err()
        {
            // Preserve an operator's existing, possibly stricter permission policy.
            return Ok(());
        }
        self.vfs
            .add_data_mount(&manifest)
            .map_err(|error| error.to_string())
    }

    fn restore_deleted_target_evidence(&self, target: &ComputeTarget) -> Result<(), String> {
        let settings = match &target.backend {
            ComputeBackend::Ssh { settings } | ComputeBackend::Slurm { settings } => settings,
            ComputeBackend::Local => return Ok(()),
        };
        let dir = self.root.join("targets").join(&target.id);
        let legacy = target.id == "legacy-slurm";
        let workspace = if legacy {
            self.config.state_dir.join("slurm-tasks")
        } else {
            dir.join("tasks")
        };
        let artifacts = if !settings.artifact_root.as_os_str().is_empty() {
            settings.artifact_root.clone()
        } else if legacy {
            self.config.state_dir.join("slurm-artifacts")
        } else {
            dir.join("artifacts")
        };
        for path in [workspace, artifacts] {
            if path.is_dir() {
                self.mount_execution_evidence(&path)?;
            }
        }
        Ok(())
    }

    fn observe(&self, target: &Arc<BuiltTarget>) -> Result<(), String> {
        if let ComputeBackend::Slurm { settings } = &target.target.backend {
            self.monitors.add(
                &target.target.id,
                target.hpc.clone().ok_or("missing account client")?,
                std::time::Duration::from_secs(settings.poll_interval_secs),
            )?;
        }
        Ok(())
    }

    fn check_job_connection(&self, target: &ComputeTarget) -> Result<(), String> {
        let Some(old) = self.built.read().unwrap().get(&target.id).cloned() else {
            return Ok(());
        };
        if same_job_connection(&old.target, target) {
            return Ok(());
        }
        // Include in-flight job-tool calls that have not written a monitor yet.
        // The map and this local clone account for two strong references.
        if Arc::strong_count(&old) > 2 {
            return Err("target is in use; wait for its current operations before changing its SSH connection".into());
        }
        if let Ok(hub) = self.monitors.target_hub(&target.id) {
            if !hub.records()?.is_empty() {
                return Err("target has retained jobs; register a different target id to change its SSH connection".into());
            }
        }
        Ok(())
    }

    fn slurm_account_client(
        &self,
        target: &ComputeTarget,
        settings: &SlurmConfig,
    ) -> Result<Arc<hpc_tools::HpcClient>, String> {
        let dir = self.root.join("targets").join(&target.id);
        private_dir(&dir)?;
        let state_dir = if target.id == "legacy-slurm" {
            self.config.state_dir.join("hpc-account")
        } else {
            dir.join("hpc-account")
        };
        Ok(Arc::new(
            hpc_tools::HpcClient::new(hpc_tools::HpcConfig {
                ssh_host: settings.ssh_host.clone(),
                data_ssh_host: settings.data_ssh_host.clone(),
                pi_ssh_host: settings.pi_ssh_host.clone(),
                ssh_command: settings.ssh_command.clone(),
                remote_python: settings.remote_python.clone(),
                state_dir,
                default_partition: settings.partition.clone(),
                default_account: settings.account.clone(),
            })?
            .with_endpoints(target.endpoints.clone())?,
        ))
    }

    fn build(&self, target: ComputeTarget) -> Result<Arc<BuiltTarget>, String> {
        let target_id = target.id.clone();
        let profile = match &target.backend {
            ComputeBackend::Slurm { settings } | ComputeBackend::Ssh { settings } => {
                crate::host::hpc_placement::configured_profile(settings)
                    .map_err(|error| error.to_string())?
            }
            ComputeBackend::Local => None,
        };
        if let Some(profile) = &profile {
            profile
                .worker
                .validate()
                .map_err(|error| error.to_string())?;
        }
        let network_policy_sha256 = profile.as_ref().and_then(|profile| {
            profile
                .worker
                .native_environment
                .as_ref()
                .and_then(|value| value.network_policy_sha256.clone())
        });
        let mut typed_worker_revision = profile
            .as_ref()
            .map(|profile| profile.worker.expected_source_revision.clone());
        if let ComputeBackend::Slurm { settings } | ComputeBackend::Ssh { settings } =
            &target.backend
        {
            if let Some(root) = &settings.storage_root_v1 {
                if !is_domain_absolute(root)
                    || settings.scratch_remote_dir != format!("{root}/dag-tasks")
                    || settings.panel_cache_remote_dir != format!("{root}/panels")
                    || !target.data_domains.first().is_some_and(|domain| {
                        domain.has_separate_task_mount()
                            && domain.remote_root == format!("{root}/data")
                    })
                {
                    return Err("unified target storage paths do not match its storage root".into());
                }
            }
            if matches!(target.backend, ComputeBackend::Slurm { .. })
                && target.id != "legacy-slurm"
                && profile.is_none()
            {
                return Err("global Slurm targets require a pinned typed-worker profile; process-only routing is retained solely for legacy migration".into());
            }
            if matches!(target.backend, ComputeBackend::Ssh { .. })
                && (settings.apptainer_manifest.is_some()
                    || !settings.sif_runtime.is_default()
                    || settings.singularity_executable.is_some())
            {
                return Err("SSH targets require Podman. SIF runtime settings are only accepted for Slurm targets".into());
            }
            match &target.backend {
                ComputeBackend::Slurm { .. } if settings.apptainer_manifest.is_none() => {
                    return Err(format!(
                        "Slurm target '{}' requires a SIF image manifest",
                        target.id
                    ));
                }
                ComputeBackend::Ssh { .. } if profile.is_none() => {
                    return Err(format!(
                        "SSH target '{}' requires a typed-worker Podman profile",
                        target.id
                    ));
                }
                _ => {}
            }
            if let Some(profile) = &settings.typed_worker {
                let bytes = serde_json::to_vec(profile).map_err(|error| error.to_string())?;
                if target.capsule_sha256.get("worker.json")
                    != Some(&format!("{:x}", Sha256::digest(bytes)))
                {
                    return Err("target worker profile capsule identity changed".into());
                }
            }
            if let Some(path) = &settings.apptainer_manifest {
                let actual = format!("{:x}", Sha256::digest(read_private(path)?));
                if target.capsule_sha256.get("images.json") != Some(&actual) {
                    return Err("target profile capsule identity changed".into());
                }
            }
        }
        // Remote placement's local fallback keeps its existing admission
        // behavior while sharing the configured receipt root.
        let local_fallback: Arc<dyn TaskExecutor> =
            Arc::new(dag_core::LocalTaskExecutor::with_workspace_root(
                dag_core::dag_tasks_root_for_state_dir(&self.config.state_dir),
            ));
        let (executor, containers, hpc, ssh_probe): (
            Arc<dyn TaskExecutor>,
            Arc<dyn container_runtime::PodmanConnection>,
            _,
            Option<dag_core::SshTarget>,
        ) = match &target.backend {
            ComputeBackend::Local => (self.local_executor.clone(), self.local.clone(), None, None),
            ComputeBackend::Slurm { settings } => {
                let mut config = self.config.clone();
                config.slurm = settings.clone();
                config.slurm.enabled = true;
                if (target.id != "legacy-slurm")
                    && config.slurm.artifact_root.as_os_str().is_empty()
                {
                    config.slurm.artifact_root =
                        self.root.join("targets").join(&target_id).join("artifacts");
                }
                let mut slurm = crate::host::shared_infra::slurm_settings(&config)
                    .map_err(|e| e.to_string())?;
                if let Some(endpoint) = target.endpoints.get(&settings.ssh_host) {
                    slurm.ssh = slurm.ssh.with_endpoint(endpoint.clone());
                }
                slurm.ssh = slurm.ssh.with_shared_endpoints(target.endpoints.clone());
                let workspace = if target.id == "legacy-slurm" {
                    config.state_dir.join("slurm-tasks")
                } else {
                    self.root.join("targets").join(&target_id).join("tasks")
                };
                // Receipts contain host-absolute paths. Updated targets must
                // expose their evidence to existing VFS clients as well.
                // Never mount the enclosing worker/credential configuration.
                self.mount_execution_evidence(&workspace)?;
                self.mount_execution_evidence(&slurm.local_artifact_root)?;
                let executor: Arc<dyn TaskExecutor> = if let Some(profile) = profile {
                    let (executor, revision) = crate::host::hpc_placement::executor(
                        profile,
                        target.data_domains.first().map(DataDomain::data_home),
                        slurm.clone(),
                        &workspace,
                        &self.storage,
                        local_fallback.clone(),
                    )
                    .map_err(|e| e.to_string())?;
                    typed_worker_revision = Some(revision);
                    executor
                } else {
                    slurm_backed_executor(slurm.clone(), workspace, local_fallback.clone())
                };
                let mut container_settings = slurm;
                if target
                    .data_domains
                    .iter()
                    .any(DataDomain::has_separate_task_mount)
                {
                    let root = Path::new(&settings.scratch_remote_dir)
                        .parent()
                        .ok_or("unified storage root is missing")?;
                    if container_settings
                        .apptainer
                        .as_ref()
                        .is_some_and(|manifest| {
                            manifest
                                .images
                                .values()
                                .any(|image| !image.path.starts_with(root.join("images/sif")))
                        })
                    {
                        return Err(
                            "SIF images must be under the unified images/sif directory".into()
                        );
                    }
                    container_settings.scratch_dir = root.join("work").display().to_string();
                }
                let runtime = plugin::slurm_runtime::SlurmContainerRuntime::new(
                    container_settings,
                    self.local.workspace_root().to_path_buf(),
                    if target.id == "legacy-slurm" {
                        config.state_dir.join("slurm-containers")
                    } else {
                        self.root
                            .join("targets")
                            .join(&target_id)
                            .join("containers")
                    },
                    settings.panel_cache_remote_dir.clone(),
                    settings.remote_python.clone(),
                )
                .and_then(|r| match &settings.storage_root_v1 {
                    Some(root) => r.with_remote_runtime_root(format!("{root}/runtime/apptainer")),
                    None => Ok(r),
                })
                .and_then(|r| r.with_gpu_partition(settings.gpu_partition.clone()))
                .and_then(|r| r.with_gpu_cpus_per_gpu(settings.gpu_cpus_per_gpu))
                .and_then(|r| match &settings.gpu_qos {
                    Some(q) => r.with_gpu_qos(q.clone()),
                    None => Ok(r),
                })
                .map_err(|e| e.to_string())?;
                let hpc = self.slurm_account_client(&target, settings)?;
                (
                    executor,
                    Arc::new(runtime) as Arc<dyn container_runtime::PodmanConnection>,
                    Some(hpc),
                    None,
                )
            }
            ComputeBackend::Ssh { settings } => {
                let artifact_root = if settings.artifact_root.as_os_str().is_empty() {
                    self.root.join("targets").join(&target_id).join("artifacts")
                } else {
                    settings.artifact_root.clone()
                };
                // The blocking exit-code wait holds one connection for
                // the task's whole horizon: it needs keepalive.
                let mut ssh = dag_core::SshTarget::new(
                    settings.ssh_command.clone(),
                    settings.ssh_host.clone(),
                )
                .with_connect_timeout(std::time::Duration::from_secs(
                    settings.ssh_connect_timeout_secs,
                ))
                .with_keepalive(std::time::Duration::from_secs(15), 2);
                if let Some(endpoint) = target.endpoints.get(&settings.ssh_host) {
                    ssh = ssh.with_endpoint(endpoint.clone());
                }
                ssh = ssh.with_shared_endpoints(target.endpoints.clone());
                let workspace = self.root.join("targets").join(&target_id).join("tasks");
                self.mount_execution_evidence(&workspace)?;
                self.mount_execution_evidence(&artifact_root)?;
                let default_time = crate::config::slurm::parse_walltime(&settings.default_time)
                    .ok_or_else(|| {
                        format!(
                            "invalid default_time `{}` on direct SSH target {}",
                            settings.default_time, target.id
                        )
                    })?;
                let ssh_probe = ssh.clone();
                let mut store = dag_core::SshArtifactStore::new(
                    ssh.clone(),
                    settings.scratch_remote_dir.clone(),
                    artifact_root,
                );
                if let Some(domain) = target.data_domains.first() {
                    let home = domain.data_home();
                    home.validate_vfs(&self.storage)
                        .map_err(|e| e.to_string())?;
                    store = store.with_data_home(home).map_err(|e| e.to_string())?;
                }
                let remote: Arc<dyn TaskExecutor> = Arc::new(RemoteTaskExecutor::new(
                    Arc::new(dag_core::SshTaskTransport::new(dag_core::SshSettings {
                        ssh: ssh.clone(),
                        scratch_dir: settings.scratch_remote_dir.clone(),
                        default_time,
                    })),
                    Arc::new(store),
                    workspace.clone(),
                ));
                let profile = profile.ok_or("SSH targets require a typed-worker Podman profile")?;
                let (executor, revision) = crate::host::hpc_placement::assemble_ssh_typed(
                    profile,
                    remote,
                    local_fallback.clone(),
                )
                .map_err(|error| format!("SSH target '{}': {error}", target.id))?;
                typed_worker_revision = Some(revision);
                // Podman runs inside the typed worker. Direct SSH also
                // provides the account file/transfer tools without a scheduler.
                let account_state_dir = self
                    .root
                    .join("targets")
                    .join(&target_id)
                    .join("hpc-account");
                private_dir(&account_state_dir)?;
                let account = Arc::new(
                    hpc_tools::HpcClient::new(hpc_tools::HpcConfig {
                        ssh_host: settings.ssh_host.clone(),
                        data_ssh_host: settings.data_ssh_host.clone(),
                        pi_ssh_host: None,
                        ssh_command: settings.ssh_command.clone(),
                        remote_python: settings.remote_python.clone(),
                        state_dir: account_state_dir,
                        default_partition: String::new(),
                        default_account: None,
                    })?
                    .with_endpoints(target.endpoints.clone())?,
                );
                (
                    executor,
                    Arc::new(routing::WorkerContainers {
                        root: self.local.workspace_root().to_path_buf(),
                    }),
                    Some(account),
                    Some(ssh_probe),
                )
            }
        };
        Ok(Arc::new(BuiltTarget {
            typed_worker_revision,
            network_policy_sha256,
            target,
            executor,
            containers,
            hpc,
            ssh_probe,
        }))
    }
    pub(crate) fn bind(&self) -> Result<(Arc<BuiltTarget>, ExecutionBinding), String> {
        let (target, _) = self.selected()?;
        let binding = self.record_binding(&target)?;
        Ok((target, binding))
    }
    pub(crate) fn record_binding(
        &self,
        target: &Arc<BuiltTarget>,
    ) -> Result<ExecutionBinding, String> {
        let binding = ExecutionBinding {
            id: uuid::Uuid::new_v4().to_string(),
            target_id: target.target.id.clone(),
            target_sha256: format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&target.target).map_err(|e| e.to_string())?)
            ),
        };
        write_binding(
            &self.root.join("runs").join(format!("{}.json", binding.id)),
            &binding,
        )?;
        Ok(binding)
    }
}
