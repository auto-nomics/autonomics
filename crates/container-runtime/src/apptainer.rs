//! Explicit worker-only Apptainer backend. A catalog binds immutable OCI
//! identities to operator-prepared SIF bytes; it does not certify scientific
//! equivalence or replace a formal execution contract.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use crate::connection::{request_user_ids, truncate_captured_bytes, validate_run_request};
use crate::{
    ContainerNetwork, ContainerRunRequest, ContainerRunResult, ContainerRuntimeError, GpuRequest,
    MAX_CAPTURED_OUTPUT_BYTES, PodmanConnection, PullPolicy,
};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SifCatalogEntry {
    pub oci_image: String,
    pub sif_path: PathBuf,
    /// Lowercase hexadecimal SHA-256, without a prefix.
    pub sif_sha256: String,
}

/// Operator-selected enforcement mechanism. This is part of the pinned SIF
/// catalog, never a workload option or an automatic fallback from cgroups v2.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum ApptainerResourceControl {
    DelegatedCgroups {},
    SlurmAllocation {
        swap_policy: crate::SlurmSwapPolicy,
        pids: ApptainerSlurmPidsControl,
    },
}

impl Default for ApptainerResourceControl {
    fn default() -> Self {
        Self::DelegatedCgroups {}
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApptainerSlurmPidsControl {
    JobCgroup,
    /// Non-root UID-wide thread/process limit, inherited across exec. This
    /// includes other tasks of the UID on this node; it is not a per-container
    /// cgroup claim. The hard limit can only become stricter in the child.
    UidRlimit,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SifCatalog {
    version: u32,
    #[serde(default)]
    panel_cache_root: Option<PathBuf>,
    #[serde(default)]
    resource_control: ApptainerResourceControl,
    images: Vec<SifCatalogEntry>,
}

pub struct ApptainerRuntime {
    workspace_root: PathBuf,
    executable: PathBuf,
    panel_cache_root: Option<PathBuf>,
    images: BTreeMap<String, SifCatalogEntry>,
    resource_control: ApptainerResourceControl,
}

fn invalid(message: impl Into<String>) -> ContainerRuntimeError {
    ContainerRuntimeError::Invalid(message.into())
}

fn absolute_path(path: &Path) -> bool {
    let Some(value) = path.to_str() else {
        return false;
    };
    path.is_absolute()
        && !path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        && !value
            .chars()
            .any(|c| c.is_control() || matches!(c, ':' | ','))
        && !value
            .split('/')
            .skip(1)
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn secure_path(path: &Path, directory: bool) -> Result<std::fs::Metadata, ContainerRuntimeError> {
    if !absolute_path(path) {
        return Err(invalid(
            "Apptainer operator paths must be absolute and free of mount delimiters",
        ));
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
        || path.canonicalize()? != path
    {
        return Err(invalid(
            "Apptainer operator paths must be canonical and have the required file type",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o022 != 0
            || (metadata.uid() != 0 && metadata.uid() != unsafe { libc::geteuid() })
        {
            return Err(invalid(
                "Apptainer operator files must be owned by the operator or root and not writable by others",
            ));
        }
        for ancestor in path.ancestors().skip(1) {
            let dir = std::fs::metadata(ancestor)?;
            let trusted_owner = dir.uid() == 0 || dir.uid() == unsafe { libc::geteuid() };
            // A root-owned sticky directory such as /tmp cannot rename an
            // operator-owned child; ordinary writable parents can.
            let sticky_root = dir.uid() == 0 && dir.mode() & libc::S_ISVTX != 0;
            if !trusted_owner || (dir.mode() & 0o022 != 0 && !sticky_root) {
                return Err(invalid(
                    "Apptainer operator files have an untrusted writable ancestor",
                ));
            }
        }
    }
    Ok(metadata)
}

fn secure_file(path: &Path) -> Result<std::fs::Metadata, ContainerRuntimeError> {
    secure_path(path, false)
}

impl ApptainerRuntime {
    /// Called only by the operator-configured cluster worker. Missing catalog
    /// entries and unsupported capabilities never fall back to Podman/pulling.
    pub fn from_catalog_file(
        workspace_root: impl Into<PathBuf>,
        executable: impl Into<PathBuf>,
        catalog_path: &Path,
    ) -> Result<Self, ContainerRuntimeError> {
        if secure_file(catalog_path)?.len() > 2 * 1024 * 1024 {
            return Err(invalid("SIF catalog exceeds 2 MiB"));
        }
        let catalog: SifCatalog = serde_json::from_reader(std::fs::File::open(catalog_path)?)
            .map_err(|error| invalid(format!("invalid operator SIF catalog: {error}")))?;
        if catalog.version != 1 || catalog.images.is_empty() {
            return Err(invalid(
                "SIF catalog requires version 1 and at least one image",
            ));
        }
        if let Some(root) = &catalog.panel_cache_root {
            secure_path(root, true)?;
        }
        let mut images = BTreeMap::new();
        for entry in catalog.images {
            crate::ImageReference::parse(&entry.oci_image)
                .map_err(|error| invalid(format!("invalid SIF catalog OCI identity: {error}")))?;
            let Some((repository, image_digest)) = entry.oci_image.rsplit_once("@sha256:") else {
                return Err(invalid(
                    "SIF catalog OCI identities must use immutable sha256 digests",
                ));
            };
            if repository.is_empty()
                || repository.chars().any(char::is_whitespace)
                || !digest(image_digest)
                || !digest(&entry.sif_sha256)
                || !absolute_path(&entry.sif_path)
            {
                return Err(invalid(
                    "invalid OCI identity, SIF path or SIF SHA-256 in catalog",
                ));
            }
            if images.insert(entry.oci_image.clone(), entry).is_some() {
                return Err(invalid("duplicate OCI identity in SIF catalog"));
            }
        }
        let workspace_root = workspace_root.into();
        if !absolute_path(&workspace_root) {
            return Err(invalid("Apptainer workspace root must be an absolute path"));
        }
        let executable = executable.into();
        secure_file(&executable)?;
        Ok(Self {
            workspace_root,
            executable,
            panel_cache_root: catalog.panel_cache_root,
            images,
            resource_control: catalog.resource_control,
        })
    }

    /// Existing cluster panel cache selected only by the operator catalog.
    pub fn panel_cache_root(&self) -> Option<&Path> {
        self.panel_cache_root.as_deref()
    }

    fn arguments(
        &self,
        request: &ContainerRunRequest,
        entry: &SifCatalogEntry,
    ) -> Result<Vec<String>, ContainerRuntimeError> {
        validate_run_request(request)?;
        let workspace = request.workspace.host_path.canonicalize()?;
        let root = self.workspace_root.canonicalize()?;
        if workspace == root
            || !workspace.starts_with(root)
            || !absolute_path(&workspace)
            || !absolute_path(Path::new(&request.workspace.container_workdir))
            || request.workspace.container_workdir == "/"
            || ["/dev", "/proc", "/sys", "/run", "/tmp"]
                .iter()
                .any(|path| Path::new(&request.workspace.container_workdir).starts_with(path))
        {
            return Err(invalid(
                "Apptainer workspace must stay below its operator-owned root",
            ));
        }
        if entry.sif_path.starts_with(&workspace) || self.executable.starts_with(&workspace) {
            return Err(invalid(
                "SIF and Apptainer executable must stay outside the writable task workspace",
            ));
        }
        if request.network != ContainerNetwork::Isolated
            || !request.read_only_rootfs
            || !matches!(request.pull_policy, PullPolicy::Never | PullPolicy::Missing)
            || request.shm_size.is_some()
            || request.gpus != GpuRequest::None
        {
            return Err(invalid(
                "Apptainer worker requires isolated networking, read-only SIF, no runtime pull, no shm-size override and no GPU request",
            ));
        }
        let (uid, gid) = request_user_ids(request);
        if uid == 0
            || uid != unsafe { libc::geteuid() } as i64
            || gid != unsafe { libc::getegid() } as i64
        {
            return Err(invalid(
                "Apptainer worker requires the current non-root operator uid/gid; remapping is unsupported",
            ));
        }
        if request.cpus.is_some_and(|n| !n.is_finite()) {
            return Err(invalid("Apptainer CPU limit must be finite"));
        }
        let mut arguments: Vec<String> = [
            "exec",
            "--containall",
            "--cleanenv",
            "--disable-cache",
            "--no-eval",
            "--no-home",
            "--no-mount",
            "bind-paths,hostfs,cwd",
            "--net",
            "--network",
            "none",
            "--drop-caps",
            "ALL",
            "--userns",
            "--no-privs",
            "--pwd",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        arguments.push(request.workspace.container_workdir.clone());
        arguments.extend([
            "--bind".into(),
            format!(
                "{}:{}:rw",
                workspace.display(),
                request.workspace.container_workdir
            ),
        ]);
        let mut mounts = BTreeSet::new();
        mounts.insert(PathBuf::from(&request.workspace.container_workdir));
        for (source, target) in request
            .input_mounts
            .iter()
            .map(|m| (&m.host_dir, m.host_dir.as_path()))
            .chain(
                request
                    .panels
                    .iter()
                    .map(|p| (&p.host_path, Path::new(&p.mount_path))),
            )
        {
            if !absolute_path(source)
                || !absolute_path(target)
                || target == Path::new("/")
                || !source.is_dir()
                || source.canonicalize()? != *source
                || mounts
                    .iter()
                    .any(|m| target.starts_with(m) || m.starts_with(target))
                || ["/dev", "/proc", "/sys", "/run", "/tmp"]
                    .iter()
                    .any(|p| target.starts_with(p))
            {
                return Err(invalid(
                    "Apptainer read-only mounts must be canonical, distinct safe directories",
                ));
            }
            mounts.insert(target.to_path_buf());
            arguments.extend([
                "--bind".into(),
                format!("{}:{}:ro", source.display(), target.display()),
            ]);
        }
        let delegated = matches!(
            self.resource_control,
            ApptainerResourceControl::DelegatedCgroups {}
        );
        if let Some(cpus) = request.cpus.filter(|_| delegated) {
            arguments.extend(["--cpus".into(), cpus.to_string()]);
        }
        if let Some(memory) = request.memory.as_ref().filter(|_| delegated) {
            let bytes = crate::podman::podman_size(memory)?;
            arguments.extend([
                "--memory".into(),
                bytes.clone(),
                "--memory-swap".into(),
                bytes,
            ]);
        }
        if let Some(pids) = request.pids_limit.filter(|_| delegated) {
            arguments.extend(["--pids-limit".into(), pids.to_string()]);
        }
        let mut names = BTreeSet::new();
        for (name, value) in &request.env {
            if name.is_empty()
                || !name.bytes().enumerate().all(|(i, c)| {
                    c == b'_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit())
                })
                || value.contains('\0')
                || !names.insert(name)
            {
                return Err(invalid(
                    "invalid or duplicate Apptainer workload environment name",
                ));
            }
        }
        if request.command.iter().any(|arg| arg.contains('\0')) {
            return Err(invalid("NUL in Apptainer command"));
        }
        arguments.push(entry.sif_path.to_string_lossy().into_owned());
        arguments.extend(request.command.clone());
        Ok(arguments)
    }

    fn admit_resources(
        &self,
        request: &ContainerRunRequest,
    ) -> Result<Option<(serde_json::Value, Option<libc::rlim_t>)>, ContainerRuntimeError> {
        let ApptainerResourceControl::SlurmAllocation { swap_policy, pids } = self.resource_control
        else {
            return Ok(None);
        };
        let mut inherited = request.clone();
        let nproc = match pids {
            ApptainerSlurmPidsControl::JobCgroup => None,
            ApptainerSlurmPidsControl::UidRlimit => {
                let limit = request
                    .pids_limit
                    .and_then(|n| libc::rlim_t::try_from(n).ok())
                    .filter(|n| *n > 0)
                    .ok_or_else(|| {
                        invalid("UID rlimit policy requires an explicit positive PID budget")
                    })?;
                ensure_unprivileged_principal()?;
                // Only the PID mechanism changes. CPU, RAM, swap, membership
                // and exact Slurm ownership still use the production observer.
                inherited.pids_limit = None;
                Some(limit)
            }
        };
        let evidence = crate::slurm_allocation::observe_and_validate(&inherited, swap_policy)?;
        let receipt = serde_json::json!({
            "schema": "APPTAINER_SLURM_RESOURCE_ENFORCEMENT_V1",
            "allocation": evidence,
            "pid_mechanism": if nproc.is_some() { "uid_rlimit_nproc" } else { "job_cgroup" },
            "declared_pids_limit": request.pids_limit,
            "uid_limit_includes_other_uid_tasks_on_node": nproc.is_some(),
            "pids_cgroup_claimed": nproc.is_none() && request.pids_limit.is_some(),
        });
        Ok(Some((receipt, nproc)))
    }
}

fn ensure_unprivileged_principal() -> std::io::Result<()> {
    // A root or set-ID process is not governed by the promised UID limit.
    let (mut real, mut effective, mut saved) = (0, 0, 0);
    let (mut real_gid, mut effective_gid, mut saved_gid) = (0, 0, 0);
    if unsafe { libc::getresuid(&mut real, &mut effective, &mut saved) } != 0
        || unsafe { libc::getresgid(&mut real_gid, &mut effective_gid, &mut saved_gid) } != 0
    {
        return Err(std::io::Error::last_os_error());
    }
    if real == 0
        || real != effective
        || real != saved
        || real_gid != effective_gid
        || real_gid != saved_gid
    {
        return Err(std::io::Error::from_raw_os_error(libc::EPERM));
    }
    // Linux exempts CAP_SYS_ADMIN / CAP_SYS_RESOURCE from RLIMIT_NPROC.
    // Reject such principals instead of relying on their nominal non-root UID.
    #[repr(C)]
    struct Header {
        version: u32,
        pid: i32,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Data {
        effective: u32,
        permitted: u32,
        inheritable: u32,
    }
    let mut header = Header {
        version: 0x2008_0522,
        pid: 0,
    };
    let mut data = [Data {
        effective: 0,
        permitted: 0,
        inheritable: 0,
    }; 2];
    if unsafe { libc::syscall(libc::SYS_capget, &mut header, data.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    if (data[0].effective | data[0].permitted | data[0].inheritable) & ((1 << 21) | (1 << 24)) != 0
    {
        return Err(std::io::Error::from_raw_os_error(libc::EPERM));
    }
    Ok(())
}

/// Called only in the child immediately before exec; no allocation or locks.
unsafe fn constrain_nproc(limit: libc::rlim_t) -> std::io::Result<()> {
    if limit == 0 {
        return Err(std::io::Error::from_raw_os_error(libc::EINVAL));
    }
    ensure_unprivileged_principal()?;
    let mut before = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    if unsafe { libc::getrlimit(libc::RLIMIT_NPROC, &mut before) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let constrained = libc::rlimit {
        rlim_cur: before.rlim_cur.min(limit),
        rlim_max: before.rlim_max.min(limit),
    };
    if unsafe { libc::setrlimit(libc::RLIMIT_NPROC, &constrained) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut after = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    if unsafe { libc::getrlimit(libc::RLIMIT_NPROC, &mut after) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    if after.rlim_cur != constrained.rlim_cur || after.rlim_max != constrained.rlim_max {
        return Err(std::io::Error::from_raw_os_error(libc::EPERM));
    }
    Ok(())
}

fn verify_sif(entry: SifCatalogEntry) -> Result<(), ContainerRuntimeError> {
    let before = secure_file(&entry.sif_path)?;
    let mut file = std::fs::File::open(&entry.sif_path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let after = secure_file(&entry.sif_path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != after.dev()
            || before.ino() != after.ino()
            || before.len() != after.len()
            || before.mtime_nsec() != after.mtime_nsec()
            || before.mtime() != after.mtime()
            || file.metadata()?.ino() != after.ino()
        {
            return Err(invalid("SIF identity changed during SHA-256 verification"));
        }
    }
    if format!("{:x}", hasher.finalize()) != entry.sif_sha256 {
        return Err(invalid("SIF SHA-256 differs from the operator catalog"));
    }
    Ok(())
}

struct ProcessGroup(u32);
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        // The group is created for this attached invocation. No detached
        // instance is used; Slurm retains the encompassing worker cgroup.
        unsafe {
            libc::kill(-(self.0 as i32), libc::SIGKILL);
        }
    }
}

async fn capture(mut input: impl AsyncRead + Unpin) -> std::io::Result<Vec<u8>> {
    let mut retained = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let size = input.read(&mut buffer).await?;
        if size == 0 {
            return Ok(retained);
        }
        let remaining = (MAX_CAPTURED_OUTPUT_BYTES + 1).saturating_sub(retained.len());
        retained.extend_from_slice(&buffer[..size.min(remaining)]);
    }
}

#[async_trait]
impl PodmanConnection for ApptainerRuntime {
    fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }
    fn name(&self) -> &'static str {
        "apptainer"
    }

    async fn run(
        &self,
        request: ContainerRunRequest,
    ) -> Result<ContainerRunResult, ContainerRuntimeError> {
        self.run_observed(request, None, Default::default()).await
    }

    async fn run_observed(
        &self,
        request: ContainerRunRequest,
        private: Option<crate::types::PrivateEnvironment>,
        observer: crate::observation::RunObserver,
    ) -> Result<ContainerRunResult, ContainerRuntimeError> {
        if private.is_some() {
            return Err(invalid(
                "runtime does not support private credential forwarding",
            ));
        }
        observer.phase("verifying_image", "verifying the pinned image");
        let entry = self.images.get(&request.image).ok_or_else(|| invalid("OCI identity has no operator-owned SIF mapping; runtime pull and local fallback are forbidden"))?;
        let arguments = self.arguments(&request, entry)?;
        let admission = self.admit_resources(&request)?;
        let entry = entry.clone();
        tokio::task::spawn_blocking(move || verify_sif(entry))
            .await
            .map_err(|error| invalid(format!("SIF verification worker failed: {error}")))??;
        let mut command = Command::new(&self.executable);
        command
            .args(arguments)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.workspace_root)
            // Keep runtime reconstruction scratch inside this task's owned
            // workspace. The enclosing Slurm job's stop proof governs cleanup.
            .env("TMPDIR", &request.workspace.host_path)
            .env("APPTAINER_TMPDIR", &request.workspace.host_path)
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        // Only async-signal-safe syscalls run between fork and exec. Inherit
        // no-new-privileges into the rootless Apptainer process and payload.
        unsafe {
            let nproc = admission.as_ref().and_then(|(_, limit)| *limit);
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if let Some(limit) = nproc {
                    constrain_nproc(limit)?;
                }
                Ok(())
            });
        }
        for (name, value) in &request.env {
            command.env(format!("APPTAINERENV_{name}"), value);
        }
        let mut child = command.spawn()?;
        let group = ProcessGroup(
            child
                .id()
                .ok_or_else(|| invalid("Apptainer child has no pid"))?,
        );
        if let Some((mut evidence, _)) = admission {
            evidence["pre_exec_completed"] = true.into();
            evidence["child_pid"] = group.0.into();
            crate::podman_worker::write_worker_evidence(
                &request
                    .workspace
                    .host_path
                    .join(".autonomics-apptainer-resources.json"),
                &evidence,
            )?;
        }
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| invalid("Apptainer stdout unavailable"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| invalid("Apptainer stderr unavailable"))?;
        let output = async {
            let wait = async {
                let mut tick = tokio::time::interval(Duration::from_secs(5));
                loop {
                    tokio::select! {
                        result = child.wait() => {
                            observer.process("collecting_outputs", group.0,
                                if result.is_ok() { "terminal" } else { "unknown" }, "apptainer");
                            break result;
                        }
                        _ = tick.tick() => observer.process("executing", group.0, "running", "apptainer"),
                    }
                }
            };
            let (status, stdout, stderr) =
                tokio::try_join!(wait, capture(stdout), capture(stderr))?;
            Ok::<_, std::io::Error>((status, stdout, stderr))
        };
        let result = tokio::time::timeout(Duration::from_secs(request.timeout_secs), output).await;
        drop(group);
        match result {
            Err(_) => {
                let _ = child.wait().await;
                Err(ContainerRuntimeError::Timeout {
                    timeout_secs: request.timeout_secs,
                })
            }
            Ok(result) => {
                let (status, stdout, stderr) = result?;
                let exit_code = status.code().unwrap_or(-1);
                let stdout = truncate_captured_bytes(&stdout);
                let stderr = truncate_captured_bytes(&stderr);
                if !status.success() {
                    return Err(ContainerRuntimeError::ExitStatus {
                        exit_code,
                        stdout,
                        stderr,
                    });
                }
                Ok(ContainerRunResult {
                    exit_code,
                    stdout,
                    stderr,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CachedPanel, WorkspaceRef};
    use std::os::unix::fs::PermissionsExt;

    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
        executable: PathBuf,
        catalog: PathBuf,
        image: String,
        sif: PathBuf,
    }

    impl Fixture {
        fn new(program: &str) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("work");
            std::fs::create_dir_all(root.join("task")).unwrap();
            let executable = dir.path().join("fake-apptainer");
            std::fs::write(&executable, format!("#!/usr/bin/python3\n{program}\n")).unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
            let sif = dir.path().join("toy.sif");
            std::fs::write(&sif, b"synthetic SIF bytes, no scientific method").unwrap();
            let catalog = dir.path().join("catalog.json");
            let image = format!("registry.example/toy@sha256:{}", "a".repeat(64));
            let entry = serde_json::json!({
                "oci_image": image, "sif_path": sif,
                "sif_sha256": format!("{:x}", Sha256::digest(std::fs::read(&sif).unwrap())),
            });
            std::fs::write(
                &catalog,
                serde_json::to_vec(&serde_json::json!({"version":1,"images":[entry]})).unwrap(),
            )
            .unwrap();
            Self {
                _dir: dir,
                root,
                executable,
                catalog,
                image,
                sif,
            }
        }

        fn runtime(&self) -> Result<ApptainerRuntime, ContainerRuntimeError> {
            ApptainerRuntime::from_catalog_file(&self.root, &self.executable, &self.catalog)
        }

        fn request(&self) -> ContainerRunRequest {
            ContainerRunRequest {
                image: self.image.clone(),
                command: vec!["python".into(), "literal $(touch /BAD); spaced".into()],
                workspace: WorkspaceRef {
                    host_path: self.root.join("task"),
                    container_workdir: "/work".into(),
                },
                env: vec![("TOY_VALUE".into(), "literal $HOME,comma".into())],
                panels: Vec::new(),
                input_mounts: Vec::new(),
                network: ContainerNetwork::Isolated,
                read_only_rootfs: true,
                pull_policy: PullPolicy::Never,
                cpus: Some(2.5),
                memory: Some("1Gi".into()),
                pids_limit: Some(128),
                shm_size: None,
                gpus: GpuRequest::None,
                user: None,
                timeout_secs: 5,
                name: "toy-test".into(),
            }
        }
    }

    #[test]
    fn catalog_rejects_mutable_unknown_duplicate_and_untrusted_entries() {
        let fixture = Fixture::new("pass");
        let original = std::fs::read(&fixture.catalog).unwrap();
        for mutation in ["tag", "digest", "duplicate", "unknown"] {
            let mut json: serde_json::Value = serde_json::from_slice(&original).unwrap();
            match mutation {
                "tag" => json["images"][0]["oci_image"] = "registry.example/toy:latest".into(),
                "digest" => json["images"][0]["sif_sha256"] = "0".into(),
                "duplicate" => {
                    let entry = json["images"][0].clone();
                    json["images"].as_array_mut().unwrap().push(entry);
                }
                "unknown" => json["scientifically_approved"] = true.into(),
                _ => unreachable!(),
            }
            std::fs::write(&fixture.catalog, serde_json::to_vec(&json).unwrap()).unwrap();
            assert!(fixture.runtime().is_err(), "{mutation}");
        }
        std::fs::write(&fixture.catalog, original).unwrap();
        std::fs::set_permissions(&fixture.catalog, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(fixture.runtime().is_err());
    }

    #[test]
    fn catalog_can_reuse_a_verified_operator_owned_panel_cache_root() {
        let fixture = Fixture::new("pass");
        let panel_root = fixture._dir.path().join("existing-panels");
        std::fs::create_dir(&panel_root).unwrap();
        let mut catalog: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&fixture.catalog).unwrap()).unwrap();
        catalog["panel_cache_root"] = serde_json::to_value(&panel_root).unwrap();
        std::fs::write(&fixture.catalog, serde_json::to_vec(&catalog).unwrap()).unwrap();
        let infra = crate::ContainerExecutionInfra::for_apptainer_worker(
            &fixture.root,
            &fixture.catalog,
            &fixture.executable,
        )
        .unwrap();
        assert_eq!(infra.config.panel_cache_root, panel_root);
        assert_eq!(infra.panel_cache.root, panel_root);
        assert!(infra.panel_cache.requires_content_verification());
        std::fs::set_permissions(&panel_root, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(fixture.runtime().is_err());
    }

    #[test]
    fn unsupported_capabilities_cannot_silently_weaken_isolation() {
        let fixture = Fixture::new("pass");
        let runtime = fixture.runtime().unwrap();
        for capability in [
            "network",
            "rootfs",
            "pull",
            "gpu",
            "shm",
            "root-user",
            "nonfinite-cpu",
            "environment",
        ] {
            let mut request = fixture.request();
            match capability {
                "network" => request.network = ContainerNetwork::Egress,
                "rootfs" => request.read_only_rootfs = false,
                "pull" => request.pull_policy = PullPolicy::Always,
                "gpu" => request.gpus = GpuRequest::All,
                "shm" => request.shm_size = Some("1Gi".into()),
                "root-user" => request.user = Some("0:0".into()),
                "nonfinite-cpu" => request.cpus = Some(f64::NAN),
                "environment" => request.env.push(("--bind".into(), "/:/host".into())),
                _ => unreachable!(),
            }
            assert!(
                runtime
                    .arguments(&request, runtime.images.get(&fixture.image).unwrap())
                    .is_err(),
                "{capability}"
            );
        }
    }

    #[test]
    fn arguments_keep_exact_resources_literal_argv_and_read_only_panels() {
        let fixture = Fixture::new("pass");
        let runtime = fixture.runtime().unwrap();
        let mut request = fixture.request();
        let panel = fixture._dir.path().join("panel");
        std::fs::create_dir(&panel).unwrap();
        request.panels.push(CachedPanel {
            id: "toy".into(),
            digest: "unused".into(),
            host_path: panel.clone(),
            mount_path: "/references/toy".into(),
            manifest_bytes: None,
        });
        let args = runtime
            .arguments(&request, runtime.images.get(&fixture.image).unwrap())
            .unwrap();
        for pair in [
            ["--network", "none"],
            ["--cpus", "2.5"],
            ["--memory", "1073741824"],
            ["--memory-swap", "1073741824"],
            ["--pids-limit", "128"],
        ] {
            assert!(args.windows(2).any(|window| window == pair));
        }
        assert!(args.contains(&format!("{}:/references/toy:ro", panel.display())));
        assert!(args.contains(&"--containall".into()));
        assert!(args.contains(&"--no-eval".into()));
        assert!(args.ends_with(&request.command));
        assert!(!args.contains(&"--writable".into()));
        request.panels[0].mount_path = "/work/overwrite".into();
        assert!(
            runtime
                .arguments(&request, runtime.images.get(&fixture.image).unwrap())
                .is_err()
        );
    }

    #[test]
    fn slurm_policy_is_explicit_strict_and_preserves_container_isolation() {
        for invalid in [
            serde_json::json!({"mode":"delegated_cgroups","pids":"uid_rlimit"}),
            serde_json::json!({"mode":"slurm_allocation","swap_policy":"inherit"}),
            serde_json::json!({"mode":"slurm_allocation","swap_policy":"inherit","pids":"none"}),
            serde_json::json!({"mode":"slurm_allocation","swap_policy":"inherit","pids":"uid_rlimit","skip_admission":true}),
        ] {
            assert!(serde_json::from_value::<ApptainerResourceControl>(invalid).is_err());
        }
        let fixture = Fixture::new("raise RuntimeError('not executed by argument test')");
        let original: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&fixture.catalog).unwrap()).unwrap();
        for pids in ["uid_rlimit", "job_cgroup"] {
            let mut catalog = original.clone();
            catalog["resource_control"] = serde_json::json!({
                "mode":"slurm_allocation","swap_policy":"inherit","pids":pids,
            });
            std::fs::write(&fixture.catalog, serde_json::to_vec(&catalog).unwrap()).unwrap();
            let runtime = fixture.runtime().unwrap();
            let request = fixture.request();
            let args = runtime
                .arguments(&request, runtime.images.get(&fixture.image).unwrap())
                .unwrap();
            for flag in ["--cpus", "--memory", "--memory-swap", "--pids-limit"] {
                assert!(!args.iter().any(|v| v == flag), "{pids}: {flag}");
            }
            for flag in [
                "--containall",
                "--userns",
                "--no-privs",
                "--no-eval",
                "--cleanenv",
            ] {
                assert!(args.iter().any(|v| v == flag), "{pids}: {flag}");
            }
            assert!(args.windows(2).any(|v| v == ["--network", "none"]));
            assert!(args.windows(2).any(|v| v == ["--drop-caps", "ALL"]));
            assert!(args.ends_with(&request.command));
        }
    }

    #[test]
    fn nproc_child_hard_limit_survives_exec_and_cannot_be_raised() {
        if unsafe { libc::geteuid() } == 0 {
            assert!(unsafe { constrain_nproc(128) }.is_err());
            return;
        }
        use std::os::unix::process::CommandExt;
        let mut parent_before = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        assert_eq!(
            unsafe { libc::getrlimit(libc::RLIMIT_NPROC, &mut parent_before) },
            0
        );
        for stricter_first in [false, true] {
            let mut command = std::process::Command::new("/usr/bin/python3");
            command.args([
                "-I",
                "-S",
                "-c",
                r#"
import resource
soft,hard=resource.getrlimit(resource.RLIMIT_NPROC)
assert 0 <= soft <= hard <= 128
try: resource.setrlimit(resource.RLIMIT_NPROC,(hard+1,hard+1))
except (ValueError,PermissionError): pass
else: raise AssertionError('unprivileged child raised its hard process limit')
print(hard)
"#,
            ]);
            unsafe {
                command.pre_exec(move || {
                    if stricter_first {
                        constrain_nproc(64)?;
                    }
                    constrain_nproc(128)
                });
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let hard: u64 = String::from_utf8(output.stdout)
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            assert!(hard <= if stricter_first { 64 } else { 128 });
        }
        let mut parent_after = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        assert_eq!(
            unsafe { libc::getrlimit(libc::RLIMIT_NPROC, &mut parent_after) },
            0
        );
        assert_eq!(
            (parent_before.rlim_cur, parent_before.rlim_max),
            (parent_after.rlim_cur, parent_after.rlim_max)
        );
    }

    #[tokio::test]
    async fn slurm_policy_rejects_unproved_allocation_before_launch() {
        let fixture = Fixture::new("from pathlib import Path\nPath(__file__+'.launched').touch()");
        let mut runtime = fixture.runtime().unwrap();
        runtime.resource_control = ApptainerResourceControl::SlurmAllocation {
            swap_policy: crate::SlurmSwapPolicy::Inherit,
            pids: ApptainerSlurmPidsControl::UidRlimit,
        };
        let mut request = fixture.request();
        // One byte cannot cover a real Rust worker allocation, including when
        // this regression itself executes inside Slurm rather than on a laptop.
        request.memory = Some("1".into());
        assert!(runtime.run(request).await.is_err());
        assert!(
            !fixture
                .executable
                .with_file_name("fake-apptainer.launched")
                .exists()
        );
        let mut request = fixture.request();
        request.pids_limit = None;
        assert!(runtime.admit_resources(&request).is_err());
    }

    /// Run explicitly on a real 2-CPU/2-GiB Slurm allocation. The ordinary
    /// suite never substitutes a fake runtime for this environment test.
    #[tokio::test]
    #[ignore = "requires an allocated Slurm node and operator-pinned real SIF catalog"]
    async fn actual_slurm_apptainer_enforces_uid_limit_and_private_network() {
        let catalog = std::env::var_os("AUTONOMICS_TEST_APPTAINER_CATALOG")
            .expect("operator catalog required");
        let image = std::env::var("AUTONOMICS_TEST_APPTAINER_OCI")
            .expect("exact mapped OCI identity required");
        let executable = std::env::var_os("AUTONOMICS_TEST_APPTAINER_EXECUTABLE")
            .expect("canonical operator executable required");
        let permanent = PathBuf::from(
            std::env::var_os("AUTONOMICS_VALIDATION_PERMANENT")
                .expect("permanent evidence required"),
        );
        let fixture = Fixture::new("raise RuntimeError('fake is never used')");
        let runtime =
            ApptainerRuntime::from_catalog_file(&fixture.root, executable, Path::new(&catalog))
                .unwrap();
        assert!(matches!(
            runtime.resource_control,
            ApptainerResourceControl::SlurmAllocation {
                pids: ApptainerSlurmPidsControl::UidRlimit,
                ..
            }
        ));
        let mut request = fixture.request();
        request.image = image;
        request.cpus = Some(2.0);
        request.memory = Some("2Gi".into());
        request.timeout_secs = 90;
        request.command = vec![
            "/usr/local/bin/python3".into(),
            "-I".into(),
            "-S".into(),
            "-c".into(),
            r#"
import errno,json,os,resource,signal,time
from pathlib import Path
soft,hard=resource.getrlimit(resource.RLIMIT_NPROC)
assert 0 < soft <= hard <= 128 and os.getuid()==os.geteuid()!=0
rows=Path('/proc/self/net/dev').read_text().splitlines()[2:]
interfaces=sorted(row.split(':')[0].strip() for row in rows)
assert interfaces==['lo'], interfaces
assert len(Path('/proc/self/net/route').read_text().splitlines())==1
try: resource.setrlimit(resource.RLIMIT_NPROC,(hard+1,hard+1))
except (PermissionError,ValueError): pass
else: raise AssertionError('hard process limit can be raised')
children=[];reaped=[];error=None
try:
    for _ in range(hard+1):
        try: pid=os.fork()
        except OSError as e: error=e.errno;break
        if pid==0: time.sleep(30);os._exit(0)
        children.append(pid)
    assert error==errno.EAGAIN and 0<len(children)<hard
finally:
    for pid in children:
        try: os.kill(pid,signal.SIGTERM)
        except ProcessLookupError: pass
    for pid in children:
        got,status=os.waitpid(pid,0);assert got==pid;reaped.append(pid)
assert reaped==children
print(json.dumps({'uid':os.getuid(),'rlimit_nproc':[soft,hard],
    'interfaces':interfaces,'network_namespace':os.readlink('/proc/self/ns/net'),
    'live_children_at_rejection':len(children),'fork_errno':error,
    'all_children_reaped':True,'method_executed':False,'scientific_authorization':False}))
"#
            .into(),
        ];
        let result = runtime.run(request).await.unwrap();
        let payload: serde_json::Value = serde_json::from_str(&result.stdout).unwrap();
        assert_eq!(payload["all_children_reaped"], true);
        assert_eq!(payload["interfaces"], serde_json::json!(["lo"]));
        assert_eq!(payload["method_executed"], false);
        let resources = std::fs::read(
            fixture
                .root
                .join("task/.autonomics-apptainer-resources.json"),
        )
        .unwrap();
        let receipt: serde_json::Value = serde_json::from_slice(&resources).unwrap();
        assert_eq!(receipt["pid_mechanism"], "uid_rlimit_nproc");
        assert_eq!(receipt["pids_cgroup_claimed"], false);
        assert_eq!(receipt["pre_exec_completed"], true);
        assert_eq!(
            receipt["allocation"]["validated_request"]["memory_bytes"],
            2_u64 * 1024 * 1024 * 1024
        );
        crate::podman_worker::write_worker_evidence(
            &permanent.join("ACTUAL_APPTAINER_PAYLOAD.json"),
            &payload,
        )
        .unwrap();
        crate::podman_worker::write_worker_evidence(
            &permanent.join("ACTUAL_APPTAINER_RESOURCES.json"),
            &receipt,
        )
        .unwrap();
    }

    #[tokio::test]
    async fn execution_rejects_unmapped_or_changed_sif_before_launch() {
        let fixture = Fixture::new("raise Exception('must never launch')");
        let runtime = fixture.runtime().unwrap();
        let mut request = fixture.request();
        request.image = "registry.example/toy:latest".into();
        assert!(
            runtime
                .run(request)
                .await
                .unwrap_err()
                .to_string()
                .contains("no operator-owned SIF mapping")
        );
        std::fs::write(&fixture.sif, b"tampered").unwrap();
        assert!(
            runtime
                .run(fixture.request())
                .await
                .unwrap_err()
                .to_string()
                .contains("SHA-256 differs")
        );
    }

    #[tokio::test]
    async fn fake_execution_preserves_literal_environment_and_bounded_output() {
        let fixture = Fixture::new(
            "import os,sys\nassert os.environ['APPTAINERENV_TOY_VALUE']=='literal $HOME,comma'\nassert not any(k.startswith('AUTONOMICS_') for k in os.environ)\nassert os.environ['TMPDIR']==os.environ['APPTAINER_TMPDIR']\nassert os.path.isdir(os.environ['TMPDIR'])\nprint('x'*70000)\nprint('toy stderr',file=sys.stderr)",
        );
        let result = fixture
            .runtime()
            .unwrap()
            .run(fixture.request())
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("[output truncated at 65536 bytes]"));
        assert_eq!(result.stderr, "toy stderr\n");
    }

    #[tokio::test]
    async fn fake_nonzero_exit_and_timeout_do_not_report_success() {
        let fixture = Fixture::new("import sys\nprint('toy failure',file=sys.stderr)\nsys.exit(7)");
        assert!(matches!(
            fixture.runtime().unwrap().run(fixture.request()).await,
            Err(ContainerRuntimeError::ExitStatus { exit_code: 7, .. })
        ));
        let slow = Fixture::new("import time\ntime.sleep(30)");
        let mut request = slow.request();
        request.timeout_secs = 1;
        assert!(matches!(
            slow.runtime().unwrap().run(request).await,
            Err(ContainerRuntimeError::Timeout { timeout_secs: 1 })
        ));
    }

    #[tokio::test]
    async fn cancellation_kills_attached_invocation_and_its_child_group() {
        use std::os::unix::process::ExitStatusExt;

        fn birth(pid: u32) -> std::io::Result<Option<String>> {
            let text = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                Ok(text) => text,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error),
            };
            // Starttime (field 22), preserving the parenthesized command.
            let birth = text
                .rsplit_once(") ")
                .and_then(|(_, rest)| rest.split_whitespace().nth(19))
                .ok_or_else(|| std::io::Error::other("malformed owned process stat"))?;
            Ok(Some(birth.to_owned()))
        }

        let fixture = Fixture::new(
            "import os,sys,time\nwork=sys.argv[sys.argv.index('--bind')+1].split(':')[0]\nwith open(work+'/pids.pending','w') as f:\n f.write(str(os.getpid())); f.flush(); os.fsync(f.fileno())\nos.replace(work+'/pids.pending',work+'/pids')\ntime.sleep(30)",
        );
        let runtime = fixture.runtime().unwrap();
        let request = fixture.request();
        // Dropping the polled run future is the same cancellation boundary as
        // aborting a Tokio task, while leaving no detached test JoinHandle.
        let mut invocation = Box::pin(runtime.run(request));
        let path = fixture.root.join("task/pids");
        let leader = tokio::select! {
            result = &mut invocation => panic!("fake Apptainer ended before cancellation: {result:?}"),
            pid = async {
                for _ in 0..200 {
                    if let Ok(text) = std::fs::read_to_string(&path) {
                        return text.parse::<u32>().unwrap();
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                panic!("fake Apptainer did not publish its pid");
            } => pid,
        };
        let leader_birth = birth(leader)
            .unwrap()
            .expect("fake Apptainer must still be running");
        // The test owns this Child directly and joins it to the actual runtime
        // group. Killing a grandchild inside the fake process instead would
        // orphan its wait status into an enclosing acceptance subreaper.
        let mut member = Command::new("/usr/bin/sleep")
            .arg("30")
            .process_group(leader as i32)
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let member_pid = member.id().unwrap();
        assert_eq!(unsafe { libc::getpgid(member_pid as i32) }, leader as i32);
        drop(invocation);
        let status = match tokio::time::timeout(Duration::from_secs(2), member.wait()).await {
            Ok(result) => result.unwrap(),
            Err(_) => {
                // Even a failing cancellation regression must reap this exact
                // owned child before the fixture can be removed.
                let _ = member.kill().await;
                let _ = member.wait().await;
                panic!("owned group member {member_pid} survived cancellation");
            }
        };
        assert_eq!(status.signal(), Some(libc::SIGKILL));
        let mut leader_reaped = false;
        for _ in 0..200 {
            if birth(leader).unwrap().as_deref() != Some(leader_birth.as_str()) {
                leader_reaped = true;
                break;
            }
            // A zombie still has the same birth identity: it is not enough
            // merely to observe that it stopped running.
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            leader_reaped,
            "fake Apptainer leader was not reaped by Tokio"
        );
    }
}
