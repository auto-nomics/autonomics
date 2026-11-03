//! Explicit ordinary-worker Podman deployment. No host store, default backend,
//! scientific authority, image pull or resource/UID fallback is admitted here.

use std::collections::BTreeSet;
use std::ffi::CStr;
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    ContainerExecutionInfra, ContainerRuntimeError, OciImageImportRequest, PanelCache,
    PodmanConfig, PodmanRuntime, PrivateStorageDriver,
};

/// Ordinary container resource ownership. Slurm mode never asks rootless
/// Podman to create or modify resource cgroups.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PodmanWorkerResourceControl {
    #[default]
    DelegatedCgroups,
    SlurmAllocation {
        swap_policy: crate::SlurmSwapPolicy,
    },
}

/// Explicit operator policy, independent of a container node's `user` field.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PodmanWorkerUserNamespace {
    #[default]
    KeepId,
    /// Container UID/GID 0 map to the non-root Slurm account. Images requiring
    /// other identities are unsupported; archive ownership is never squashed.
    SingleUidRoot,
}

pub(crate) fn write_worker_evidence(
    path: &Path,
    value: &impl Serialize,
) -> Result<(), ContainerRuntimeError> {
    let mut stream = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    serde_json::to_writer_pretty(&mut stream, value).map_err(|error| invalid(error.to_string()))?;
    stream.sync_all()?;
    Ok(())
}

pub(crate) fn validate_single_uid_request(
    request: &crate::ContainerRunRequest,
) -> Result<(), ContainerRuntimeError> {
    if request
        .user
        .as_deref()
        .is_some_and(|user| !matches!(user, "0" | "0:0"))
    {
        return Err(invalid(
            "single-UID Podman supports only the operator-mapped container root; explicit alternate users are refused",
        ));
    }
    Ok(())
}

pub(crate) fn validate_worker_container_name(name: &str) -> Result<(), ContainerRuntimeError> {
    if name.is_empty()
        || name.len() > 128
        || !name.as_bytes()[0].is_ascii_alphanumeric()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return Err(invalid(
            "ordinary Podman container name must be a safe single component",
        ));
    }
    Ok(())
}

fn single_uid_mapping(
    bytes: &[u8],
    host_uid: u32,
    host_gid: u32,
) -> Result<serde_json::Value, ContainerRuntimeError> {
    let text = std::str::from_utf8(bytes).map_err(|error| invalid(error.to_string()))?;
    let lines = text.lines().collect::<Vec<_>>();
    if lines.len() != 2 || host_uid == 0 || host_gid == 0 {
        return Err(invalid(
            "single-UID Podman requires exactly one non-root UID and GID mapping",
        ));
    }
    for (line, host_id) in lines.iter().zip([host_uid, host_gid]) {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() != 3
            || fields[0] != "0"
            || fields[2] != "1"
            || fields[1].parse::<u32>().ok() != Some(host_id)
        {
            return Err(invalid(
                "Podman user namespace is not the exact single operator UID/GID mapping",
            ));
        }
    }
    Ok(
        serde_json::json!({"schema":"ORDINARY_PODMAN_SINGLE_UID_V1", "host_uid":host_uid, "host_gid":host_gid, "uid_map":lines[0], "gid_map":lines[1], "container_uid":0, "container_gid":0, "ownership_squashing":false}),
    )
}

pub(crate) async fn verify_single_uid_namespace(
    config: &PodmanConfig,
    budget_ms: u64,
) -> Result<serde_json::Value, ContainerRuntimeError> {
    if config.worker_runtime_dir.is_none() || config.inherit_user_namespace {
        return Err(invalid(
            "single-UID mapping requires an isolated ordinary Podman backend",
        ));
    }
    secure_path(Path::new("/usr/bin/cat"), false)?;
    let seconds = (budget_ms / 1000).min(10);
    if seconds == 0 {
        return Err(invalid(
            "Podman namespace verification exhausted the task budget",
        ));
    }
    let mut command = config.async_command();
    command.args([
        "unshare",
        "/usr/bin/cat",
        "/proc/self/uid_map",
        "/proc/self/gid_map",
    ]);
    // Own and actually wait the child even when the caller is cancelled.
    // Persistent namespace helpers stay under Slurm and need its stop proof.
    let output = tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        async fn capture(
            pipe: &mut (impl tokio::io::AsyncRead + Unpin),
            bytes: &mut Vec<u8>,
        ) -> std::io::Result<()> {
            let mut buffer = [0u8; 4096];
            loop {
                let n = pipe.read(&mut buffer).await?;
                if n == 0 {
                    return Ok(());
                }
                let retain = n.min(65537usize.saturating_sub(bytes.len()));
                bytes.extend_from_slice(&buffer[..retain]);
            }
        }
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = command.spawn()?;
        let (Some(mut out), Some(mut err)) = (child.stdout.take(), child.stderr.take()) else {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(invalid("Podman namespace verification capture unavailable"));
        };
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let waited = tokio::time::timeout(std::time::Duration::from_secs(seconds), async {
            let (status, (), ()) = tokio::try_join!(
                child.wait(),
                capture(&mut out, &mut stdout),
                capture(&mut err, &mut stderr)
            )?;
            Ok::<_, std::io::Error>(status)
        })
        .await;
        let status = match waited {
            Ok(Ok(status)) => status,
            other => {
                let _ = child.start_kill();
                child.wait().await?;
                return Err(invalid(match other {
                    Err(_) => "Podman namespace verification timed out",
                    _ => "Podman namespace verification capture failed",
                }));
            }
        };
        Ok(std::process::Output {
            status,
            stdout,
            stderr,
        })
    })
    .await
    .map_err(|error| invalid(format!("Podman namespace verifier join failed: {error}")))??;
    if !output.status.success() || output.stdout.len() > 65536 || output.stderr.len() > 65536 {
        return Err(invalid(
            "Podman namespace verification failed or exceeded its output bound",
        ));
    }
    // SAFETY: credential accessors only read the current worker identity.
    single_uid_mapping(&output.stdout, unsafe { libc::geteuid() }, unsafe {
        libc::getegid()
    })
}

fn invalid(message: impl Into<String>) -> ContainerRuntimeError {
    ContainerRuntimeError::Invalid(message.into())
}

fn verify_oci_runtime(config: &PodmanConfig) -> Result<(), ContainerRuntimeError> {
    if let Some((path, hash)) = &config.worker_oci_runtime {
        if secure_path(path, false)?.mode() & 0o111 == 0 {
            return Err(invalid("ordinary worker OCI runtime must be executable"));
        }
        verified_file(path, hash, 256 * 1024 * 1024, false)?;
    }
    Ok(())
}

fn configure_oci_runtime(config: &PodmanConfig, state: &Path) -> Result<(), ContainerRuntimeError> {
    let Some((path, hash)) = &config.worker_oci_runtime else {
        return Ok(());
    };
    verify_oci_runtime(config)?;
    let root = config
        .worker_runtime_dir
        .as_ref()
        .ok_or_else(|| invalid("pinned OCI runtime requires a private ordinary backend"))?;
    let parent = root.join("config/containers");
    std::fs::DirBuilder::new().mode(0o700).create(&parent)?;
    let runtime_path = path
        .to_str()
        .ok_or_else(|| invalid("OCI runtime path must be UTF-8"))?;
    let quoted = serde_json::to_string(runtime_path).map_err(|error| invalid(error.to_string()))?;
    // containers/common v0.57.x (Podman 4.9) decodes the singular TOML key
    // `runtime_supports_nocgroup`; the plural spelling is silently ignored.
    let bytes = format!(
        "[engine]\nruntime=\"autonomics-crun\"\nruntime_supports_nocgroup=[\"autonomics-crun\"]\nruntime_supports_json=[\"autonomics-crun\"]\n[engine.runtimes]\nautonomics-crun=[{quoted}]\n"
    );
    let file = parent.join("containers.conf");
    use std::io::Write;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&file)?;
    output.write_all(bytes.as_bytes())?;
    output.sync_all()?;
    let metadata = secure_path(path, false)?;
    write_worker_evidence(
        &state.join("podman-oci-runtime.json"),
        &serde_json::json!({
            "schema":"ORDINARY_PODMAN_OCI_RUNTIME_V1", "path":path, "sha256":hash,
            "dev":metadata.dev(), "ino":metadata.ino(), "uid":metadata.uid(), "mode":metadata.mode(),
            "engine_name":"autonomics-crun", "config_path":file, "config_sha256":format!("{:x}",Sha256::digest(bytes.as_bytes())),
            "container_execution_proven":false,
        }),
    )?;
    Ok(())
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn immutable_image(image: &str) -> bool {
    image.rsplit_once("@sha256:").is_some_and(|(name, hash)| {
        !name.is_empty()
            && !name.starts_with('-')
            && digest(hash)
            && !name
                .bytes()
                .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    })
}

fn secure_path(path: &Path, directory: bool) -> Result<std::fs::Metadata, ContainerRuntimeError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        || path
            .to_string_lossy()
            .split('/')
            .any(|part| part == "." || part == "..")
    {
        return Err(invalid("Podman operator paths must be clean and absolute"));
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
        || path.canonicalize()? != path
    {
        return Err(invalid(
            "Podman operator paths must be canonical with the required type",
        ));
    }
    // SAFETY: geteuid has no arguments and only reads the current credential.
    let uid = unsafe { libc::geteuid() };
    if ![0, uid].contains(&metadata.uid()) || metadata.mode() & 0o022 != 0 {
        return Err(invalid(
            "Podman operator paths must be operator/root-owned and not writable by others",
        ));
    }
    for parent in path.ancestors().skip(1) {
        let directory = std::fs::metadata(parent)?;
        let sticky_root = directory.uid() == 0 && directory.mode() & libc::S_ISVTX != 0;
        if ![0, uid].contains(&directory.uid()) || (directory.mode() & 0o022 != 0 && !sticky_root) {
            return Err(invalid(
                "Podman operator path has an untrusted writable ancestor",
            ));
        }
    }
    Ok(metadata)
}

type FileStamp = (u64, u64, u64, u32, u32, u32, u64, i64, i64, i64, i64);
fn stamp(m: &std::fs::Metadata) -> FileStamp {
    (
        m.dev(),
        m.ino(),
        m.len(),
        m.uid(),
        m.gid(),
        m.mode(),
        m.nlink(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    )
}

fn verified_file(
    path: &Path,
    expected: &str,
    maximum: u64,
    retain: bool,
) -> Result<Vec<u8>, ContainerRuntimeError> {
    if !digest(&expected.to_ascii_lowercase()) {
        return Err(invalid("Podman operator file requires a SHA256 pin"));
    }
    let named = secure_path(path, false)?;
    if named.len() > maximum || named.nlink() != 1 {
        return Err(invalid(
            "Podman operator file exceeds size or hardlink policy",
        ));
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let before = file.metadata()?;
    if stamp(&named) != stamp(&before) {
        return Err(invalid("Podman operator file changed before reading"));
    }
    let mut hash = Sha256::new();
    let mut bytes = Vec::new();
    let mut count = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        let size = file.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        count += size as u64;
        if count > maximum {
            return Err(invalid("Podman operator file grew beyond its bound"));
        }
        hash.update(&buffer[..size]);
        if retain {
            bytes.extend_from_slice(&buffer[..size]);
        }
    }
    let after = file.metadata()?;
    let named_after = secure_path(path, false)?;
    if count != before.len()
        || stamp(&before) != stamp(&after)
        || stamp(&after) != stamp(&named_after)
        || !format!("{:x}", hash.finalize()).eq_ignore_ascii_case(expected)
    {
        return Err(invalid("Podman operator file SHA256 or identity changed"));
    }
    Ok(bytes)
}

/// A short node-local runroot avoids Podman's Unix socket path limit. This
/// directory has no Drop cleanup: pause/conmon helpers require independent
/// stopped-worker and exact directory-identity proof before later deletion.
fn short_runtime(
    state: &Path,
    allowed_cgroup_root: Option<&str>,
) -> Result<PathBuf, ContainerRuntimeError> {
    let base = Path::new("/tmp");
    let base_metadata = std::fs::symlink_metadata(base)?;
    if !base_metadata.is_dir()
        || base.canonicalize()? != base
        || base_metadata.uid() != 0
        || base_metadata.mode() & libc::S_ISVTX == 0
    {
        return Err(invalid(
            "short Podman runtime requires a canonical root-owned sticky /tmp",
        ));
    }
    let mut template = b"/tmp/apm-XXXXXX\0".to_vec();
    // SAFETY: mkdtemp writes only within the live mutable NUL-terminated
    // template and atomically creates its exclusive directory with mode0700.
    let pointer = unsafe { libc::mkdtemp(template.as_mut_ptr().cast()) };
    if pointer.is_null() {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: successful mkdtemp returned this template's terminated buffer.
    let runtime = PathBuf::from(
        unsafe { CStr::from_ptr(pointer) }
            .to_str()
            .map_err(|e| invalid(e.to_string()))?,
    );
    let metadata = secure_path(&runtime, true)?;
    // SAFETY: geteuid only reads the current process credential.
    if metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o777 != 0o700
        || runtime.as_os_str().len() > 40
    {
        return Err(invalid(
            "short Podman runtime ownership/mode/length differs",
        ));
    }
    let record = serde_json::json!({
        "schema":"ORDINARY_PODMAN_RUNTIME_V1", "path":runtime,
        "dev":metadata.dev(), "ino":metadata.ino(), "uid":metadata.uid(), "gid":metadata.gid(), "mode":metadata.mode(),
        "hostname":std::fs::read_to_string("/proc/sys/kernel/hostname")?.trim(),
        "cgroup":std::fs::read_to_string("/proc/self/cgroup")?,
        "allowed_container_cgroup_root":allowed_cgroup_root,
        "job_id":std::env::var("SLURM_JOB_ID").ok(),
        "cleanup":"RETAINED_UNTIL_INDEPENDENT_HELPER_STOP_AND_IDENTITY_PROOF",
    });
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(state.join("podman-runtime.json"))?;
    serde_json::to_writer_pretty(&mut output, &record).map_err(|e| invalid(e.to_string()))?;
    output.sync_all()?;
    for name in ["home", "config", "data", "cache", "tmp"] {
        let directory = runtime.join(name);
        std::fs::DirBuilder::new().mode(0o700).create(&directory)?;
    }
    Ok(runtime)
}

/// Only immutable OCI references occur in this pinned operator-owned catalog.
/// The existing importer checks the held archive bytes and the actual imported
/// manifest/config identity; this record does not replace any of those checks.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageCatalog {
    version: u32,
    images: Vec<ImageCatalogEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageCatalogEntry {
    oci_image: String,
    archive_path: PathBuf,
    archive_sha256: String,
    archive_bytes: u64,
    image_config_sha256: String,
}

fn import_request(
    catalog: &Path,
    catalog_sha256: &str,
    image: &str,
    budget_ms: u64,
) -> Result<OciImageImportRequest, ContainerRuntimeError> {
    if !immutable_image(image) {
        return Err(invalid(
            "ordinary Podman plugin requires an immutable OCI digest",
        ));
    }
    let bytes = verified_file(catalog, catalog_sha256, 1024 * 1024, true)?;
    let catalog: ImageCatalog =
        serde_json::from_slice(&bytes).map_err(|e| invalid(e.to_string()))?;
    if catalog.version != 1 || catalog.images.is_empty() || catalog.images.len() > 1024 {
        return Err(invalid("unsupported or unbounded Podman image catalog"));
    }
    let mut seen = BTreeSet::new();
    let mut selected = None;
    for entry in catalog.images {
        if !immutable_image(&entry.oci_image)
            || !seen.insert(entry.oci_image.clone())
            || !entry.archive_path.is_absolute()
            || !digest(&entry.archive_sha256)
            || !digest(&entry.image_config_sha256)
            || entry.archive_bytes == 0
            || entry.archive_bytes > 64 * 1024 * 1024 * 1024
        {
            return Err(invalid("invalid or duplicate Podman image catalog entry"));
        }
        if entry.oci_image == image {
            selected = Some(entry);
        }
    }
    let entry = selected
        .ok_or_else(|| invalid("Podman image digest is absent from the pinned operator catalog"))?;
    let timeout_secs = (budget_ms / 1000).min(3600);
    if timeout_secs == 0 {
        return Err(invalid(
            "Podman image import needs an explicit remaining task budget of at least one second",
        ));
    }
    Ok(OciImageImportRequest {
        archive: entry.archive_path,
        archive_sha256: entry.archive_sha256,
        archive_bytes: entry.archive_bytes,
        image_reference: image.to_string(),
        image_config_sha256: entry.image_config_sha256,
        timeout_secs,
    })
}

fn check_worker_controllers(
    required: &[&str],
    unified: bool,
    available: &str,
) -> Result<(), ContainerRuntimeError> {
    if required.is_empty() {
        return Ok(());
    }
    if !unified {
        return Err(invalid(
            "ordinary rootless Podman CPU/memory/PID limits require cgroup v2; v1 limits may be ignored",
        ));
    }
    let controllers = available.split_whitespace().collect::<BTreeSet<_>>();
    if required
        .iter()
        .any(|controller| !controllers.contains(controller))
    {
        return Err(invalid(
            "ordinary rootless Podman lacks requested cgroup controllers",
        ));
    }
    Ok(())
}

/// Resource flags alone do not prove enforcement: rootless Podman may ignore
/// them on v1. Keep this gate exclusive to the explicit ordinary worker. V2
/// admission still requires a later synthetic container readback qualification.
pub(crate) fn validate_worker_resources(
    config: &PodmanConfig,
    request: &crate::ContainerRunRequest,
) -> Result<(), ContainerRuntimeError> {
    validate_worker_container_name(&request.name)?;
    verify_oci_runtime(config)?;
    if let Some(PodmanWorkerResourceControl::SlurmAllocation { swap_policy }) =
        config.worker_resource_control
    {
        if config.worker_oci_runtime.is_none() {
            return Err(invalid(
                "Slurm allocation requires an explicitly pinned OCI runtime supporting NoCgroups",
            ));
        }
        if config.worker_runtime_dir.is_none()
            || config.cgroup_parent.is_some()
            || config.inherit_user_namespace
            || config.disable_swap
        {
            return Err(invalid(
                "Slurm allocation mode requires a private ordinary backend without a Podman cgroup parent or formal policy",
            ));
        }
        let evidence = crate::slurm_allocation::observe_and_validate(request, swap_policy)?;
        let receipt = serde_json::json!({
            "schema":"SLURM_ALLOCATION_RUN_ADMISSION_V1", "resource_control":"slurm_allocation",
            "swap_policy":swap_policy, "evidence":evidence,
            "request":{"name":request.name,"cpus":request.cpus,"memory":request.memory,"pids_limit":request.pids_limit,"timeout_secs":request.timeout_secs},
            "user_namespace":match config.worker_user_namespace {Some(PodmanWorkerUserNamespace::SingleUidRoot)=>"single_uid_root",_=>"keep_id"},
            "container_execution_proven":false,
        });
        write_worker_evidence(
            &config
                .workspace_root
                .join(format!("{}.slurm-allocation.json", request.name)),
            &receipt,
        )?;
        return Ok(());
    }
    let mut required = Vec::new();
    if request.cpus.is_some() {
        required.push("cpu");
    }
    if request.memory.is_some() {
        required.push("memory");
    }
    if request.pids_limit.is_some() {
        required.push("pids");
    }
    validate_worker_controller_requirements(config, &required)
}

/// Enumerate only the current worker's ancestors inside its exact Slurm job.
/// Neither a similarly prefixed job nor an ancestor above job_<id> qualifies.
fn slurm_parent_candidates(
    current: &str,
    job: &str,
) -> Result<Vec<PathBuf>, ContainerRuntimeError> {
    if job.is_empty() || !job.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid("ordinary Podman needs a numeric SLURM_JOB_ID"));
    }
    let path = Path::new(current);
    if !path.is_absolute()
        || current.split('/').any(|part| matches!(part, "." | ".."))
        || current.contains("//")
        || (current.len() > 1 && current.ends_with('/'))
    {
        return Err(invalid("invalid unified worker cgroup path"));
    }
    let anchor = format!("job_{job}");
    let mut job_root = None;
    let mut prefix = PathBuf::from("/");
    for component in path.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        let name = name
            .to_str()
            .ok_or_else(|| invalid("worker cgroup must be UTF-8"))?;
        prefix.push(name);
        if let Some(number) = name
            .strip_prefix("job_")
            .filter(|number| !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
        {
            if number != job || job_root.is_some() {
                return Err(invalid("ambiguous or different Slurm job cgroup"));
            }
            job_root = Some(prefix.clone());
        }
    }
    let root =
        job_root.ok_or_else(|| invalid(format!("worker cgroup has no exact {anchor} anchor")))?;
    Ok(path
        .ancestors()
        .take_while(|candidate| candidate.starts_with(&root))
        .map(Path::to_path_buf)
        .collect())
}

fn unified_worker_path(cgroup: &str) -> Result<&str, ContainerRuntimeError> {
    let lines = cgroup
        .lines()
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if lines.len() != 1 {
        return Err(invalid(
            "ordinary Podman requires a unified cgroup v2 worker",
        ));
    }
    lines[0].strip_prefix("0::").ok_or_else(|| invalid("ordinary rootless Podman CPU/memory/PID limits require cgroup v2; v1 limits may be ignored"))
}

fn worker_parent_in_job(
    current: &str,
    job: &str,
    parent: &str,
) -> Result<(), ContainerRuntimeError> {
    // Check the parent spelling too: Path equality normalizes internal '.'
    // and repeated separators, but no aliased admission root is allowed.
    slurm_parent_candidates(parent, job)?;
    if !slurm_parent_candidates(current, job)?
        .iter()
        .any(|candidate| candidate == Path::new(parent))
    {
        return Err(invalid(
            "ordinary Podman parent must be a worker ancestor within its exact Slurm job",
        ));
    }
    Ok(())
}

fn delegated_parent_at(
    mount: &Path,
    parent: &Path,
    required: &[&str],
) -> Result<(), ContainerRuntimeError> {
    let directory = mount.join(
        parent
            .strip_prefix("/")
            .map_err(|e| invalid(e.to_string()))?,
    );
    if directory.canonicalize()? != directory || !std::fs::symlink_metadata(&directory)?.is_dir() {
        return Err(invalid(
            "delegated Podman cgroup parent must be a canonical directory",
        ));
    }
    let controllers = std::fs::read_to_string(directory.join("cgroup.controllers"))?;
    let enabled = std::fs::read_to_string(directory.join("cgroup.subtree_control"))?;
    check_worker_controllers(required, true, &controllers)?;
    check_worker_controllers(required, true, &enabled)?;
    // Domain controllers cannot distribute resources while this cgroup has
    // internal processes. The worker stays in its existing descendant leaf.
    if !std::fs::read_to_string(directory.join("cgroup.procs"))?
        .trim()
        .is_empty()
        || std::fs::read_to_string(directory.join("cgroup.type"))?.trim() != "domain"
    {
        return Err(invalid(
            "delegated Podman parent must be an empty domain cgroup",
        ));
    }
    for (path, permissions) in [
        (directory.clone(), libc::W_OK | libc::X_OK),
        (directory.join("cgroup.procs"), libc::W_OK),
        (directory.join("cgroup.subtree_control"), libc::W_OK),
    ] {
        let path = std::ffi::CString::new(
            path.to_str()
                .ok_or_else(|| invalid("worker cgroup must be UTF-8"))?,
        )
        .map_err(|e| invalid(e.to_string()))?;
        // SAFETY: faccessat with AT_EACCESS queries effective-credential
        // permissions only. It creates/moves no process and writes no limit.
        if unsafe { libc::faccessat(libc::AT_FDCWD, path.as_ptr(), permissions, libc::AT_EACCESS) }
            != 0
        {
            return Err(invalid(
                "ordinary Podman cgroup parent is not delegated for child creation/migration",
            ));
        }
    }
    Ok(())
}

fn choose_worker_parent(
    current: &str,
    job: &str,
    mut admissible: impl FnMut(&Path) -> bool,
) -> Result<String, ContainerRuntimeError> {
    slurm_parent_candidates(current, job)?
        .into_iter()
        .find(|candidate| admissible(candidate))
        .map(|parent| parent.to_string_lossy().into_owned())
        .ok_or_else(|| {
            invalid("no enabled, empty, writable delegated Podman parent inside this Slurm job")
        })
}

fn select_worker_parent(required: &[&str]) -> Result<String, ContainerRuntimeError> {
    let cgroup = std::fs::read_to_string("/proc/self/cgroup")?;
    let current = unified_worker_path(&cgroup)?;
    let job = std::env::var("SLURM_JOB_ID")
        .map_err(|_| invalid("ordinary Podman requires an explicit Slurm job anchor"))?;
    choose_worker_parent(current, &job, |candidate| {
        delegated_parent_at(Path::new("/sys/fs/cgroup"), candidate, required).is_ok()
    })
}

fn validate_worker_controller_requirements(
    config: &PodmanConfig,
    required: &[&str],
) -> Result<(), ContainerRuntimeError> {
    let cgroup = std::fs::read_to_string("/proc/self/cgroup")?;
    let current = unified_worker_path(&cgroup)?;
    let job = std::env::var("SLURM_JOB_ID")
        .map_err(|_| invalid("ordinary Podman requires an explicit Slurm job anchor"))?;
    let parent = config
        .cgroup_parent
        .as_deref()
        .ok_or_else(|| invalid("ordinary Podman has no admitted delegated parent"))?;
    worker_parent_in_job(current, &job, parent)?;
    delegated_parent_at(Path::new("/sys/fs/cgroup"), Path::new(parent), required)
}

fn helper_cgroup_root(
    config: &PodmanConfig,
    worker: &str,
) -> Result<String, ContainerRuntimeError> {
    let Some(parent) = config.cgroup_parent.as_deref() else {
        return Ok(worker.to_owned());
    };
    let current = unified_worker_path(worker)?;
    let job = std::env::var("SLURM_JOB_ID")
        .map_err(|_| invalid("missing Slurm helper-confinement anchor"))?;
    worker_parent_in_job(current, &job, parent)?;
    Ok(format!("0::{parent}\n"))
}

fn cgroup_descendant(baseline: &str, observed: &str) -> bool {
    let parse = |text: &str| {
        text.lines()
            .filter_map(|line| {
                let fields = line.splitn(3, ':').collect::<Vec<_>>();
                (fields.len() == 3).then(|| (fields[1].to_owned(), fields[2].to_owned()))
            })
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    let parents = parse(baseline);
    let actual = parse(observed);
    !parents.is_empty()
        && parents.len() == actual.len()
        && parents.iter().all(|(controller, parent)| {
            actual.get(controller).is_some_and(|path| {
                path == parent || path.starts_with(&format!("{}/", parent.trim_end_matches('/')))
            })
        })
}

impl ContainerExecutionInfra {
    /// Observe the known pause pidfile without killing a process or deleting
    /// runtime state. A missing pidfile is not a global helper-absence proof.
    /// Any observed helper must remain under the admitted job-scoped parent
    /// (or native worker baseline) in every controller hierarchy. External Slurm stop proof is still required.
    pub fn record_podman_worker_helpers(
        &self,
        state: &Path,
        phase: &str,
    ) -> Result<(), ContainerRuntimeError> {
        let Some(runtime) = &self.config.worker_runtime_dir else {
            return Ok(());
        };
        if !matches!(phase, "after-import" | "after-execution") {
            return Err(invalid("invalid helper observation phase"));
        }
        let cgroup = std::fs::read_to_string("/proc/self/cgroup")?;
        let allowed_cgroup = helper_cgroup_root(&self.config, &cgroup)?;
        let path = runtime.join("libpod/tmp/pause.pid");
        let mut pause_paths = vec![path.clone()];
        if let Some(runroot) = &self.config.run_root {
            let candidate = runroot.join("libpod/tmp/pause.pid");
            if candidate != path {
                pause_paths.push(candidate);
            }
        }
        let mut rows = Vec::new();
        let mut confined = true;
        for observed_path in &pause_paths {
            match std::fs::symlink_metadata(observed_path) {
                Ok(metadata) => {
                    // SAFETY: geteuid only reads the current process credential.
                    if !metadata.is_file()
                        || metadata.uid() != unsafe { libc::geteuid() }
                        || metadata.len() > 128
                    {
                        return Err(invalid(
                            "ordinary Podman pause pidfile ownership/type differs; retain runtime",
                        ));
                    }
                    let file = OpenOptions::new()
                        .read(true)
                        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                        .open(observed_path)?;
                    let mut text = String::new();
                    file.take(129).read_to_string(&mut text)?;
                    let pid: u32 = text
                        .trim()
                        .parse()
                        .map_err(|_| invalid("invalid Podman pause PID; retain runtime"))?;
                    if pid == 0 {
                        return Err(invalid("invalid zero pause PID"));
                    }
                    let proc = PathBuf::from(format!("/proc/{pid}"));
                    match std::fs::read_to_string(proc.join("cgroup")) {
                        Ok(actual) => {
                            let observed_confined = cgroup_descendant(&allowed_cgroup, &actual);
                            confined &= observed_confined;
                            rows.push(serde_json::json!({"pid":pid,"pidfile":observed_path,"cgroup":actual,"comm":std::fs::read_to_string(proc.join("comm"))?.trim(),"confined":observed_confined}));
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => rows
                            .push(serde_json::json!({"pid":pid,"not_running_at_observation":true})),
                        Err(error) => return Err(error.into()),
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
        }
        let record = serde_json::json!({"schema":"ORDINARY_PODMAN_HELPER_OBSERVATION_V1","phase":phase,"worker_cgroup":cgroup,"allowed_cgroup_root":allowed_cgroup,"known_pause_pidfile":path,"known_pause_pidfiles":pause_paths,"observed":rows,"all_observed_helpers_confined":confined,"global_helper_absence_proven":false,"runtime_retained":true,"cleanup":"REQUIRES_INDEPENDENT_HELPER_STOP_AND_IDENTITY_PROOF"});
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(state.join(format!("podman-helpers-{phase}.json")))?;
        serde_json::to_writer_pretty(&mut output, &record).map_err(|e| invalid(e.to_string()))?;
        output.sync_all()?;
        if !confined {
            return Err(invalid(
                "Podman pause helper escaped the worker cgroup; retain all runtime state",
            ));
        }
        Ok(())
    }

    /// Ordinary typed worker only. The executable is byte-pinned and the
    /// backend is private VFS/cgroupfs. Native requests with no plugin image
    /// perform no Podman command. A plugin image requires a pinned catalog and
    /// is loaded through the existing strict offline importer, never pulled.
    pub async fn for_podman_worker(
        state: &Path,
        executable: &Path,
        executable_sha256: &str,
        image_catalog: Option<(&Path, &str)>,
        panel_cache_root: Option<&Path>,
        image: Option<&str>,
        remaining_budget_ms: Option<u64>,
    ) -> Result<Self, ContainerRuntimeError> {
        Self::for_podman_worker_with_policy(
            state,
            executable,
            executable_sha256,
            image_catalog,
            panel_cache_root,
            image,
            remaining_budget_ms,
            PodmanWorkerResourceControl::DelegatedCgroups,
            PodmanWorkerUserNamespace::KeepId,
            None,
        )
        .await
    }

    pub async fn for_podman_worker_with_policy(
        state: &Path,
        executable: &Path,
        executable_sha256: &str,
        image_catalog: Option<(&Path, &str)>,
        panel_cache_root: Option<&Path>,
        image: Option<&str>,
        remaining_budget_ms: Option<u64>,
        resource_control: PodmanWorkerResourceControl,
        user_namespace: PodmanWorkerUserNamespace,
        oci_runtime: Option<(&Path, &str)>,
    ) -> Result<Self, ContainerRuntimeError> {
        let preparation_started = std::time::Instant::now();
        crate::ensure_backend_env_removed().map_err(invalid)?;
        let state_metadata = secure_path(state, true)?;
        // SAFETY: geteuid only reads the current process credential.
        if state_metadata.uid() != unsafe { libc::geteuid() } || state_metadata.mode() & 0o077 != 0
        {
            return Err(invalid(
                "ordinary Podman state must be operator-owned and private",
            ));
        }
        let metadata = secure_path(executable, false)?;
        if metadata.mode() & 0o111 == 0 {
            return Err(invalid("pinned Podman program is not executable"));
        }
        verified_file(executable, executable_sha256, 512 * 1024 * 1024, false)?;
        let mut config =
            PodmanConfig::for_state_dir_with_storage_driver(state, PrivateStorageDriver::Vfs);
        config.program = executable
            .to_str()
            .ok_or_else(|| invalid("Podman executable path must be UTF-8"))?
            .into();
        config.cgroup_manager = Some("cgroupfs".into());
        config.worker_resource_control = Some(resource_control);
        config.worker_user_namespace = Some(user_namespace);
        config.worker_oci_runtime =
            oci_runtime.map(|(path, hash)| (path.to_path_buf(), hash.to_string()));
        verify_oci_runtime(&config)?;
        if image.is_some()
            && matches!(
                resource_control,
                PodmanWorkerResourceControl::SlurmAllocation { .. }
            )
            && oci_runtime.is_none()
        {
            return Err(invalid(
                "Slurm allocation requires an explicitly pinned OCI runtime supporting NoCgroups",
            ));
        }
        if image.is_some() {
            if image_catalog.is_none() {
                return Err(invalid(
                    "ordinary Podman container plugin requires a pinned offline image catalog",
                ));
            }
            // Refuse unsupported rootless hosts before touching a large
            // archive or starting Podman's persistent pause process.
            match resource_control {
                PodmanWorkerResourceControl::DelegatedCgroups => {
                    config.cgroup_parent = Some(select_worker_parent(&["cpu", "memory", "pids"])?);
                    validate_worker_controller_requirements(&config, &["cpu", "memory", "pids"])?;
                }
                PodmanWorkerResourceControl::SlurmAllocation { swap_policy } => {
                    let observation = crate::slurm_allocation::observe()?;
                    write_worker_evidence(
                        &state.join("slurm-allocation-preparation.json"),
                        &serde_json::json!({
                            "schema":"SLURM_ALLOCATION_PREPARATION_V1", "swap_policy":swap_policy,
                            "evidence":observation, "request_admitted":false, "container_execution_proven":false,
                        }),
                    )?;
                }
            }
        }
        let runtime_dir = short_runtime(state, config.cgroup_parent.as_deref())?;
        config.run_root = Some(runtime_dir.clone());
        config.worker_runtime_dir = Some(runtime_dir.clone());
        if matches!(
            resource_control,
            PodmanWorkerResourceControl::SlurmAllocation { .. }
        ) {
            // Rootless layer unpacking on the site's shared GPFS fails before
            // a container starts. Keep only this disposable backend node-local;
            // input/output workspaces and pinned archives stay on shared data.
            // Separate siblings preserve the OCI importer's backend lock policy.
            config.storage_root = Some(runtime_dir.join("storage"));
            config.run_root = Some(runtime_dir.join("run"));
        }
        if let Some(root) = panel_cache_root {
            secure_path(root, true)?;
            config.panel_cache_root = root.to_path_buf();
        }
        // The importer creates only the leaf roots, so prepare their fresh,
        // private parent without consulting a host/default image store.
        for path in [
            &config.workspace_root,
            config.storage_root.as_ref().expect("private storage"),
            config.run_root.as_ref().expect("private runroot"),
        ] {
            std::fs::create_dir_all(path)?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
        }
        if matches!(
            resource_control,
            PodmanWorkerResourceControl::SlurmAllocation { .. }
        ) {
            let mut roots = Vec::new();
            for path in [
                config.storage_root.as_ref().expect("private storage"),
                config.run_root.as_ref().expect("private runroot"),
            ] {
                let metadata = secure_path(path, true)?;
                roots.push(serde_json::json!({"path":path,"dev":metadata.dev(),"ino":metadata.ino(),"uid":metadata.uid(),"gid":metadata.gid(),"mode":metadata.mode()}));
            }
            write_worker_evidence(
                &state.join("podman-local-backend.json"),
                &serde_json::json!({
                    "schema":"ORDINARY_PODMAN_SLURM_LOCAL_BACKEND_V1", "runtime":runtime_dir,
                    "storage":roots[0], "runroot":roots[1], "workspace":config.workspace_root,
                    "cleanup":"RETAINED_UNTIL_INDEPENDENT_HELPER_STOP_AND_IDENTITY_PROOF",
                }),
            )?;
        }
        configure_oci_runtime(&config, state)?;
        let runtime = PodmanRuntime::new(config.clone());
        if let Some(image) = image {
            if user_namespace == PodmanWorkerUserNamespace::SingleUidRoot {
                let mapping =
                    verify_single_uid_namespace(&config, remaining_budget_ms.unwrap_or(0)).await?;
                write_worker_evidence(&state.join("podman-single-uid-preparation.json"), &mapping)?;
            }
            let (catalog, hash) = image_catalog.ok_or_else(|| {
                invalid("ordinary Podman container plugin requires a pinned offline image catalog")
            })?;
            let remaining = remaining_budget_ms
                .ok_or_else(|| {
                    invalid("ordinary Podman image import requires an explicit task budget")
                })?
                .saturating_sub(
                    preparation_started
                        .elapsed()
                        .as_millis()
                        .min(u64::MAX as u128) as u64,
                );
            let mut request = import_request(catalog, hash, image, remaining)?;
            let remaining = remaining_budget_ms
                .expect("explicit budget checked above")
                .saturating_sub(
                    preparation_started
                        .elapsed()
                        .as_millis()
                        .min(u64::MAX as u128) as u64,
                );
            request.timeout_secs = request.timeout_secs.min(remaining / 1000);
            if request.timeout_secs == 0 {
                return Err(invalid(
                    "ordinary Podman preparation exhausted the task budget",
                ));
            }
            let receipt = runtime.import_oci_archive(request).await?;
            let destination = state.join("podman-image-import.json");
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(destination)?;
            serde_json::to_writer_pretty(&mut output, &receipt)
                .map_err(|e| invalid(e.to_string()))?;
            output.sync_all()?;
        }
        Ok(Self {
            runtime: Arc::new(runtime),
            panel_cache: Arc::new(PanelCache::content_required(
                config.panel_cache_root.clone(),
            )),
            config,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_uid_mapping_requires_exact_operator_identity_without_extra_ranges() {
        let result = single_uid_mapping(b"0 89274 1\n0 89275 1\n", 89274, 89275).unwrap();
        assert_eq!(result["ownership_squashing"], false);
        for bytes in [
            &b"0 89274 65536\n0 89275 1\n"[..],
            &b"0 89273 1\n0 89275 1\n"[..],
            &b"0 89274 1\n1 100000 65536\n0 89275 1\n"[..],
            &b"0 89274 1\n0 89275 1\nextra\n"[..],
        ] {
            assert!(single_uid_mapping(bytes, 89274, 89275).is_err());
        }
        assert!(single_uid_mapping(b"0 0 1\n0 0 1\n", 0, 0).is_err());
    }

    #[test]
    fn evidence_container_names_cannot_escape_the_workspace() {
        for name in [
            "",
            "../escaped",
            ".",
            "..",
            "-flag",
            "dir/name",
            "a\nb",
            "a b",
        ] {
            assert!(validate_worker_container_name(name).is_err(), "{name:?}");
        }
        validate_worker_container_name("typed-1234_run.1").unwrap();
        assert!(validate_worker_container_name(&"a".repeat(129)).is_err());
    }

    struct ToyRuntimeCleanup(PathBuf);
    impl Drop for ToyRuntimeCleanup {
        fn drop(&mut self) {
            let record = self.0.join("podman-runtime.json");
            if let Ok(bytes) = std::fs::read(record) {
                let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                // All commands in these tests are the local waited fixture,
                // which creates no pause/conmon/container or background child.
                let runtime = PathBuf::from(value["path"].as_str().unwrap());
                let metadata = std::fs::symlink_metadata(&runtime).unwrap();
                assert_eq!(metadata.dev(), value["dev"].as_u64().unwrap());
                assert_eq!(metadata.ino(), value["ino"].as_u64().unwrap());
                std::fs::remove_dir_all(runtime).unwrap();
            }
        }
    }

    fn program(root: &Path) -> (PathBuf, String) {
        let path = root.join("fake-podman");
        std::fs::write(&path, b"#!/bin/sh\nexit 99\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let hash = format!("{:x}", Sha256::digest(std::fs::read(&path).unwrap()));
        (path, hash)
    }
    fn catalog(root: &Path, images: serde_json::Value) -> (PathBuf, String) {
        let path = root.join("catalog.json");
        let bytes = serde_json::to_vec(&serde_json::json!({"version":1,"images":images})).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        (path, format!("{:x}", Sha256::digest(&bytes)))
    }
    fn entry(image: &str, archive: &Path) -> serde_json::Value {
        serde_json::json!({"oci_image":image,"archive_path":archive,"archive_sha256":"a".repeat(64),"archive_bytes":17,"image_config_sha256":"b".repeat(64)})
    }

    #[tokio::test]
    async fn native_backend_uses_only_the_explicit_byte_pinned_program_and_private_vfs() {
        let temp = tempfile::tempdir().unwrap();
        let _cleanup = ToyRuntimeCleanup(temp.path().to_path_buf());
        let (program, hash) = program(temp.path());
        // A spawned fixture exits99: successful construction proves native
        // setup does not invoke Podman or inspect an ambient image inventory.
        let infra = ContainerExecutionInfra::for_podman_worker(
            temp.path(),
            &program,
            &hash,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(infra.config.program, program.to_str().unwrap());
        assert_eq!(infra.runtime.name(), "podman");
        assert_eq!(infra.config.storage_driver.as_deref(), Some("vfs"));
        assert_eq!(infra.config.cgroup_manager.as_deref(), Some("cgroupfs"));
        assert!(!infra.config.inherit_user_namespace);
        assert_eq!(
            infra.config.storage_root,
            Some(temp.path().join("containers/storage"))
        );
        let run = infra.config.run_root.as_ref().unwrap();
        assert!(run.starts_with("/tmp/apm-") || run.to_string_lossy().starts_with("/tmp/apm-"));
        assert!(run.as_os_str().len() < 40);
        assert_eq!(infra.config.worker_runtime_dir.as_ref(), Some(run));
        assert!(infra.panel_cache.requires_content_verification());
    }

    #[tokio::test]
    async fn slurm_backend_keeps_graphroot_and_runroot_as_private_local_siblings() {
        let temp = tempfile::tempdir().unwrap();
        let _cleanup = ToyRuntimeCleanup(temp.path().to_path_buf());
        let (program, hash) = program(temp.path());
        let infra = ContainerExecutionInfra::for_podman_worker_with_policy(
            temp.path(),
            &program,
            &hash,
            None,
            None,
            None,
            None,
            PodmanWorkerResourceControl::SlurmAllocation {
                swap_policy: crate::SlurmSwapPolicy::Inherit,
            },
            PodmanWorkerUserNamespace::SingleUidRoot,
            Some((&program, &hash)),
        )
        .await
        .unwrap();
        // A native preparation does not invoke the fixture program, create a
        // container, or assert that the host has an admitted Slurm allocation.
        let base = infra.config.worker_runtime_dir.as_ref().unwrap();
        let storage = infra.config.storage_root.as_ref().unwrap();
        let runroot = infra.config.run_root.as_ref().unwrap();
        assert_eq!(storage, &base.join("storage"));
        assert_eq!(runroot, &base.join("run"));
        assert!(!storage.starts_with(runroot) && !runroot.starts_with(storage));
        assert_eq!(
            infra.config.workspace_root,
            temp.path().join("containers/workspace")
        );
        infra
            .record_podman_worker_helpers(temp.path(), "after-import")
            .unwrap();
        let observation: serde_json::Value = serde_json::from_slice(
            &std::fs::read(temp.path().join("podman-helpers-after-import.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            observation["known_pause_pidfile"],
            base.join("libpod/tmp/pause.pid").to_string_lossy().as_ref()
        );
        assert_eq!(
            observation["known_pause_pidfiles"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let identity: serde_json::Value = serde_json::from_slice(
            &std::fs::read(temp.path().join("podman-local-backend.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            identity["storage"]["ino"],
            std::fs::metadata(storage).unwrap().ino()
        );
        assert_eq!(
            identity["runroot"]["ino"],
            std::fs::metadata(runroot).unwrap().ino()
        );
        let selected: serde_json::Value = serde_json::from_slice(
            &std::fs::read(temp.path().join("podman-oci-runtime.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(selected["path"], program.to_string_lossy().as_ref());
        assert_eq!(selected["sha256"], hash);
        assert_eq!(selected["container_execution_proven"], false);
        let args = infra
            .config
            .blocking_command()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let flag = args.iter().position(|arg| arg == "--runtime").unwrap();
        assert_eq!(args[flag + 1], "autonomics-crun");
        let private_config =
            std::fs::read_to_string(base.join("config/containers/containers.conf")).unwrap();
        assert!(private_config.contains("runtime_supports_nocgroup=[\"autonomics-crun\"]"));
        assert!(
            private_config.contains(&serde_json::to_string(program.to_str().unwrap()).unwrap())
        );
        verify_oci_runtime(&infra.config).unwrap();
        std::fs::write(&program, b"#!/bin/sh\nexit 98\n").unwrap();
        assert!(
            verify_oci_runtime(&infra.config).is_err(),
            "changed OCI executable must fail before any container call"
        );
    }

    #[tokio::test]
    async fn changed_unpinned_aliased_or_writable_executable_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let _cleanup = ToyRuntimeCleanup(temp.path().to_path_buf());
        let (program, hash) = program(temp.path());
        assert!(
            ContainerExecutionInfra::for_podman_worker(
                temp.path(),
                &program,
                &"c".repeat(64),
                None,
                None,
                None,
                None
            )
            .await
            .is_err()
        );
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(
            ContainerExecutionInfra::for_podman_worker(
                temp.path(),
                &program,
                &hash,
                None,
                None,
                None,
                None
            )
            .await
            .is_err()
        );
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(&program, &alias).unwrap();
        assert!(
            ContainerExecutionInfra::for_podman_worker(
                temp.path(),
                &alias,
                &hash,
                None,
                None,
                None,
                None
            )
            .await
            .is_err()
        );
        std::fs::write(&program, b"#!/bin/sh\nexit 0\n").unwrap();
        assert!(
            ContainerExecutionInfra::for_podman_worker(
                temp.path(),
                &program,
                &hash,
                None,
                None,
                None,
                None
            )
            .await
            .is_err()
        );
    }

    #[test]
    fn slurm_parent_selection_stays_inside_the_exact_numeric_job() {
        let current = "/slurm/uid_1000/job_42/step_batch/worker-native";
        let candidates = slurm_parent_candidates(current, "42").unwrap();
        assert_eq!(
            candidates,
            [
                PathBuf::from(current),
                PathBuf::from("/slurm/uid_1000/job_42/step_batch"),
                PathBuf::from("/slurm/uid_1000/job_42"),
            ]
        );
        let chosen = choose_worker_parent(current, "42", |candidate| {
            candidate == Path::new("/slurm/uid_1000/job_42/step_batch")
        })
        .unwrap();
        assert_eq!(chosen, "/slurm/uid_1000/job_42/step_batch");
        assert!(
            choose_worker_parent(current, "42", |candidate| candidate
                == Path::new("/slurm/uid_1000"))
            .is_err()
        );
        assert!(choose_worker_parent(current, "42", |_| false).is_err());
        for (path, job) in [
            (current, ""),
            (current, "4x"),
            (current, "420"),
            ("/slurm/job_420/worker", "42"),
            ("/slurm/job_42/job_43/worker", "42"),
            ("/slurm/job_42/job_42/worker", "42"),
            ("/slurm/job_42/../job_43", "42"),
            ("/slurm//job_42/worker", "42"),
            ("/slurm/no-job/worker", "42"),
            ("relative/job_42/worker", "42"),
        ] {
            assert!(
                slurm_parent_candidates(path, job).is_err(),
                "{path} / {job}"
            );
        }
        for parent in [
            "/",
            "/slurm",
            "/slurm/uid_1000",
            "/slurm/uid_1000/job_43",
            "/slurm/uid_1000/job_42/step_other",
            "/slurm/uid_1000/job_42/./step_batch",
            "/slurm/uid_1000/job_42//step_batch",
        ] {
            assert!(
                worker_parent_in_job(current, "42", parent).is_err(),
                "{parent}"
            );
        }
        worker_parent_in_job(current, "42", &chosen).unwrap();
        assert!(unified_worker_path("2:cpu:/slurm/job_42\n").is_err());
        assert!(unified_worker_path("0::/slurm/job_42\n2:cpu:/other\n").is_err());
    }

    fn toy_cgroup(
        mount: &Path,
        path: &str,
        controllers: &str,
        enabled: &str,
        processes: &str,
    ) -> PathBuf {
        let directory = mount.join(path.trim_start_matches('/'));
        std::fs::create_dir_all(&directory).unwrap();
        for (name, content) in [
            ("cgroup.controllers", controllers),
            ("cgroup.subtree_control", enabled),
            ("cgroup.procs", processes),
            ("cgroup.type", "domain\n"),
        ] {
            std::fs::write(directory.join(name), content).unwrap();
        }
        directory
    }

    #[test]
    fn delegated_parent_requires_empty_enabled_writable_domain_within_job() {
        // Ordinary files model the controller admission protocol only; this
        // never creates a real cgroup or claims host resource qualification.
        let temp = tempfile::tempdir().unwrap();
        let current = "/slurm/job_42/delegated/worker-native";
        let parent = "/slurm/job_42/delegated";
        toy_cgroup(temp.path(), current, "cpu memory pids", "", "123\n");
        let delegated = toy_cgroup(
            temp.path(),
            parent,
            "cpu memory pids",
            "cpu memory pids",
            "",
        );
        toy_cgroup(temp.path(), "/slurm/job_42", "cpu memory pids", "", "");
        toy_cgroup(
            temp.path(),
            "/slurm",
            "cpu memory pids",
            "cpu memory pids",
            "",
        );
        let required = ["cpu", "memory", "pids"];
        let select = || {
            choose_worker_parent(current, "42", |candidate| {
                delegated_parent_at(temp.path(), candidate, &required).is_ok()
            })
        };
        assert_eq!(
            select().unwrap(),
            parent,
            "populated worker leaf must not be used"
        );
        std::fs::write(delegated.join("cgroup.subtree_control"), "cpu memory").unwrap();
        assert!(
            select().is_err(),
            "must not escape above job to an enabled parent"
        );
        std::fs::write(delegated.join("cgroup.subtree_control"), "cpu memory pids").unwrap();
        std::fs::write(delegated.join("cgroup.controllers"), "").unwrap();
        assert!(select().is_err());
        std::fs::write(delegated.join("cgroup.controllers"), "cpu memory pids").unwrap();
        std::fs::write(delegated.join("cgroup.procs"), "555\n").unwrap();
        assert!(
            select().is_err(),
            "domain controller parent must have no internal processes"
        );
        std::fs::write(delegated.join("cgroup.procs"), "").unwrap();
        std::fs::write(delegated.join("cgroup.type"), "threaded\n").unwrap();
        assert!(select().is_err());
        std::fs::write(delegated.join("cgroup.type"), "domain\n").unwrap();
        // SAFETY: geteuid only queries the current credential. Root DAC
        // naturally grants writes; unprivileged effective permissions must
        // reject the read-only cgroup.procs fixture, with no mutation probe.
        if unsafe { libc::geteuid() } != 0 {
            let file = delegated.join("cgroup.procs");
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o400)).unwrap();
            let rejected = select().is_err();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
            assert!(rejected);
        }
        assert_eq!(select().unwrap(), parent);
    }

    #[test]
    fn admitted_parent_allows_same_job_oci_siblings_but_no_job_escape() {
        let allowed = "0::/slurm/job_42/delegated\n";
        let worker = "0::/slurm/job_42/delegated/worker-native\n";
        let container = "0::/slurm/job_42/delegated/libpod-conmon/container\n";
        assert!(cgroup_descendant(allowed, worker));
        assert!(cgroup_descendant(allowed, container));
        assert!(!cgroup_descendant(worker, container));
        assert!(!cgroup_descendant(
            allowed,
            "0::/slurm/job_43/delegated/worker\n"
        ));
        assert!(!cgroup_descendant(
            allowed,
            "0::/slurm/job_42/other-delegation/worker\n"
        ));
        assert!(!cgroup_descendant(allowed, "0::/slurm\n"));
    }

    #[test]
    fn helper_confinement_requires_every_controller_to_remain_in_the_worker_subtree() {
        let worker = "2:cpu:/slurm/job_1/step_batch\n1:pids:/system.slice/slurmd.service\n";
        assert!(cgroup_descendant(worker, worker));
        assert!(cgroup_descendant(
            worker,
            "2:cpu:/slurm/job_1/step_batch/container\n1:pids:/system.slice/slurmd.service\n"
        ));
        assert!(!cgroup_descendant(
            worker,
            "2:cpu:/user.slice/user-1.slice\n1:pids:/system.slice/slurmd.service\n"
        ));
        assert!(!cgroup_descendant(
            worker,
            "2:cpu:/slurm/job_1/step_batch\n"
        ));
    }

    #[test]
    fn ordinary_rootless_resource_gate_refuses_v1_and_missing_controllers() {
        assert!(check_worker_controllers(&["cpu"], false, "cpu memory pids").is_err());
        assert!(check_worker_controllers(&["memory", "pids"], true, "cpu memory").is_err());
        assert!(check_worker_controllers(&["cpu", "memory", "pids"], true, "").is_err());
        check_worker_controllers(&[], false, "").unwrap();
        check_worker_controllers(&["cpu", "memory", "pids"], true, "cpu memory pids").unwrap();
    }

    #[test]
    fn catalog_selects_exactly_one_immutable_identity_with_a_bounded_budget() {
        let temp = tempfile::tempdir().unwrap();
        let _cleanup = ToyRuntimeCleanup(temp.path().to_path_buf());
        let wanted = format!("example.test/wanted@sha256:{}", "c".repeat(64));
        let other = format!("example.test/other@sha256:{}", "d".repeat(64));
        let (path, hash) = catalog(
            temp.path(),
            serde_json::json!([
                entry(&wanted, &temp.path().join("wanted.oci")),
                entry(&other, &temp.path().join("not-loaded.oci")),
            ]),
        );
        let request = import_request(&path, &hash, &wanted, 1999).unwrap();
        assert_eq!(request.archive, temp.path().join("wanted.oci"));
        assert_eq!(request.timeout_secs, 1);
        assert_eq!(request.image_reference, wanted);
        assert!(import_request(&path, &hash, "example.test/wanted:latest", 10000).is_err());
        assert!(import_request(&path, &hash, &wanted, 999).is_err());
        assert!(import_request(&path, &"e".repeat(64), &wanted, 10000).is_err());
        let absent = format!("example.test/absent@sha256:{}", "f".repeat(64));
        assert!(import_request(&path, &hash, &absent, 10000).is_err());
        let (duplicate, duplicate_hash) = catalog(
            temp.path(),
            serde_json::json!([
                entry(&wanted, &temp.path().join("wanted.oci")),
                entry(&wanted, &temp.path().join("other.oci"))
            ]),
        );
        assert!(import_request(&duplicate, &duplicate_hash, &wanted, 10000).is_err());
    }

    #[tokio::test]
    async fn plugin_image_requires_catalog_before_any_podman_command() {
        let temp = tempfile::tempdir().unwrap();
        let _cleanup = ToyRuntimeCleanup(temp.path().to_path_buf());
        let (program, hash) = program(temp.path());
        let image = format!("example.test/unit@sha256:{}", "c".repeat(64));
        let error = ContainerExecutionInfra::for_podman_worker(
            temp.path(),
            &program,
            &hash,
            None,
            None,
            Some(&image),
            Some(10000),
        )
        .await
        .err()
        .unwrap();
        assert!(error.to_string().contains("pinned offline image catalog"));
        assert!(!temp.path().join("podman-image-import.json").exists());
    }

    #[tokio::test]
    async fn worker_preloads_only_the_selected_archive_using_the_existing_strict_importer() {
        let temp = tempfile::tempdir().unwrap();
        let _cleanup = ToyRuntimeCleanup(temp.path().to_path_buf());
        let root = temp.path();
        let archive = root.join("selected.oci");
        let bytes = b"UNIT_ONLY_NOT_A_REAL_OCI_ARCHIVE";
        std::fs::write(&archive, bytes).unwrap();
        let image = format!("example.test/unit@sha256:{}", "c".repeat(64));
        let trace = root.join("calls.jsonl");
        let observed = root.join("observed");
        let (program, _) = program(root);
        std::fs::write(
            &program,
            format!(
                r#"#!/usr/bin/python3
import json,sys,pathlib
args=sys.argv[1:]
with pathlib.Path({trace:?}).open('a') as stream:stream.write(json.dumps(args)+'\n')
if 'load' in args:
 pathlib.Path({observed:?}).write_bytes(pathlib.Path(args[args.index('--input')+1]).read_bytes())
elif 'inspect' in args:
 print(json.dumps([{{'Id':'sha256:{config}','Digest':'sha256:{manifest}'}}]))
else:sys.exit(97)
"#,
                trace = trace.to_str().unwrap(),
                observed = observed.to_str().unwrap(),
                config = "b".repeat(64),
                manifest = "c".repeat(64)
            ),
        )
        .unwrap();
        let program_hash = format!("{:x}", Sha256::digest(std::fs::read(&program).unwrap()));
        let mut selected = entry(&image, &archive);
        selected["archive_sha256"] = format!("{:x}", Sha256::digest(bytes)).into();
        selected["archive_bytes"] = (bytes.len() as u64).into();
        let other = format!("example.test/unused@sha256:{}", "d".repeat(64));
        let (catalog, catalog_hash) = catalog(
            root,
            serde_json::json!([selected, entry(&other, &root.join("missing-unused.oci"))]),
        );
        verified_file(&program, &program_hash, 512 * 1024 * 1024, false).unwrap();
        let mut config = PodmanConfig::for_state_dir(root);
        config.program = program.to_str().unwrap().into();
        std::fs::create_dir(root.join("containers")).unwrap();
        let request = import_request(&catalog, &catalog_hash, &image, 10000).unwrap();
        let receipt = PodmanRuntime::new(config)
            .import_oci_archive(request)
            .await
            .unwrap();
        assert_eq!(std::fs::read(observed).unwrap(), bytes);
        let calls = std::fs::read_to_string(trace).unwrap();
        assert_eq!(calls.lines().count(), 2);
        assert!(!calls.contains("missing-unused"));
        let receipt = serde_json::to_value(receipt).unwrap();
        assert_eq!(receipt["image_reference"], image);
        assert_eq!(
            receipt["manifest_digest"],
            format!("sha256:{}", "c".repeat(64))
        );
    }
}

#[cfg(test)]
mod command_tests {
    use super::*;

    #[test]
    fn worker_commands_use_local_backend_and_private_runtime_environment() {
        let mut config = PodmanConfig::for_state_dir(Path::new("/operator/state"));
        config.worker_runtime_dir = Some("/tmp/apm-fixture".into());
        let check = |command: &std::process::Command| {
            assert!(command.get_args().any(|arg| arg == "--remote=false"));
            let env = command
                .get_envs()
                .map(|(key, value)| {
                    (
                        key.to_string_lossy().into_owned(),
                        value.map(|v| v.to_string_lossy().into_owned()),
                    )
                })
                .collect::<std::collections::BTreeMap<_, _>>();
            assert_eq!(env["XDG_RUNTIME_DIR"].as_deref(), Some("/tmp/apm-fixture"));
            assert_eq!(env["HOME"].as_deref(), Some("/tmp/apm-fixture/home"));
            for name in [
                "CONTAINER_HOST",
                "CONTAINER_CONNECTION",
                "CONTAINERS_CONF",
                "CONTAINERS_STORAGE_CONF",
                "REGISTRY_AUTH_FILE",
            ] {
                assert_eq!(env[name], None, "ambient {name} must be removed");
            }
        };
        check(config.async_command().as_std());
        check(&config.blocking_command());
        config.worker_runtime_dir = None;
        assert!(
            !config
                .blocking_command()
                .get_args()
                .any(|arg| arg == "--remote=false")
        );
        assert!(
            !config
                .blocking_command()
                .get_envs()
                .any(|(key, _)| key == "XDG_RUNTIME_DIR")
        );
    }
}
