//! Operator-owned placement for an ordinary thin coordinator.
//!
//! This is not an admission path for signed research workers. Their existing
//! native launcher and result custody remain separate.

use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dag_core::dag::execution::typed_worker::TypedPodmanResourceControl;
use data_engine::dag::execution::{
    LimitedTaskExecutor, MAX_REMOTE_TASKS, PlacementTaskExecutor, RemoteTaskExecutor,
    SlurmSettings, SlurmTaskTransport, SshArtifactStore, SshDataHome, TaskExecutor,
    TypedRemoteTaskExecutor, TypedWorkerSettings,
};
use serde::Deserialize;

use crate::error::{Error, Result};

const MAX_PROFILE_BYTES: u64 = 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PlacementProfile {
    pub(crate) worker: TypedWorkerSettings,
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
    decode_profile(&bytes)
}

pub(crate) fn decode_profile(bytes: &[u8]) -> Result<PlacementProfile> {
    if bytes.len() as u64 > MAX_PROFILE_BYTES {
        return Err(Error::Other("typed-worker profile exceeds 1 MiB".into()));
    }
    let profile: PlacementProfile = serde_json::from_slice(bytes)
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

pub(crate) fn configured_profile(
    settings: &crate::config::SlurmConfig,
) -> Result<Option<PlacementProfile>> {
    if let Some(value) = &settings.typed_worker {
        let bytes = serde_json::to_vec(value).map_err(|error| Error::Other(error.to_string()))?;
        return decode_profile(&bytes).map(Some);
    }
    settings
        .typed_worker_config
        .as_deref()
        .map(read_profile)
        .transpose()
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
    if !config.slurm.has_typed_worker() {
        return Ok(!slurm_containers);
    }
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
    let profile = configured_profile(&config.slurm)?
        .ok_or_else(|| Error::Other("missing typed-worker profile".into()))?;
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

pub(crate) fn executor(
    profile: PlacementProfile,
    data_home: Option<SshDataHome>,
    settings: SlurmSettings,
    workspace: &Path,
    storage: &vfs::OpendalFileStorage,
    local: Arc<dyn TaskExecutor>,
) -> Result<(Arc<dyn TaskExecutor>, String)> {
    let mut store = SshArtifactStore::new(
        settings.ssh.clone(),
        settings.scratch_dir.clone(),
        settings.local_artifact_root.clone(),
    );
    // The catalog's data-domain declaration is authoritative when
    // present; the worker-profile field remains the legacy fallback for
    // pre-existing frozen enrollments.
    let data_home = data_home.or_else(|| profile.data_home.clone());
    if let Some(data_home) = data_home {
        data_home
            .validate_vfs(storage)
            .map_err(|error| Error::Other(error.to_string()))?;
        store = store
            .with_data_home(data_home)
            .map_err(|error| Error::Other(error.to_string()))?;
    }
    let mut profile = profile;
    if profile.worker.podman.as_ref().is_some_and(|podman| {
        podman.resource_control == TypedPodmanResourceControl::SshDelegatedCgroups
    }) {
        return Err(Error::Other(
            "SSH Podman cgroup mode cannot be used for Slurm".into(),
        ));
    }
    if let Some(sif) = &settings.apptainer {
        if !sif.runtime.is_default() && profile.worker.podman.is_some() {
            return Err(Error::Other(
                "SingularityCE cannot override a typed worker pinned to Podman".into(),
            ));
        }
        if profile.worker.apptainer_catalog.is_some() {
            profile.worker.sif_runtime = sif.runtime;
            if !sif.runtime.is_default() {
                profile.worker.apptainer_executable = Some(sif.program.clone());
            }
        }
    }
    let remote: Arc<dyn TaskExecutor> = Arc::new(RemoteTaskExecutor::new(
        Arc::new(SlurmTaskTransport::new(settings)),
        Arc::new(store),
        workspace,
    ));
    assemble_profile(profile, remote, false, local)
}

pub(crate) fn validate_ssh_podman(
    plugins_root: &Path,
    plugins_sha256: &str,
    podman: &dag_core::dag::execution::typed_worker::TypedPodmanSettings,
) -> Result<()> {
    podman
        .validate()
        .map_err(|error| Error::Other(error.to_string()))?;
    if !plugins_root.is_absolute()
        || plugins_sha256.len() != 64
        || !plugins_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        || podman.image_catalog.is_none()
    {
        return Err(Error::Other("SSH Podman requires pinned plugins and an offline image catalog in the typed-worker profile".into()));
    }
    if matches!(
        podman.resource_control,
        TypedPodmanResourceControl::SlurmAllocation { .. }
    ) {
        return Err(Error::Other(
            "SSH Podman cannot use Slurm allocation limits".into(),
        ));
    }
    Ok(())
}

pub(crate) fn assemble_ssh_typed(
    mut profile: PlacementProfile,
    remote: Arc<dyn TaskExecutor>,
    local: Arc<dyn TaskExecutor>,
) -> Result<(Arc<dyn TaskExecutor>, String)> {
    let worker = &mut profile.worker;
    let podman = worker.podman.as_mut().ok_or_else(|| {
        Error::Other(
            "SSH Podman requires a pinned podman section in the typed-worker profile".into(),
        )
    })?;
    let (plugins_root, plugins_sha256) = worker
        .plugins_root
        .as_deref()
        .zip(worker.plugins_sha256.as_deref())
        .ok_or_else(|| Error::Other("SSH Podman requires a pinned plugin directory".into()))?;
    validate_ssh_podman(plugins_root, plugins_sha256, podman)?;
    podman.resource_control = TypedPodmanResourceControl::SshDelegatedCgroups;
    assemble_profile(profile, remote, true, local)
}

fn assemble_profile(
    profile: PlacementProfile,
    remote: Arc<dyn TaskExecutor>,
    worker_container_dispatch: bool,
    local: Arc<dyn TaskExecutor>,
) -> Result<(Arc<dyn TaskExecutor>, String)> {
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
    let placement = PlacementTaskExecutor::new(local, Some(remote), profile.local_kinds)
        .map_err(|error| Error::Other(error.to_string()))?;
    let placement = if worker_container_dispatch {
        placement.with_worker_container_dispatch()
    } else {
        placement.with_remote_container_dispatch()
    };
    Ok((Arc::new(placement), source_revision))
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
    fn ssh_podman_requires_pinned_plugins_and_images() {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("profile.json");
        profile(&path, serde_json::json!([]));
        let build = || {
            assemble_ssh_typed(
                read_profile(&path).unwrap(),
                Arc::new(dag_core::LocalTaskExecutor::with_workspace_root(
                    fixture.path().join("remote"),
                )),
                Arc::new(dag_core::LocalTaskExecutor::with_workspace_root(
                    fixture.path().join("local"),
                )),
            )
        };
        assert!(build().is_err());

        let mut config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let worker = config["worker"].as_object_mut().unwrap();
        worker.insert("plugins_root".into(), "/site/plugins".into());
        worker.insert("plugins_sha256".into(), "b".repeat(64).into());
        worker.insert(
            "podman".into(),
            serde_json::json!({
                "executable": "/usr/bin/podman",
                "executable_sha256": "c".repeat(64),
            }),
        );
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        assert!(build().is_err());
        config["worker"]["podman"]["image_catalog"] = "/site/images.json".into();
        config["worker"]["podman"]["image_catalog_sha256"] = "d".repeat(64).into();
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        assert!(build().is_ok());

        config["worker"]["podman"]["resource_control"] =
            serde_json::json!({"mode":"slurm_allocation","swap_policy":"inherit"});
        config["worker"]["podman"]["oci_runtime"] = "/usr/bin/crun".into();
        config["worker"]["podman"]["oci_runtime_sha256"] = "e".repeat(64).into();
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        assert!(build().is_err());
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
