//! Operator-owned compute selection. Storage namespaces have an independent lifetime.
mod readiness;
mod routing;
pub use readiness::ReadinessRequest;
#[cfg(test)]
mod tests;
pub mod tools;

use crate::{RuntimeConfig, config::SlurmConfig};
use container_runtime::{ContainerExecutionInfra, PodmanConnection};
use dag_core::{
    ProcessRoutingTaskExecutor, RemoteTaskExecutor, TaskExecutor,
    dag::execution::local_task_executor, dag::execution::slurm_backed_executor,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
};

pub use routing::ComputeExecutor;
pub(crate) use routing::ContainerRouter;
#[cfg(test)]
pub(crate) use routing::ImageBuilder as ComputeImageBuilder;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ComputeBackend {
    Local,
    Slurm {
        settings: SlurmConfig,
    },
    /// A scheduler-free remote host: tasks run directly over ssh with no
    /// queue, no priority and no enforced resource ceiling. The settings
    /// reuses the `SlurmConfig` carrier for its ssh/scratch/artifact keys;
    /// the scheduler-only keys (partition, account, qos, gpu_*, mem_per_cpu)
    /// are ignored here and the editors do not offer them.
    Ssh {
        settings: SlurmConfig,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComputeTarget {
    pub id: String,
    pub revision: u64,
    pub name: String,
    pub enabled: bool,
    pub backend: ComputeBackend,
    #[serde(default)]
    pub capsule_sha256: BTreeMap<String, String>,
    #[serde(default)]
    pub endpoints: BTreeMap<String, vfs::SshEndpoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DataBackend {
    Local {
        root: String,
    },
    Ssh {
        alias: String,
        root: String,
        python: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataSource {
    pub id: String,
    pub name: String,
    pub backend: DataBackend,
    #[serde(default = "yes")]
    pub read_only: bool,
    #[serde(default)]
    pub endpoint: Option<vfs::SshEndpoint>,
    #[serde(default)]
    pub permissions: vfs::permission::MountPermissions,
}
fn yes() -> bool {
    true
}

/// Resolve durable placement before opening any legacy backend. The
/// operator's site profile seeds the base config (file > env > default);
/// an enrolled compute catalog then overrides placement for enrolled
/// deployments. Environment variables seed the first catalog; they do not
/// retarget an enrolled revision.
pub(crate) fn startup_config(config: &RuntimeConfig) -> Result<RuntimeConfig, String> {
    let mut config = config.clone();
    if let Some(profile) = crate::config::slurm::SlurmSiteProfile::load(&config.state_dir)? {
        profile.apply_to(&mut config.slurm);
    }
    let config = &config;
    let path = config.state_dir.join("compute/catalog.json");
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(config.clone()),
        Err(error) => return Err(error.to_string()),
        Ok(_) => (),
    }
    let catalog: ComputeCatalog =
        serde_json::from_slice(&read_private(&path)?).map_err(|e| e.to_string())?;
    if catalog.version != 1 {
        return Err("unsupported compute catalog".into());
    }
    let target = catalog
        .targets
        .iter()
        .find(|t| t.id == catalog.policy.target_id && t.revision == catalog.policy.target_revision)
        .ok_or("selected target revision is missing")?;
    let mut resolved = config.clone();
    match &target.backend {
        ComputeBackend::Local => {
            resolved.slurm.enabled = false;
            resolved.slurm.typed_worker_config = None;
            resolved.slurm.apptainer_manifest = None;
        }
        ComputeBackend::Slurm { settings } => {
            resolved.slurm = settings.clone();
            resolved.slurm.enabled = true;
            resolved.plugin_rsi.environment_dev.enabled = false;
        }
        ComputeBackend::Ssh { settings } => {
            // A direct host has no scheduler keys to honor; only the ssh,
            // scratch, artifact and worker-pinning keys are meaningful.
            resolved.slurm = settings.clone();
            resolved.slurm.enabled = true;
            resolved.plugin_rsi.environment_dev.enabled = false;
        }
    }
    Ok(resolved)
}

impl DataSource {
    pub fn mount_path(&self) -> String {
        format!("/data/{}", self.id)
    }
    fn manifest(&self) -> Result<vfs::VfsManifest, String> {
        identifier(&self.id)?;
        if self.name.trim().is_empty() {
            return Err("data source name is required".into());
        }
        let config = match &self.backend {
            DataBackend::Local { root } => {
                if !Path::new(root).is_absolute() || root == "/" {
                    return Err("data root must be an absolute non-root directory".into());
                }
                if !Path::new(root).is_dir() {
                    return Err("local data root must already be a directory".into());
                }
                vfs::BackendConfig::local(root)
            }
            DataBackend::Ssh {
                alias,
                root,
                python,
            } => {
                if !self.read_only {
                    return Err("SSH data sources must be read-only".into());
                }
                vfs::BackendConfig::Ssh {
                    alias: alias.clone(),
                    root: root.clone(),
                    python: python.clone(),
                    endpoint: self.endpoint.clone(),
                }
            }
        };
        Ok(vfs::VfsManifest {
            backend: vec![vfs::BackendDefinition {
                id: format!("data-{}", self.id),
                config,
            }],
            mount: vec![vfs::MountDefinition {
                path: self.mount_path(),
                backend: format!("data-{}", self.id),
                source: "/".into(),
                read_only: self.read_only,
                permissions: self.permissions.clone(),
            }],
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComputePolicy {
    pub revision: u64,
    pub target_id: String,
    pub target_revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComputeCatalog {
    pub version: u32,
    pub revision: u64,
    pub policy: ComputePolicy,
    pub targets: Vec<ComputeTarget>,
    pub data_sources: Vec<DataSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputeStatus {
    #[serde(default)]
    pub deployment: serde_json::Value,
    pub catalog: ComputeCatalog,
    pub mounts: Vec<vfs::MountDefinition>,
    pub runs: Vec<ExecutionBinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionBinding {
    pub id: String,
    pub target_id: String,
    pub target_revision: u64,
    pub policy_revision: u64,
    pub target_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputeImageEvidence {
    pub target_id: String,
    pub target_revision: u64,
    pub target_sha256: String,
    pub capsule_sha256: BTreeMap<String, String>,
    pub resolution: container_runtime::ContainerImageResolution,
    pub evidence_scope: String,
    pub sif_bytes_verified: bool,
    pub node_execution_performed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveTarget {
    pub expected_revision: u64,
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub backend: ComputeBackend,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectTarget {
    pub expected_revision: u64,
    pub target_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveDataSource {
    pub expected_revision: u64,
    pub source: DataSource,
}

pub(crate) struct BuiltTarget {
    typed_worker_revision: Option<String>,
    network_policy_sha256: Option<String>,
    pub target: ComputeTarget,
    pub executor: Arc<dyn TaskExecutor>,
    pub containers: Arc<dyn PodmanConnection>,
    pub hpc: Option<Arc<hpc_tools::HpcClient>>,
    /// The direct-host ssh identity, so a configuration check can observe
    /// the connection a run would actually use. Absent on other backends.
    pub ssh_probe: Option<dag_core::SshTarget>,
    pub monitor_key: String,
}

impl BuiltTarget {
    fn deployment(&self) -> serde_json::Value {
        let source_compatible = self
            .typed_worker_revision
            .as_deref()
            .is_none_or(|revision| revision == dag_core::source_revision());
        serde_json::json!({
            "target_id": self.target.id,
            "target_revision": self.target.revision,
            "coordinator_source_revision": dag_core::source_revision(),
            "worker_source_revision": self.typed_worker_revision,
            "source_compatible": source_compatible,
            "scope": "COORDINATOR_WORKER_SOURCE_COMPATIBILITY_ONLY",
            "workflow_readiness": "NOT_CHECKED_USE_COMPUTE_READINESS",
            "worker_and_image_execution_verified": false,
            "error": if source_compatible { None } else { Some("typed_worker_source_mismatch") },
        })
    }

    fn ensure_source_compatible(&self) -> Result<(), String> {
        if let Some(revision) = &self.typed_worker_revision
            && revision != dag_core::source_revision()
        {
            return Err(format!(
                "compute target {} revision {} has typed-worker source {}; coordinator source is {}. Deploy the measured matching worker and enroll a new target revision before starting a run; preserve historical pins",
                self.target.id,
                self.target.revision,
                revision,
                dag_core::source_revision(),
            ));
        }
        Ok(())
    }
}

pub struct ComputeRegistry {
    root: PathBuf,
    config: RuntimeConfig,
    storage: Arc<vfs::OpendalFileStorage>,
    vfs: Arc<vfs::MountedObjectStore>,
    local: Arc<dyn PodmanConnection>,
    local_executor: Arc<dyn TaskExecutor>,
    state: RwLock<ComputeCatalog>,
    built: RwLock<BTreeMap<String, Arc<BuiltTarget>>>,
    mutation: Mutex<()>,
    poisoned: std::sync::atomic::AtomicBool,
    pub monitors: Arc<crate::hpc_monitor::MonitorHub>,
}

pub(crate) fn identifier(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(
            "identifier must contain 1-64 ASCII letters, digits, underscores or dashes".into(),
        );
    }
    Ok(())
}
fn key(id: &str, revision: u64) -> String {
    format!("{id}-r{revision}")
}

pub(crate) fn private_dir(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt;
    let created = match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(e) => return Err(e.to_string()),
    };
    let m = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o7777 != 0o700 {
        return Err("compute state directory must be owned, private and nonsymlink".into());
    }
    if created {
        fs::File::open(path.parent().ok_or("compute directory requires a parent")?)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn read_private(path: &Path) -> Result<Vec<u8>, String> {
    read_owned(path, true)
}

fn read_owned(path: &Path, private: bool) -> Result<Vec<u8>, String> {
    let mut f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| e.to_string())?;
    let m = f.metadata().map_err(|e| e.to_string())?;
    if !m.is_file()
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & if private { 0o077 } else { 0o022 } != 0
        || m.nlink() != 1
        || m.len() > 4 * 1024 * 1024
    {
        return Err("compute configuration must be a bounded private regular file".into());
    }
    let mut bytes = Vec::new();
    (&mut f)
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let after = f.metadata().map_err(|e| e.to_string())?;
    let named = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    let identity = |v: &fs::Metadata| {
        (
            v.dev(),
            v.ino(),
            v.mode(),
            v.uid(),
            v.gid(),
            v.nlink(),
            v.len(),
            v.mtime(),
            v.mtime_nsec(),
            v.ctime(),
            v.ctime_nsec(),
        )
    };
    if identity(&m) != identity(&after)
        || identity(&after) != identity(&named)
        || bytes.len() > 4 * 1024 * 1024
    {
        return Err("compute configuration changed while reading".into());
    }
    Ok(bytes)
}

pub(crate) fn write_new(path: &Path, value: &impl Serialize) -> Result<(), String> {
    write_bytes_new(
        path,
        &serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?,
    )
}
fn write_bytes_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| e.to_string())?;
    f.write_all(bytes)
        .and_then(|_| f.sync_all())
        .map_err(|e| e.to_string())?;
    fs::File::open(path.parent().unwrap())
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}
/// Atomically publish a catalog so readers never observe a partial file.
fn write_catalog(root: &Path, catalog: &ComputeCatalog) -> Result<(), String> {
    let pending = root.join(format!(".pending-{}", uuid::Uuid::new_v4()));
    write_new(&pending, catalog)?;
    fs::rename(&pending, root.join("catalog.json")).map_err(|e| e.to_string())?;
    fs::File::open(root)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}
/// Publish a complete binding without replacing historical evidence. Pending
/// bytes survive a failed write/publication, but are never read as run records.
fn write_binding(path: &Path, binding: &ExecutionBinding) -> Result<(), String> {
    use std::os::unix::ffi::OsStrExt;
    let parent = path.parent().ok_or("binding path requires a parent")?;
    let pending = parent.join(format!(".pending-{}", uuid::Uuid::new_v4()));
    write_new(&pending, binding)?;
    let from = std::ffi::CString::new(pending.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let to = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    // SAFETY: both paths are live NUL-terminated strings. Linux's no-replace
    // rename atomically exposes the synced file and preserves an existing ID.
    if unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    fs::File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}

impl ComputeRegistry {
    /// Inspect the same in-memory runtime used by runs bound to this revision.
    /// Never reread a mutable inventory or expose private profile contents.
    pub fn image_evidence(
        &self,
        id: &str,
        revision: u64,
        reference: &str,
    ) -> Result<ComputeImageEvidence, String> {
        let target = self.target(id, Some(revision))?;
        let resolution = target
            .containers
            .image_resolution(reference)
            .map_err(|e| e.to_string())?;
        Ok(ComputeImageEvidence {
            target_id: target.target.id.clone(),
            target_revision: target.target.revision,
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
        containers: &ContainerExecutionInfra,
        monitors: Arc<crate::hpc_monitor::MonitorHub>,
    ) -> Result<Arc<Self>, String> {
        let root = config.state_dir.join("compute");
        private_dir(&root)?;
        private_dir(&root.join("runs"))?;
        private_dir(&root.join("targets"))?;
        let catalog_path = root.join("catalog.json");
        let is_new = match fs::symlink_metadata(&catalog_path) {
            Ok(_) => false,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
            Err(error) => return Err(error.to_string()),
        };
        let mut catalog: ComputeCatalog = if !is_new {
            serde_json::from_slice(&read_private(&catalog_path)?).map_err(|e| e.to_string())?
        } else {
            let mut targets = vec![ComputeTarget {
                id: "local".into(),
                revision: 1,
                name: "Local".into(),
                enabled: true,
                backend: ComputeBackend::Local,
                capsule_sha256: BTreeMap::new(),
                endpoints: BTreeMap::new(),
            }];
            if config.slurm.enabled || config.slurm.tools_enabled {
                targets.push(ComputeTarget {
                    id: "legacy-slurm".into(),
                    revision: 1,
                    name: "SJTU".into(),
                    enabled: config.slurm.enabled,
                    backend: ComputeBackend::Slurm {
                        settings: config.slurm.clone(),
                    },
                    capsule_sha256: BTreeMap::new(),
                    endpoints: BTreeMap::new(),
                });
            }
            let target_id = if config.slurm.enabled {
                "legacy-slurm"
            } else {
                "local"
            };
            ComputeCatalog {
                version: 1,
                revision: 1,
                policy: ComputePolicy {
                    revision: 1,
                    target_id: target_id.into(),
                    target_revision: 1,
                },
                targets,
                data_sources: vec![],
            }
        };
        if catalog.version != 1 || catalog.revision == 0 {
            return Err("unsupported compute catalog".into());
        }
        // Names are part of saved target revisions. Presentation changes must
        // not rewrite historical capsules or invalidate a prepared deployment
        // catalog merely by opening the registry.
        // A single gateway already owns state_dir; mutations additionally serialize here.
        let local = Arc::new(container_runtime::PodmanRuntime::new(
            containers.config.clone(),
        ));
        let registry = Arc::new(Self {
            root,
            config: config.clone(),
            storage,
            vfs,
            local,
            local_executor: Arc::new(
                dag_core::dag::execution::LimitedTaskExecutor::new(
                    local_task_executor(),
                    std::thread::available_parallelism()
                        .map(|n| n.get().saturating_sub(2).max(1).min(1024))
                        .unwrap_or(1),
                )
                .map_err(|e| e.to_string())?,
            ),
            state: RwLock::new(catalog.clone()),
            built: RwLock::new(BTreeMap::new()),
            mutation: Mutex::new(()),
            poisoned: std::sync::atomic::AtomicBool::new(false),
            monitors,
        });
        for source in &catalog.data_sources {
            registry
                .vfs
                .add_data_mount(&source.manifest()?)
                .map_err(|e| e.to_string())?;
        }
        for target in &mut catalog.targets {
            if is_new {
                registry.freeze_profile(target)?;
            }
            identifier(&target.id)?;
            if target.revision == 0 || target.name.trim().is_empty() {
                return Err("invalid compute target".into());
            }
            let built = registry.build(target.clone())?;
            if registry
                .built
                .write()
                .unwrap()
                .insert(built.monitor_key.clone(), built)
                .is_some()
            {
                return Err("duplicate compute target revision".into());
            }
        }
        *registry.state.write().unwrap() = catalog.clone();
        registry.target(
            &catalog.policy.target_id,
            Some(catalog.policy.target_revision),
        )?;
        if is_new {
            registry.persist(&catalog)?;
        }
        Ok(registry)
    }

    fn healthy(&self) -> Result<(), String> {
        if self.poisoned.load(std::sync::atomic::Ordering::Acquire) {
            return Err("compute persistence outcome uncertain; restart and reconcile before admitting work".into());
        }
        Ok(())
    }
    fn persist(&self, catalog: &ComputeCatalog) -> Result<(), String> {
        let result = write_catalog(&self.root, catalog);
        if result.is_err() {
            // A storage failure must not let new work outlive a damaged or
            // non-durable catalog. Already-bound work retains its own backend.
            self.poisoned
                .store(true, std::sync::atomic::Ordering::Release);
        }
        result
    }
    pub fn status(&self) -> Result<ComputeStatus, String> {
        self.healthy()?;
        let mut runs = Vec::new();
        for entry in fs::read_dir(self.root.join("runs")).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.extension().is_some_and(|e| e == "json") {
                runs.push(
                    serde_json::from_slice::<ExecutionBinding>(&read_private(&path)?)
                        .map_err(|e| e.to_string())?,
                );
            }
        }
        runs.sort_by(|a, b| a.id.cmp(&b.id));
        let catalog = self.catalog();
        let deployment = self
            .target(
                &catalog.policy.target_id,
                Some(catalog.policy.target_revision),
            )?
            .deployment();
        Ok(ComputeStatus {
            deployment,
            catalog,
            mounts: self.vfs.mount_definitions(),
            runs,
        })
    }
    pub fn catalog(&self) -> ComputeCatalog {
        self.state.read().unwrap().clone()
    }

    /// Read-only connection inspection. Never allocates a compute job.
    pub async fn check_target(&self, id: &str) -> Result<serde_json::Value, String> {
        let target = self.target(id, None)?;
        let deployment = target.deployment();
        if let Err(error) = target.ensure_source_compatible() {
            return Ok(
                serde_json::json!({"target_id": id, "target_revision": target.target.revision,
                "configuration_valid": false, "deployment": deployment, "error": error,
                "connection_observation": null, "worker_and_image_execution_verified": false,
                "new_job_submitted": false, "scientific_admission_granted": false}),
            );
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
            serde_json::json!({"target_id":target.target.id,"target_revision":target.target.revision,"configuration_valid":true,"deployment":deployment,"connection_observation":observation,"worker_and_image_execution_verified":false,"new_job_submitted":false,"scientific_admission_granted":false}),
        )
    }

    pub(crate) fn selected(&self) -> Result<(Arc<BuiltTarget>, ComputePolicy), String> {
        self.healthy()?;
        let state = self.state.read().unwrap();
        let target = self
            .built
            .read()
            .unwrap()
            .get(&key(&state.policy.target_id, state.policy.target_revision))
            .cloned()
            .ok_or("selected target revision is missing")?;
        // Admission checks current enabled state, while existing run bindings remain valid.
        let latest = state
            .targets
            .iter()
            .rev()
            .find(|t| t.id == target.target.id)
            .ok_or("target missing")?;
        if !latest.enabled {
            return Err("selected compute target is disabled; select another target".into());
        }
        target.ensure_source_compatible()?;
        Ok((target, state.policy.clone()))
    }
    pub(crate) fn target(
        &self,
        id: &str,
        revision: Option<u64>,
    ) -> Result<Arc<BuiltTarget>, String> {
        self.healthy()?;
        let state = self.state.read().unwrap();
        let t = state
            .targets
            .iter()
            .rev()
            .find(|t| t.id == id && revision.is_none_or(|r| r == t.revision))
            .ok_or("unknown target revision")?;
        self.built
            .read()
            .unwrap()
            .get(&key(id, t.revision))
            .cloned()
            .ok_or("target unavailable".into())
    }
    pub fn select(&self, request: SelectTarget) -> Result<ComputeCatalog, String> {
        let _guard = self.mutation.lock().map_err(|_| "compute lock poisoned")?;
        self.healthy()?;
        let mut next = self.catalog();
        if next.revision != request.expected_revision {
            return Err("stale compute revision; reload settings".into());
        }
        let t = next
            .targets
            .iter()
            .rev()
            .find(|t| t.id == request.target_id && t.enabled)
            .ok_or("target is missing or disabled")?;
        if next
            .targets
            .iter()
            .rev()
            .find(|t| t.id == request.target_id)
            .is_some_and(|t| !t.enabled)
        {
            return Err("target is disabled".into());
        }
        self.target(&t.id, Some(t.revision))?
            .ensure_source_compatible()?;
        next.policy = ComputePolicy {
            revision: next
                .policy
                .revision
                .checked_add(1)
                .ok_or("policy revision overflow")?,
            target_id: t.id.clone(),
            target_revision: t.revision,
        };
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or("catalog revision overflow")?;
        self.persist(&next)?;
        *self.state.write().unwrap() = next.clone();
        Ok(next)
    }
    pub fn save_target(&self, request: SaveTarget) -> Result<ComputeCatalog, String> {
        let _guard = self.mutation.lock().map_err(|_| "compute lock poisoned")?;
        self.healthy()?;
        identifier(&request.id)?;
        if request.name.trim().is_empty() {
            return Err("target name is required".into());
        }
        let mut next = self.catalog();
        if next.revision != request.expected_revision {
            return Err("stale compute revision; reload settings".into());
        }
        let revision = next
            .targets
            .iter()
            .filter(|t| t.id == request.id)
            .map(|t| t.revision)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or("target revision overflow")?;
        let mut target = ComputeTarget {
            id: request.id,
            revision,
            name: request.name,
            enabled: request.enabled,
            backend: request.backend,
            capsule_sha256: BTreeMap::new(),
            endpoints: BTreeMap::new(),
        };
        self.freeze_profile(&mut target)?;
        let built = self.build(target.clone())?;
        // Publish the built revision first; it is unreachable by policy until the durable commit.
        self.built
            .write()
            .unwrap()
            .insert(built.monitor_key.clone(), built);
        next.targets.push(target);
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or("catalog revision overflow")?;
        self.persist(&next)?;
        *self.state.write().unwrap() = next.clone();
        Ok(next)
    }
    pub async fn add_source(&self, mut request: SaveDataSource) -> Result<ComputeCatalog, String> {
        self.healthy()?;
        identifier(&request.source.id)?;
        if self.catalog().revision != request.expected_revision {
            return Err("stale compute revision; reload settings".into());
        }
        // Probe before taking the synchronous mutation lock. Recheck the
        // revision and mount conflicts under that lock before publication.
        self.storage
            .require_vacant_mount_path(&request.source.mount_path())
            .await
            .map_err(|e| e.to_string())?;
        let _guard = self.mutation.lock().map_err(|_| "compute lock poisoned")?;
        self.healthy()?;
        let mut next = self.catalog();
        if next.revision != request.expected_revision {
            return Err("stale compute revision; reload settings".into());
        }
        if next.data_sources.iter().any(|s| s.id == request.source.id) {
            return Err("data source identity already exists; use a new namespace".into());
        }
        if let DataBackend::Ssh { alias, .. } = &request.source.backend {
            request.source.endpoint = Some(vfs::SshEndpoint::capture("ssh", alias)?);
        }
        let manifest = request.source.manifest()?;
        vfs::MountedObjectStore::from_manifest(&manifest).map_err(|e| e.to_string())?;
        // Collision check before persistence; no existing namespace can be rebound.
        let path = request.source.mount_path();
        if self.vfs.mount_paths().iter().any(|p| {
            p == &path
                || p.starts_with(&format!("{path}/"))
                || (p != "/" && path.starts_with(&format!("{p}/")))
        }) {
            return Err("data source shadows an existing mount".into());
        }
        next.data_sources.push(request.source);
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or("catalog revision overflow")?;
        self.persist(&next)?;
        if let Err(e) = self.vfs.add_data_mount(&manifest) {
            self.poisoned
                .store(true, std::sync::atomic::Ordering::Release);
            return Err(e.to_string());
        }
        *self.state.write().unwrap() = next.clone();
        Ok(next)
    }

    fn freeze_profile(&self, target: &mut ComputeTarget) -> Result<(), String> {
        let settings = match &mut target.backend {
            ComputeBackend::Slurm { settings } | ComputeBackend::Ssh { settings } => settings,
            ComputeBackend::Local => return Ok(()),
        };
        let dir = self
            .root
            .join("targets")
            .join(key(&target.id, target.revision));
        private_dir(&dir)?;
        let dir = dir.join(format!("profile-{}", uuid::Uuid::new_v4()));
        private_dir(&dir)?;
        // Legacy installation starts offline and keeps historical bindings unchanged.
        // New operator revisions resolve and freeze their non-secret endpoints.
        for alias in std::iter::once(&settings.ssh_host)
            .chain(settings.data_ssh_host.iter())
            .chain(settings.pi_ssh_host.iter())
            .filter(|_| !(target.id == "legacy-slurm" && target.revision == 1))
        {
            target.endpoints.insert(
                alias.clone(),
                vfs::SshEndpoint::capture(&settings.ssh_command, alias)?,
            );
        }
        for (name, path) in [
            ("worker.json", &mut settings.typed_worker_config),
            ("images.json", &mut settings.apptainer_manifest),
        ] {
            if let Some(source) = path {
                let bytes = read_owned(source, false)?;
                let _: serde_json::Value =
                    serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                // Credentials are references, never copied credential-file contents.
                let frozen = dir.join(name);
                write_bytes_new(&frozen, &bytes)?;
                target.capsule_sha256.insert(
                    name.into(),
                    format!("{:x}", Sha256::digest(read_private(&frozen)?)),
                );
                *source = frozen;
            }
        }
        Ok(())
    }

    fn mount_execution_evidence(&self, root: &Path) -> Result<(), String> {
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

    fn build(&self, target: ComputeTarget) -> Result<Arc<BuiltTarget>, String> {
        let monitor_key = key(&target.id, target.revision);
        let network_policy_sha256 = match &target.backend {
            ComputeBackend::Slurm { settings } | ComputeBackend::Ssh { settings } => settings
                .typed_worker_config
                .as_ref()
                .map(|path| {
                    crate::host::hpc_placement::pinned_network_policy(path)
                        .map_err(|error| error.to_string())
                })
                .transpose()?
                .flatten(),
            ComputeBackend::Local => None,
        };
        let mut typed_worker_revision = match &target.backend {
            ComputeBackend::Slurm { settings } | ComputeBackend::Ssh { settings } => settings
                .typed_worker_config
                .as_ref()
                .map(|path| {
                    crate::host::hpc_placement::pinned_source_revision(path)
                        .map_err(|error| error.to_string())
                })
                .transpose()?,
            ComputeBackend::Local => None,
        };
        if let ComputeBackend::Slurm { settings } | ComputeBackend::Ssh { settings } =
            &target.backend
        {
            if matches!(target.backend, ComputeBackend::Slurm { .. })
                && target.id != "legacy-slurm"
                && settings.typed_worker_config.is_none()
            {
                return Err("global Slurm targets require a pinned typed-worker profile; process-only routing is retained solely for legacy migration".into());
            }
            if matches!(target.backend, ComputeBackend::Ssh { .. })
                && settings.apptainer_manifest.is_some()
            {
                return Err("container workloads are not available on direct SSH targets yet; enroll a Slurm target for container work".into());
            }
            for (name, path) in [
                ("worker.json", &settings.typed_worker_config),
                ("images.json", &settings.apptainer_manifest),
            ] {
                if let Some(path) = path {
                    let actual = format!("{:x}", Sha256::digest(read_private(path)?));
                    if target.capsule_sha256.get(name) != Some(&actual) {
                        return Err("target profile capsule identity changed".into());
                    }
                }
            }
        }
        let (executor, containers, hpc, ssh_probe): (
            Arc<dyn TaskExecutor>,
            Arc<dyn PodmanConnection>,
            _,
            Option<dag_core::SshTarget>,
        ) = match &target.backend {
            ComputeBackend::Local => (self.local_executor.clone(), self.local.clone(), None, None),
            ComputeBackend::Slurm { settings } => {
                let mut config = self.config.clone();
                config.slurm = settings.clone();
                config.slurm.enabled = true;
                if !(target.id == "legacy-slurm" && target.revision == 1)
                    && config.slurm.artifact_root.as_os_str().is_empty()
                {
                    config.slurm.artifact_root = self
                        .root
                        .join("targets")
                        .join(&monitor_key)
                        .join("artifacts");
                }
                let mut slurm = crate::host::shared_infra::slurm_settings(&config)
                    .map_err(|e| e.to_string())?;
                if let Some(endpoint) = target.endpoints.get(&settings.ssh_host) {
                    slurm.ssh = slurm.ssh.with_endpoint(endpoint.clone());
                }
                slurm.ssh = slurm.ssh.with_shared_endpoints(target.endpoints.clone());
                let workspace = if target.id == "legacy-slurm" && target.revision == 1 {
                    config.state_dir.join("slurm-tasks")
                } else {
                    self.root.join("targets").join(&monitor_key).join("tasks")
                };
                // Receipts contain host-absolute paths. New revisions must
                // expose their evidence to existing VFS clients as well.
                // Never mount the enclosing worker/credential configuration.
                self.mount_execution_evidence(&workspace)?;
                self.mount_execution_evidence(&slurm.local_artifact_root)?;
                let executor: Arc<dyn TaskExecutor> =
                    if let Some(path) = &settings.typed_worker_config {
                        let (executor, _, revision) = crate::host::hpc_placement::executor(
                            path,
                            slurm.clone(),
                            &workspace,
                            &self.storage,
                            settings.apptainer_manifest.is_some(),
                        )
                        .map_err(|e| e.to_string())?;
                        typed_worker_revision = Some(revision);
                        executor
                    } else {
                        slurm_backed_executor(slurm.clone(), workspace, local_task_executor())
                    };
                let containers: Arc<dyn PodmanConnection> = if settings.apptainer_manifest.is_some()
                {
                    let runtime = container_plugin::slurm_runtime::SlurmContainerRuntime::new(
                        slurm,
                        self.local.workspace_root().to_path_buf(),
                        if target.id == "legacy-slurm" && target.revision == 1 {
                            config.state_dir.join("slurm-containers")
                        } else {
                            self.root
                                .join("targets")
                                .join(&monitor_key)
                                .join("containers")
                        },
                        settings.panel_cache_remote_dir.clone(),
                        settings.remote_python.clone(),
                    )
                    .and_then(|r| r.with_gpu_partition(settings.gpu_partition.clone()))
                    .and_then(|r| r.with_gpu_cpus_per_gpu(settings.gpu_cpus_per_gpu))
                    .and_then(|r| match &settings.gpu_qos {
                        Some(q) => r.with_gpu_qos(q.clone()),
                        None => Ok(r),
                    })
                    .map_err(|e| e.to_string())?;
                    Arc::new(runtime)
                } else if target.id == "legacy-slurm" && target.revision == 1 && {
                    // Preserve the original operator placement, including
                    // explicitly local legacy container adapters. New
                    // Slurm targets never inherit this compatibility route.
                    let mut legacy = config.clone();
                    legacy.plugin_rsi.environment_dev.enabled = false;
                    crate::host::hpc_placement::requires_local_containers(
                        &legacy,
                        "exploratory",
                        false,
                    )
                    .map_err(|e| e.to_string())?
                } {
                    self.local.clone()
                } else {
                    Arc::new(routing::UnavailableContainers {
                        root: self.local.workspace_root().to_path_buf(),
                    })
                };
                let dir = self.root.join("targets").join(&monitor_key);
                private_dir(&dir)?;
                let state_dir = if target.id == "legacy-slurm" && target.revision == 1 {
                    config.state_dir.join("hpc-account")
                } else {
                    dir.join("hpc-account")
                };
                let hpc = Arc::new(
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
                );
                self.monitors.add(
                    &monitor_key,
                    hpc.clone(),
                    std::time::Duration::from_secs(settings.poll_interval_secs),
                )?;
                (executor, containers, Some(hpc), None)
            }
            ComputeBackend::Ssh { settings } => {
                let artifact_root = if settings.artifact_root.as_os_str().is_empty() {
                    self.root
                        .join("targets")
                        .join(&monitor_key)
                        .join("artifacts")
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
                let workspace = self.root.join("targets").join(&monitor_key).join("tasks");
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
                let remote: Arc<dyn TaskExecutor> = Arc::new(RemoteTaskExecutor::new(
                    Arc::new(dag_core::SshTaskTransport::new(dag_core::SshSettings {
                        ssh: ssh.clone(),
                        scratch_dir: settings.scratch_remote_dir.clone(),
                        default_time,
                    })),
                    Arc::new(dag_core::SshArtifactStore::new(
                        ssh,
                        settings.scratch_remote_dir.clone(),
                        artifact_root,
                    )),
                    workspace.clone(),
                ));
                let executor: Arc<dyn TaskExecutor> =
                    if let Some(path) = &settings.typed_worker_config {
                        let (executor, _, revision) =
                            crate::host::hpc_placement::assemble_typed(path, remote, false)
                                .map_err(|e| e.to_string())?;
                        typed_worker_revision = Some(revision);
                        executor
                    } else {
                        Arc::new(ProcessRoutingTaskExecutor::new(
                            local_task_executor(),
                            Some(remote),
                        ))
                    };
                // No scheduler means no job tools and no container
                // runtime, but the account file/transfer tools are plain
                // ssh and stay available: they need an ssh endpoint, not
                // an allocation.
                let account_state_dir = self
                    .root
                    .join("targets")
                    .join(&monitor_key)
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
                    Arc::new(routing::UnavailableContainers {
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
            monitor_key,
        }))
    }
    pub(crate) fn bind(&self) -> Result<(Arc<BuiltTarget>, ExecutionBinding), String> {
        let (target, policy) = self.selected()?;
        let binding = self.record_binding(&target, &policy)?;
        Ok((target, binding))
    }
    pub(crate) fn record_binding(
        &self,
        target: &Arc<BuiltTarget>,
        policy: &ComputePolicy,
    ) -> Result<ExecutionBinding, String> {
        let binding = ExecutionBinding {
            id: uuid::Uuid::new_v4().to_string(),
            target_id: target.target.id.clone(),
            target_revision: target.target.revision,
            policy_revision: policy.revision,
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
