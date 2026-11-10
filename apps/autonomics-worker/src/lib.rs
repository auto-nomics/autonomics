//! Independent ordinary typed worker. No daemon, history database, model
//! endpoint, research store or signed scientific execution entry is exposed.
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use dag_core::dag::execution::typed_worker::{execute_typed_worker, typed_worker_envelope};
use sha2::{Digest, Sha256};

const MAX_INPUT_ENV_BYTES: usize = 64 * 1024;

fn invalid_input(message: &'static str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message)
}

type InputPaths = BTreeMap<String, Vec<PathBuf>>;
type InputArtifacts = BTreeMap<String, Vec<dag_core::value::FileRef>>;

fn parse_inputs(paths: &str, artifacts: &str) -> std::io::Result<(InputPaths, InputArtifacts)> {
    if paths.len() > MAX_INPUT_ENV_BYTES || artifacts.len() > MAX_INPUT_ENV_BYTES {
        return Err(invalid_input(
            "typed worker input metadata exceeds 64 KiB per environment value",
        ));
    }
    let paths: InputPaths = serde_json::from_str(paths).map_err(std::io::Error::other)?;
    let artifacts: InputArtifacts =
        serde_json::from_str(artifacts).map_err(std::io::Error::other)?;
    if paths.keys().ne(artifacts.keys()) {
        return Err(invalid_input(
            "typed worker input names differ from transport artifacts",
        ));
    }
    for (name, files) in &paths {
        let expected = &artifacts[name];
        if files.len() != expected.len() {
            return Err(invalid_input(
                "typed worker input cardinality differs from transport artifacts",
            ));
        }
        for (path, artifact) in files.iter().zip(expected) {
            let path = path
                .to_str()
                .ok_or_else(|| invalid_input("typed worker input path must be UTF-8"))?;
            if path != artifact.path
                || !path.starts_with('/')
                || path == "/"
                || path.chars().any(char::is_control)
                || path
                    .split('/')
                    .skip(1)
                    .any(|part| part.is_empty() || matches!(part, "." | ".."))
            {
                return Err(invalid_input(
                    "typed worker input path differs from transport or is not a clean absolute path",
                ));
            }
            let expected = artifact
                .fingerprint
                .as_ref()
                .ok_or_else(|| invalid_input("typed worker input has no transport fingerprint"))?;
            let hash = expected
                .content_hash
                .as_deref()
                .ok_or_else(|| invalid_input("typed worker input has no transport SHA256"))?;
            let hash = hash.strip_prefix("sha256:").unwrap_or(hash);
            if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(invalid_input("typed worker transport SHA256 is malformed"));
            }
        }
    }
    Ok((paths, artifacts))
}

fn verify_manifest_inputs(
    artifacts: &InputArtifacts,
    manifest: Option<&serde_json::Value>,
) -> std::io::Result<()> {
    match manifest {
        Some(manifest)
            if &serde_json::to_value(artifacts).map_err(std::io::Error::other)? == manifest =>
        {
            Ok(())
        }
        None if artifacts.is_empty() => Ok(()),
        _ => Err(invalid_input(
            "typed worker input artifacts differ from the durable transport manifest",
        )),
    }
}

fn use_snapshot_metadata(
    envelope: &mut dag_core::dag::execution::typed_worker::TypedWorkerEnvelope,
    paths: &InputPaths,
    artifacts: &InputArtifacts,
) -> std::io::Result<()> {
    for binding in &envelope.task.inputs {
        let sources = artifacts
            .get(&binding.name)
            .ok_or_else(|| invalid_input("typed worker snapshot binding is missing"))?;
        let snapshots = paths
            .get(&binding.name)
            .ok_or_else(|| invalid_input("typed worker snapshot paths are missing"))?;
        if sources.len() != snapshots.len() {
            return Err(invalid_input("typed worker snapshot cardinality changed"));
        }
        let mut verified = sources.clone();
        for (index, (file, snapshot)) in verified.iter_mut().zip(snapshots).enumerate() {
            file.path = snapshot
                .to_str()
                .ok_or_else(|| invalid_input("typed worker snapshot path must be UTF-8"))?
                .into();
            // Keep semantic file formats from the typed source, including an
            // intentionally absent label; retain the transport's verified hash.
            if let Some(original) = envelope
                .input_files
                .get(&binding.port)
                .and_then(|files| files.get(index))
            {
                file.format = original.format.clone();
            }
        }
        envelope.input_files.insert(binding.port, verified);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn open_input_no_follow(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;

    // Every ancestor is opened relative to the previous held descriptor, so
    // neither a final symlink nor a swapped ancestor can redirect this read.
    let mut directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open("/")?;
    let components = path.components().skip(1).collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        let component = std::ffi::CString::new(component.as_os_str().as_bytes())?;
        let flags = if index + 1 == components.len() {
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC
        } else {
            // Search permission suffices to traverse a site-owned data-home
            // ancestor; directory listing permission must not be required.
            libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC
        };
        // SAFETY: the parent FD and NUL-terminated component live across the
        // call. A successful descriptor is immediately given unique ownership.
        let descriptor = unsafe { libc::openat(directory.as_raw_fd(), component.as_ptr(), flags) };
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: openat succeeded and no other object owns this new FD.
        directory = unsafe { std::fs::File::from_raw_fd(descriptor) };
    }
    Ok(directory)
}

#[cfg(target_os = "linux")]
fn input_identity(
    metadata: &std::fs::Metadata,
) -> (u64, u64, u32, u32, u32, u64, u64, i64, i64, i64, i64) {
    use std::os::unix::fs::MetadataExt;
    (
        metadata.dev(),
        metadata.ino(),
        metadata.mode(),
        metadata.uid(),
        metadata.gid(),
        metadata.nlink(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    )
}

/// Take a private, same-host transport snapshot before constructing/running
/// any node. These files are attempt scratch, never a new canonical data home.
#[cfg(target_os = "linux")]
fn snapshot_inputs(
    workspace: &Path,
    paths: InputPaths,
    artifacts: InputArtifacts,
) -> std::io::Result<InputPaths> {
    use std::io::{Read, Write};
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

    let root = workspace.join("typed-inputs");
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    let mut snapshots = BTreeMap::new();
    for (binding_index, (name, files)) in paths.into_iter().enumerate() {
        let mut staged = Vec::with_capacity(files.len());
        for (file_index, (path, expected)) in files.into_iter().zip(&artifacts[&name]).enumerate() {
            let fingerprint = expected
                .fingerprint
                .as_ref()
                .ok_or_else(|| invalid_input("typed worker input fingerprint missing"))?;
            let expected_hash = fingerprint
                .content_hash
                .as_deref()
                .ok_or_else(|| invalid_input("typed worker input SHA256 missing"))?;
            let expected_hash = expected_hash
                .strip_prefix("sha256:")
                .unwrap_or(expected_hash);
            let mut source = open_input_no_follow(&path)?;
            let before = source.metadata()?;
            if !before.is_file() || before.len() != fingerprint.size {
                return Err(invalid_input(
                    "typed worker input type or size changed after transport admission",
                ));
            }
            // Preserve the original basename and extension without allowing
            // collisions between different ports/files or overwriting a file.
            let parent = root.join(format!("input-{binding_index}-{file_index}"));
            std::fs::DirBuilder::new().mode(0o700).create(&parent)?;
            let destination = parent.join(
                path.file_name()
                    .ok_or_else(|| invalid_input("typed worker input basename missing"))?,
            );
            let mut target = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&destination)?;
            let mut hash = Sha256::new();
            let mut count = 0_u64;
            let mut buffer = vec![0_u8; 1024 * 1024];
            loop {
                let size = source.read(&mut buffer)?;
                if size == 0 {
                    break;
                }
                count = count
                    .checked_add(size as u64)
                    .ok_or_else(|| invalid_input("typed worker input length overflow"))?;
                if count > fingerprint.size {
                    return Err(invalid_input(
                        "typed worker input grew after transport admission",
                    ));
                }
                hash.update(&buffer[..size]);
                target.write_all(&buffer[..size])?;
            }
            let after = source.metadata()?;
            let named = open_input_no_follow(&path)?.metadata()?;
            if count != fingerprint.size
                || input_identity(&before) != input_identity(&after)
                || input_identity(&before) != input_identity(&named)
                || !format!("{:x}", hash.finalize()).eq_ignore_ascii_case(expected_hash)
            {
                return Err(invalid_input(
                    "typed worker input identity or SHA256 changed after transport admission",
                ));
            }
            target.sync_all()?;
            target.set_permissions(std::fs::Permissions::from_mode(0o400))?;
            staged.push(destination);
        }
        snapshots.insert(name, staged);
    }
    Ok(snapshots)
}

#[cfg(not(target_os = "linux"))]
fn snapshot_inputs(_: &Path, _: InputPaths, _: InputArtifacts) -> std::io::Result<InputPaths> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "typed worker input snapshots require Linux O_PATH/no-follow descriptors",
    ))
}

/// Read a pinned worker VFS policy through one held descriptor and bounded
/// bytes, rejecting a changed identity instead of following replacement paths.
#[cfg(target_os = "linux")]
fn load_worker_vfs_manifest(
    path: &Path,
    expected: &str,
) -> Result<vfs::VfsManifest, Box<dyn std::error::Error>> {
    use std::io::Read;
    use std::os::unix::fs::MetadataExt;
    const MAX_BYTES: u64 = 4 * 1024 * 1024;
    let file = open_input_no_follow(path)?;
    let before = file.metadata()?;
    // SAFETY: geteuid only observes the current process credential.
    let uid = unsafe { libc::geteuid() };
    if !before.is_file()
        || before.len() > MAX_BYTES
        || before.nlink() != 1
        || before.mode() & 0o022 != 0
        || (before.uid() != 0 && before.uid() != uid)
    {
        return Err(
            "worker VFS manifest must be a bounded, operator/root-owned, non-writable regular file"
                .into(),
        );
    }
    let mut bytes = Vec::new();
    (&file).take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    let named = std::fs::symlink_metadata(path)?;
    if bytes.len() as u64 > MAX_BYTES
        || input_identity(&before) != input_identity(&after)
        || input_identity(&after) != input_identity(&named)
    {
        return Err("worker VFS manifest changed while reading".into());
    }
    if !format!("{:x}", Sha256::digest(&bytes)).eq_ignore_ascii_case(expected) {
        return Err("worker VFS manifest SHA256 mismatch".into());
    }
    let manifest = vfs::VfsManifest::from_toml(std::str::from_utf8(&bytes)?)?;
    if manifest.mount.iter().any(|mount| !mount.read_only) {
        return Err(
            "worker data-home VFS mounts must be read-only; outputs use the attempt object store"
                .into(),
        );
    }
    Ok(manifest)
}

#[cfg(not(target_os = "linux"))]
fn load_worker_vfs_manifest(
    _path: &Path,
    _expected: &str,
) -> Result<vfs::VfsManifest, Box<dyn std::error::Error>> {
    Err("typed worker VFS requires descriptor-relative Linux confinement".into())
}

/// Bind both storage and the DataFusion mount adapter to the original caller.
/// A worker VFS never inherits the worker process's default operator identity.
fn worker_storage(
    storage_root: &Path,
    manifest: Option<vfs::VfsManifest>,
    principal: Option<vfs::permission::VfsPrincipal>,
) -> Result<
    (
        Arc<vfs::OpendalFileStorage>,
        Option<Arc<vfs::MountedObjectStore>>,
    ),
    Box<dyn std::error::Error>,
> {
    if manifest.is_some() && principal.is_none() {
        return Err("worker VFS requires the original coordinator VFS principal".into());
    }
    if let Some(principal) = &principal {
        principal
            .validate_transport()
            .map_err(std::io::Error::other)?;
    }
    let mounts = manifest
        .map(|manifest| {
            let mounted = vfs::MountedObjectStore::from_manifest(&manifest)?;
            Ok::<_, vfs::mount::MountError>(Arc::new(match &principal {
                Some(principal) => mounted.with_principal(principal.clone()),
                None => mounted,
            }))
        })
        .transpose()?;
    let storage = if let Some(mounts) = &mounts {
        vfs::OpendalFileStorage::with_mounts(storage_root, mounts.clone())
    } else {
        vfs::OpendalFileStorage::new(storage_root)
    };
    let storage = match principal {
        Some(principal) => storage.with_principal(principal),
        None => storage,
    };
    Ok((Arc::new(storage), mounts))
}

/// Read-only deployment preparation. This opens no task workspace or database.
pub fn identity_report(
    plugins_root: Option<&Path>,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    use std::io::Read;
    let mut binary = std::fs::File::open(std::env::current_exe()?)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = binary.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    let plugin_hash = plugins_root
        .map(dag_core::dag::execution::typed_worker::typed_worker_tree_sha256)
        .transpose()?;
    Ok(serde_json::json!({
        "artifact": "autonomics-worker", "protocol": 1,
        "capabilities": ["task-worker"],
        "source_revision": dag_core::source_revision(),
        "engine_version": dag_core::engine_version(),
        "executable_sha256": format!("{:x}", hash.finalize()),
        "plugins_sha256": plugin_hash,
    }))
}

fn podman_worker_policy(
    settings: &dag_core::dag::execution::typed_worker::TypedPodmanSettings,
) -> (
    container_runtime::PodmanWorkerResourceControl,
    container_runtime::PodmanWorkerUserNamespace,
) {
    use dag_core::dag::execution::typed_worker::{
        TypedPodmanResourceControl, TypedPodmanUserNamespace, TypedSlurmSwapPolicy,
    };
    let resource_control = match settings.resource_control {
        TypedPodmanResourceControl::DelegatedCgroups => {
            container_runtime::PodmanWorkerResourceControl::DelegatedCgroups
        }
        TypedPodmanResourceControl::SlurmAllocation { swap_policy } => {
            container_runtime::PodmanWorkerResourceControl::SlurmAllocation {
                swap_policy: match swap_policy {
                    TypedSlurmSwapPolicy::Inherit => container_runtime::SlurmSwapPolicy::Inherit,
                    TypedSlurmSwapPolicy::Disabled => container_runtime::SlurmSwapPolicy::Disabled,
                },
            }
        }
    };
    let user_namespace = match settings.user_namespace {
        TypedPodmanUserNamespace::KeepId => container_runtime::PodmanWorkerUserNamespace::KeepId,
        TypedPodmanUserNamespace::SingleUidRoot => {
            container_runtime::PodmanWorkerUserNamespace::SingleUidRoot
        }
    };
    (resource_control, user_namespace)
}

fn create_private_directory(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

pub async fn run(dispatch_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("worker: reading dispatch and verifying executable identity");
    let worker_started = std::time::Instant::now();
    let workspace = PathBuf::from(std::env::var("AUTONOMICS_WORKSPACE")?);
    if !workspace.is_absolute()
        || workspace.canonicalize()? != workspace
        || dispatch_path != workspace.join("task.json")
    {
        return Err("typed worker requires the canonical transport task workspace/manifest".into());
    }
    let metadata = std::fs::symlink_metadata(dispatch_path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 16 * 1024 * 1024
    {
        return Err("typed dispatch must be a bounded regular file".into());
    }
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(dispatch_path)?
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err("typed dispatch exceeds 16 MiB".into());
    }
    let dispatch: dag_core::TaskDispatch = serde_json::from_slice(&bytes)?;
    dispatch.resources.validate()?;
    let mut envelope = typed_worker_envelope(&dispatch)?;
    if std::env::var("AUTONOMICS_TASK_ID")? != envelope.task.id {
        return Err("typed worker task identity does not match transport".into());
    }
    let heartbeat = dag_core::dag::execution::heartbeat::WorkerHeartbeat::start(
        dispatch_path,
        &dispatch.spec.id,
    );
    if let Some(heartbeat) = &heartbeat {
        heartbeat.phase("verifying_inputs");
    }
    // Both values are reserved transport output, never agent-specified env.
    // Validate and snapshot before loading plugins or executing any node.
    tracing::info!(task_id = %envelope.task.id, kind = %envelope.task.kind, "worker: verifying and snapshotting inputs");
    let artifact_env = match std::env::var("AUTONOMICS_INPUT_ARTIFACTS") {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => "{}".into(),
        Err(error) => return Err(error.into()),
    };
    let (paths, artifacts) = parse_inputs(&std::env::var("AUTONOMICS_INPUTS")?, &artifact_env)?;
    verify_manifest_inputs(
        &artifacts,
        dispatch.spec.spec.get("_autonomics_input_artifacts"),
    )?;
    let verified_artifacts = artifacts.clone();
    let snapshot_workspace = workspace.clone();
    let paths =
        tokio::task::spawn_blocking(move || snapshot_inputs(&snapshot_workspace, paths, artifacts))
            .await??;
    use_snapshot_metadata(&mut envelope, &paths, &verified_artifacts)?;
    if let Some(heartbeat) = &heartbeat {
        heartbeat.phase("preparing_runtime");
    }
    tracing::info!("worker: preparing node registry and runtime");
    let state = workspace.join("typed-state");
    create_private_directory(&state)?;
    let containers = if let Some(podman) = &envelope.worker.podman {
        let (resource_control, user_namespace) = podman_worker_policy(podman);
        container_runtime::ContainerExecutionInfra::for_podman_worker_with_policy(
            &state,
            &podman.executable,
            &podman.executable_sha256,
            podman
                .image_catalog
                .as_deref()
                .zip(podman.image_catalog_sha256.as_deref()),
            podman.panel_cache_root.as_deref(),
            envelope
                .plugin_identity
                .as_ref()
                .map(|identity| identity.image_reference.as_str()),
            dispatch.resources.max_duration_ms.map(|budget| {
                budget.saturating_sub(
                    worker_started.elapsed().as_millis().min(u64::MAX as u128) as u64
                )
            }),
            resource_control,
            user_namespace,
            podman
                .oci_runtime
                .as_deref()
                .zip(podman.oci_runtime_sha256.as_deref()),
        )
        .await?
    } else {
        match (
            &envelope.worker.apptainer_catalog,
            &envelope.worker.apptainer_executable,
        ) {
            (Some(catalog), Some(executable)) => {
                container_runtime::ContainerExecutionInfra::for_apptainer_worker(
                    &state, catalog, executable,
                )
                .map_err(std::io::Error::other)?
            }
            (None, None) if envelope.worker.plugins_root.is_none() => {
                let mut config = container_runtime::PodmanConfig::for_state_dir(&state);
                // Native-only tasks cannot discover an ambient container backend.
                config.program = "/usr/bin/false".into();
                container_runtime::ContainerExecutionInfra::from_config(config)
            }
            _ => {
                return Err(
                "ordinary container plugins require an explicitly configured Apptainer or pinned Podman backend"
                    .into(),
            );
            }
        }
    };
    let plugins = if let Some(root) = &envelope.worker.plugins_root {
        root.clone()
    } else {
        let root = state.join("empty-plugins");
        std::fs::create_dir(&root)?;
        root
    };
    let storage_root = state.join("objects");
    std::fs::create_dir(&storage_root)?;
    let manifest = match (
        &envelope.worker.worker_vfs_manifest,
        &envelope.worker.worker_vfs_sha256,
    ) {
        (Some(path), Some(expected)) => Some(load_worker_vfs_manifest(path, expected)?),
        (None, None) => None,
        _ => return Err("worker VFS manifest/pin must be configured together".into()),
    };
    let (storage, mounts) = worker_storage(&storage_root, manifest, envelope.vfs_principal()?)?;
    containers.record_podman_worker_helpers(&state, "after-import")?;
    let worker_containers = containers.clone();
    let session = datafusion::prelude::SessionContext::new();
    let runtime = session.runtime_env();
    let file_url = datafusion::execution::object_store::ObjectStoreUrl::parse("file://")?;
    runtime.register_object_store(file_url.as_ref(), storage.clone());
    storage.register_domain_stores(&runtime)?;
    if mounts.is_some() {
        let vfs_url = datafusion::execution::object_store::ObjectStoreUrl::parse("vfs://")?;
        runtime.register_object_store(vfs_url.as_ref(), storage.clone());
    }
    let registry = data_engine::default_registry::build_default_registry_with_container_execution(
        runtime,
        Some(storage),
        Arc::new(dag_core::BundleRegistry::from_bundles(
            envelope.bundles.clone(),
        )?),
        Arc::new(containers),
        Some(plugins),
    );
    if let Some(heartbeat) = &heartbeat {
        heartbeat.phase("executing");
    }
    tracing::info!("worker: executing analysis node");
    let result = execute_typed_worker(&envelope, &registry, paths, &workspace).await;
    if let Some(heartbeat) = &heartbeat {
        heartbeat.phase("collecting_outputs");
    }
    // Each container request keeps its existing owned-intent cleanup. This
    // final pause observation grants no runtime-directory deletion authority.
    worker_containers.record_podman_worker_helpers(&state, "after-execution")?;
    result?;
    tracing::info!("worker: outputs ready for collection");
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use dag_core::FileRef;
    use std::os::unix::fs::{PermissionsExt, symlink};
    fn artifact(path: &Path, bytes: &[u8]) -> FileRef {
        FileRef::remote(
            path.to_str().unwrap(),
            Some("toy".into()),
            bytes.len() as u64,
            Some(format!("{:x}", Sha256::digest(bytes))),
        )
    }

    fn admitted(path: &Path, expected: FileRef) -> std::io::Result<(InputPaths, InputArtifacts)> {
        parse_inputs(
            &serde_json::to_string(&BTreeMap::from([("files", vec![path])])).unwrap(),
            &serde_json::to_string(&BTreeMap::from([("files", vec![expected])])).unwrap(),
        )
    }

    #[test]
    fn worker_takes_private_read_only_snapshots_and_never_reuses_original_paths() {
        let fixture = tempfile::tempdir().unwrap();
        let source = fixture.path().join("source.tsv");
        let bytes = b"synthetic\tvalue\n";
        std::fs::write(&source, bytes).unwrap();
        let (paths, artifacts) = admitted(&source, artifact(&source, bytes)).unwrap();
        let snapshots = snapshot_inputs(fixture.path(), paths, artifacts).unwrap();
        let snapshot = &snapshots["files"][0];
        assert!(snapshot.starts_with(fixture.path().join("typed-inputs")));
        assert_eq!(snapshot.file_name(), source.file_name());
        assert_ne!(snapshot, &source);
        assert_eq!(
            std::fs::metadata(snapshot).unwrap().permissions().mode() & 0o777,
            0o400
        );
        assert_eq!(
            std::fs::metadata(fixture.path().join("typed-inputs"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        std::fs::write(&source, b"new canonical content").unwrap();
        assert_eq!(std::fs::read(snapshot).unwrap(), bytes);
    }

    #[test]
    fn worker_rejects_modified_bytes_size_and_missing_transport_hash() {
        for replacement in [b"tampered".as_slice(), b"short".as_slice()] {
            let fixture = tempfile::tempdir().unwrap();
            let source = fixture.path().join("source");
            let expected = artifact(&source, b"original");
            std::fs::write(&source, replacement).unwrap();
            let (paths, artifacts) = admitted(&source, expected).unwrap();
            assert!(snapshot_inputs(fixture.path(), paths, artifacts).is_err());
        }
        let fixture = tempfile::tempdir().unwrap();
        let source = fixture.path().join("source");
        let mut expected = artifact(&source, b"original");
        expected.fingerprint.as_mut().unwrap().content_hash = None;
        assert!(admitted(&source, expected).is_err());
    }

    #[test]
    fn worker_refuses_final_and_ancestor_symlinks_even_with_the_expected_bytes() {
        for ancestor in [false, true] {
            let fixture = tempfile::tempdir().unwrap();
            let directory = fixture.path().join("data");
            std::fs::create_dir(&directory).unwrap();
            let source = directory.join("source");
            let bytes = b"synthetic authorized bytes";
            std::fs::write(&source, bytes).unwrap();
            let path = if ancestor {
                let link = fixture.path().join("linked-directory");
                symlink(&directory, &link).unwrap();
                link.join("source")
            } else {
                let link = fixture.path().join("linked-file");
                symlink(&source, &link).unwrap();
                link
            };
            let (paths, artifacts) = admitted(&path, artifact(&path, bytes)).unwrap();
            assert!(snapshot_inputs(fixture.path(), paths, artifacts).is_err());
        }
    }
}
