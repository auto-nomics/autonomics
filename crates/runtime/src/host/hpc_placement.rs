//! Operator-owned placement for an ordinary thin coordinator.
//!
//! This is not an admission path for signed research workers. Their existing
//! native launcher and result custody remain separate.

use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use data_engine::dag::execution::{
    LimitedTaskExecutor, MAX_REMOTE_TASKS, PlacementTaskExecutor, RemoteTaskExecutor,
    SlurmSettings, SlurmTaskTransport, SshArtifactStore, SshDataHome, TaskExecutor,
    TypedRemoteTaskExecutor, TypedWorkerSettings, local_task_executor,
};
use serde::Deserialize;

use crate::error::{Error, Result};

const MAX_PROFILE_BYTES: u64 = 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlacementProfile {
    worker: TypedWorkerSettings,
    #[serde(default)]
    local_kinds: Vec<String>,
    #[serde(default)]
    data_home: Option<SshDataHome>,
    #[serde(default)]
    max_remote_tasks: Option<usize>,
}

fn read_profile(path: &Path) -> Result<PlacementProfile> {
    if !path.is_absolute() {
        return Err(Error::Other("typed-worker profile must be absolute".into()));
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| Error::Other(format!("open typed-worker profile: {error}")))?;
    let before = file
        .metadata()
        .map_err(|error| Error::Other(error.to_string()))?;
    // SAFETY: geteuid has no arguments and only reads the current credential.
    let uid = unsafe { libc::geteuid() };
    if !before.is_file()
        || before.uid() != uid
        || before.mode() & 0o022 != 0
        || before.nlink() != 1
        || before.len() > MAX_PROFILE_BYTES
    {
        return Err(Error::Other(
            "typed-worker profile must be a bounded, owned, non-writable regular file".into(),
        ));
    }
    let mut bytes = Vec::new();
    (&file)
        .take(MAX_PROFILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| Error::Other(error.to_string()))?;
    let after = file
        .metadata()
        .map_err(|error| Error::Other(error.to_string()))?;
    let named = std::fs::symlink_metadata(path).map_err(|error| Error::Other(error.to_string()))?;
    let identity = |m: &std::fs::Metadata| {
        (
            m.dev(),
            m.ino(),
            m.mode(),
            m.uid(),
            m.gid(),
            m.nlink(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
        )
    };
    if bytes.len() as u64 > MAX_PROFILE_BYTES
        || identity(&before) != identity(&after)
        || identity(&after) != identity(&named)
    {
        return Err(Error::Other(
            "typed-worker profile changed while reading".into(),
        ));
    }
    let profile: PlacementProfile = serde_json::from_slice(&bytes)
        .map_err(|error| Error::Other(format!("invalid typed-worker profile: {error}")))?;
    if profile
        .max_remote_tasks
        .is_some_and(|limit| !(1..=MAX_REMOTE_TASKS).contains(&limit))
    {
        return Err(Error::Other(format!(
            "max_remote_tasks must be between 1 and {MAX_REMOTE_TASKS}"
        )));
    }
    Ok(profile)
}

/// Read the same protected profile schema used to construct the executor.
/// Callers retain this identity with the resident target, never a mutable seed.
pub(crate) fn pinned_source_revision(path: &Path) -> Result<String> {
    Ok(read_profile(path)?.worker.expected_source_revision)
}

pub(crate) fn pinned_network_policy(path: &Path) -> Result<Option<String>> {
    let worker = read_profile(path)?.worker;
    worker.validate().map_err(|error| Error::Other(error.to_string()))?;
    Ok(worker.native_environment.and_then(|environment| environment.network_policy_sha256))
}

/// Offline deployment policy only. This validates the local operator profile;
/// it does not claim that the remote binary, SIF inventory or cluster is ready.
pub(crate) fn requires_local_containers(
    config: &crate::RuntimeConfig,
    execution_mode: &str,
    research_binding: bool,
) -> Result<bool> {
    // This is the same ordinary Slurm/Apptainer route selected by
    // shared_infra::open_container_execution. Local plugin adapters still
    // dispatch their container bodies remotely; they need no local Podman
    // inventory or garbage collector. Formal bindings and environment
    // development retain their existing local backend requirements.
    let slurm_containers = config.slurm.enabled
        && config.slurm.apptainer_manifest.is_some()
        && matches!(execution_mode.trim(), "" | "exploratory")
        && !research_binding
        && !config.plugin_rsi.environment_dev.enabled;
    let Some(path) = &config.slurm.typed_worker_config else {
        return Ok(!slurm_containers);
    };
    if !config.slurm.enabled {
        return Err(Error::Other(
            "typed-worker placement requires Slurm enabled".into(),
        ));
    }
    // Formal workers and brokers retain their independently governed backend.
    match execution_mode.trim() {
        "formal" => return Ok(true),
        "" | "exploratory" => (),
        _ => {
            return Err(Error::Other(
                "AUTONOMICS_EXECUTION_MODE must be formal or exploratory".into(),
            ));
        }
    }
    if research_binding {
        return Ok(true);
    }
    let profile = read_profile(path)?;
    profile
        .worker
        .validate()
        .map_err(|error| Error::Other(error.to_string()))?;
    if let Some(data_home) = profile.data_home.clone() {
        // This constructor validates mappings without connecting or creating
        // directories. Invalid retained-data policy must not look provisioned.
        SshArtifactStore::new(
            data_engine::dag::execution::SshTarget::new(
                config.slurm.ssh_command.clone(),
                config.slurm.ssh_host.clone(),
            ),
            config.slurm.scratch_remote_dir.clone(),
            config.slurm.artifact_root.clone(),
        )
        .with_data_home(data_home)
        .map_err(|error| Error::Other(error.to_string()))?;
    }
    if profile
        .local_kinds
        .iter()
        .any(|kind| kind.trim().is_empty() || kind == "process")
    {
        return Err(Error::Other(
            "invalid local node kind in typed-worker placement profile".into(),
        ));
    }
    // Do not narrow the operator's existing local execution contract. Unknown
    // or computation-capable local kinds retain the local backend requirement.
    let coordinator_only = profile.local_kinds.iter().all(|kind| {
        matches!(
            kind.as_str(),
            "channel" | "logical_gather" | "dynamic_fanout"
        )
    });
    if coordinator_only && config.plugin_rsi.environment_dev.enabled {
        return Err(Error::Other("remote-only typed placement conflicts with enabled local environment development; set plugin_rsi.environment_dev.enabled=false".into()));
    }
    Ok(!coordinator_only && !slurm_containers)
}

struct CoordinatorContainerRuntime {
    workspace_root: PathBuf,
}

#[async_trait::async_trait]
impl container_runtime::PodmanConnection for CoordinatorContainerRuntime {
    async fn run(
        &self,
        _request: container_runtime::ContainerRunRequest,
    ) -> std::result::Result<
        container_runtime::ContainerRunResult,
        container_runtime::ContainerRuntimeError,
    > {
        Err(container_runtime::ContainerRuntimeError::Invalid("local container execution is disabled by remote-only typed placement; submit computation to the cluster worker".into()))
    }
    fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }
    fn name(&self) -> &'static str {
        "remote-only-coordinator"
    }
}

pub(super) fn disable_local_container_execution(
    infra: &mut container_runtime::ContainerExecutionInfra,
) {
    infra.runtime = Arc::new(CoordinatorContainerRuntime {
        workspace_root: infra.config.workspace_root.clone(),
    });
}

pub(crate) fn executor(
    path: &Path,
    settings: SlurmSettings,
    workspace: &Path,
    storage: &vfs::OpendalFileStorage,
    remote_container_dispatch: bool,
) -> Result<(Arc<dyn TaskExecutor>, Option<usize>, String)> {
    let profile = read_profile(path)?;
    let mut store = SshArtifactStore::new(
        settings.ssh.clone(),
        settings.scratch_dir.clone(),
        settings.local_artifact_root.clone(),
    );
    if let Some(data_home) = profile.data_home {
        data_home
            .validate_vfs(storage)
            .map_err(|error| Error::Other(error.to_string()))?;
        store = store
            .with_data_home(data_home)
            .map_err(|error| Error::Other(error.to_string()))?;
    }
    let remote: Arc<dyn TaskExecutor> = Arc::new(RemoteTaskExecutor::new(
        Arc::new(SlurmTaskTransport::new(settings)),
        Arc::new(store),
        workspace,
    ));
    assemble_typed(path, remote, remote_container_dispatch)
}

/// Wrap one already-built remote executor with the pinned typed-worker
/// placement profile: typed lowering, the concurrency limit and the local
/// kind whitelist. The transport choice is the caller's — Slurm and the
/// direct SSH backend differ only below this seam.
pub(crate) fn assemble_typed(
    path: &Path,
    remote: Arc<dyn TaskExecutor>,
    remote_container_dispatch: bool,
) -> Result<(Arc<dyn TaskExecutor>, Option<usize>, String)> {
    let profile = read_profile(path)?;
    let source_revision = profile.worker.expected_source_revision.clone();
    let typed: Arc<dyn TaskExecutor> = Arc::new(
        TypedRemoteTaskExecutor::new(remote, profile.worker)
            .map_err(|error| Error::Other(error.to_string()))?,
    );
    let remote = match profile.max_remote_tasks {
        Some(limit) => Arc::new(
            LimitedTaskExecutor::new(typed, limit)
                .map_err(|error| Error::Other(error.to_string()))?,
        ) as Arc<dyn TaskExecutor>,
        None => typed,
    };
    let mut placement =
        PlacementTaskExecutor::new(local_task_executor(), Some(remote), profile.local_kinds)
            .map_err(|error| Error::Other(error.to_string()))?;
    if remote_container_dispatch {
        placement = placement.with_remote_container_dispatch();
    }
    Ok((Arc::new(placement), profile.max_remote_tasks, source_revision))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn profile(path: &Path, local_kinds: serde_json::Value) {
        std::fs::write(path,serde_json::to_vec(&serde_json::json!({
            "worker":{"executable":"/cluster/autonomics","executable_sha256":"a".repeat(64),"expected_source_revision":"toy-source"},
            "local_kinds":local_kinds,
        })).unwrap()).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn slurm_container_adapters_do_not_require_a_local_podman_store() {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("profile.json");
        profile(&path, serde_json::json!(["plink2", "channel"]));
        let mut config = crate::RuntimeConfig::default();
        config.slurm.enabled = true;
        config.slurm.apptainer_manifest = Some(fixture.path().join("images.json"));
        config.plugin_rsi.environment_dev.enabled = false;
        config.slurm.typed_worker_config = None;
        assert!(!requires_local_containers(&config, "", false).unwrap());
        config.slurm.typed_worker_config = Some(path.clone());
        assert!(!requires_local_containers(&config, "exploratory", false).unwrap());
        assert!(requires_local_containers(&config, "formal", false).unwrap());
        assert!(requires_local_containers(&config, "", true).unwrap());
        config.plugin_rsi.environment_dev.enabled = true;
        assert!(requires_local_containers(&config, "", false).unwrap());
        config.plugin_rsi.environment_dev.enabled = false;
        std::fs::write(&path, b"{}").unwrap();
        assert!(requires_local_containers(&config, "", false).is_err());
    }

    #[test]
    fn only_validated_coordinator_profiles_remove_the_local_backend_requirement() {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("profile.json");
        profile(&path, serde_json::json!([]));
        let mut config = crate::RuntimeConfig::default();
        config.slurm.enabled = true;
        config.slurm.typed_worker_config = Some(path.clone());
        config.plugin_rsi.environment_dev.enabled = false;
        assert!(!requires_local_containers(&config, "exploratory", false).unwrap());
        profile(
            &path,
            serde_json::json!(["channel", "logical_gather", "dynamic_fanout"]),
        );
        assert!(!requires_local_containers(&config, "", false).unwrap());
        profile(
            &path,
            serde_json::json!(["core/some_operator_local_method"]),
        );
        assert!(requires_local_containers(&config, "", false).unwrap());
        profile(&path, serde_json::json!([]));
        assert!(requires_local_containers(&config, "formal", false).unwrap());
        assert!(requires_local_containers(&config, "", true).unwrap());
        config.plugin_rsi.environment_dev.enabled = true;
        assert!(
            requires_local_containers(&config, "", false)
                .unwrap_err()
                .to_string()
                .contains("environment development")
        );
        config.plugin_rsi.environment_dev.enabled = false;
        config.slurm.enabled = false;
        assert!(requires_local_containers(&config, "", false).is_err());
        config.slurm.enabled = true;
        std::fs::write(&path, b"{}").unwrap();
        assert!(requires_local_containers(&config, "", false).is_err());
        config.slurm.typed_worker_config = None;
        assert!(requires_local_containers(&config, "", false).unwrap());
    }

    #[tokio::test]
    async fn coordinator_runtime_rejects_a_local_container_without_creating_its_workspace() {
        let fixture = tempfile::tempdir().unwrap();
        let mut infra = container_runtime::ContainerExecutionInfra::from_config(
            container_runtime::PodmanConfig::for_state_dir(fixture.path()),
        );
        disable_local_container_execution(&mut infra);
        assert_eq!(infra.runtime.name(), "remote-only-coordinator");
        let request = container_runtime::ContainerRunRequest {
            image: "must-not-run".into(),
            command: vec!["touch".into(), "MUST_NOT_BE_CREATED".into()],
            workspace: container_runtime::WorkspaceRef {
                host_path: infra.config.workspace_root.join("must-not-create"),
                container_workdir: "/work".into(),
            },
            env: vec![],
            panels: vec![],
            input_mounts: vec![],
            network: container_runtime::ContainerNetwork::Isolated,
            read_only_rootfs: true,
            pull_policy: container_runtime::PullPolicy::Never,
            cpus: None,
            memory: None,
            pids_limit: None,
            shm_size: None,
            gpus: container_runtime::GpuRequest::None,
            user: None,
            timeout_secs: 1,
            name: "coordinator-must-not-run".into(),
        };
        let error = infra.runtime.run(request).await.unwrap_err();
        assert!(error.to_string().contains("disabled by remote-only"));
        assert!(!infra.config.workspace_root.exists());
    }

    #[test]
    fn remote_task_limit_is_opt_in_and_rejects_invalid_or_misspelled_values() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("profile.json");
        profile(&path, serde_json::json!([]));
        assert_eq!(read_profile(&path).unwrap().max_remote_tasks, None);
        let original: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        for value in [
            serde_json::json!(0),
            serde_json::json!(1025),
            serde_json::json!(-1),
            serde_json::json!(1.5),
            serde_json::json!(8.0),
            serde_json::json!(true),
            serde_json::json!("8"),
        ] {
            let mut content = original.clone();
            content["max_remote_tasks"] = value;
            std::fs::write(&path, serde_json::to_vec(&content).unwrap()).unwrap();
            assert!(read_profile(&path).is_err());
        }
        let mut unlimited = original.clone();
        unlimited["max_remote_tasks"] = serde_json::Value::Null;
        std::fs::write(&path, serde_json::to_vec(&unlimited).unwrap()).unwrap();
        assert_eq!(read_profile(&path).unwrap().max_remote_tasks, None);
        for value in [1, 8, 16, 1024] {
            let mut content = original.clone();
            content["max_remote_tasks"] = serde_json::json!(value);
            std::fs::write(&path, serde_json::to_vec(&content).unwrap()).unwrap();
            assert_eq!(read_profile(&path).unwrap().max_remote_tasks, Some(value));
        }
        let mut content = original;
        content["max_remote_task"] = serde_json::json!(8);
        std::fs::write(&path, serde_json::to_vec(&content).unwrap()).unwrap();
        assert!(read_profile(&path).is_err());
    }

    #[test]
    fn refuses_symlink_fifo_and_writable_profiles_without_blocking() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("profile.json");
        std::fs::write(&path, b"{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(read_profile(&path).is_err());
        let link = directory.path().join("link.json");
        symlink(&path, &link).unwrap();
        assert!(read_profile(&link).is_err());
        let fifo = directory.path().join("fifo");
        use std::os::unix::ffi::OsStrExt;
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: name is a valid NUL-terminated path in the owned toy fixture.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(read_profile(&fifo).is_err());
    }

    #[test]
    fn rejects_unknown_fields_and_oversize_profiles() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("profile.json");
        std::fs::write(&path, b"{\"worker\":{},\"allow_formal\":true}").unwrap();
        assert!(read_profile(&path).is_err());
        std::fs::write(&path, vec![b' '; MAX_PROFILE_BYTES as usize + 1]).unwrap();
        assert!(read_profile(&path).is_err());
    }
}
