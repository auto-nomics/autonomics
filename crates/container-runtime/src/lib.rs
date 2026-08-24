//! Minimal container execution framework.
//!
//! The first backend deliberately uses the Podman CLI rather than a daemon
//! API. A host-owned TUI can invoke the user's rootless Podman session without
//! exporting a privileged socket to the workload container.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use thiserror::Error;
use tokio::{io::AsyncReadExt, process::Command};

pub const DEFAULT_CONTAINER_WORKDIR: &str = "/work";
pub const DEFAULT_TIMEOUT_SECS: u64 = 3600;
pub const MAX_CAPTURED_OUTPUT_BYTES: usize = 64 * 1024;

#[cfg(unix)]
use std::os::unix::process::CommandExt as _;

#[derive(Debug, Error)]
pub enum ContainerRuntimeError {
    #[error("invalid container request: {0}")]
    Invalid(String),
    #[error("container command failed to start: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("container exited with status {exit_code}: {stderr}")]
    ExitStatus { exit_code: i32, stderr: String },
    #[error("container was killed before completing within {timeout_secs}s")]
    Timeout { timeout_secs: u64 },
}

/// A reference-data or engine-managed bind mount.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Deserialize)]
pub struct ContainerMount {
    /// Absolute path as seen by the process that invokes Podman.
    pub host_path: String,
    /// Absolute path inside the workload container.
    pub container_path: String,
    /// Writable mounts are opt-in; the default is read-only.
    #[serde(default)]
    pub writable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, JsonSchema, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PullPolicy {
    /// Pull only when the image is absent locally.
    #[default]
    Missing,
    /// Pull before every execution.
    Always,
    /// Pull only when the registry has a newer local image.
    Newer,
    /// Never pull; fail if the image is absent.
    Never,
}

impl PullPolicy {
    fn as_cli_value(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Always => "always",
            Self::Newer => "newer",
            Self::Never => "never",
        }
    }
}

/// One ephemeral container invocation.
#[derive(Debug, Clone)]
pub struct ContainerRunRequest {
    pub image: String,
    /// argv executed inside the image. No shell is inserted.
    pub command: Vec<String>,
    /// Scratch directory on the Podman client host.
    pub host_workdir: PathBuf,
    /// Fixed writable bind destination for `host_workdir`.
    pub container_workdir: String,
    pub env: Vec<(String, String)>,
    pub mounts: Vec<ContainerMount>,
    pub network: String,
    pub read_only_rootfs: bool,
    pub pull_policy: PullPolicy,
    pub cpus: Option<f64>,
    pub memory: Option<String>,
    pub pids_limit: Option<i64>,
    pub shm_size: Option<String>,
    /// Overrides the runtime's default `uid:gid`; reserved for advanced images.
    pub user: Option<String>,
    pub timeout_secs: u64,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct ContainerRunResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[async_trait]
pub trait ContainerRuntime: Send + Sync {
    async fn run(
        &self,
        request: ContainerRunRequest,
    ) -> Result<ContainerRunResult, ContainerRuntimeError>;

    fn name(&self) -> &'static str {
        "container"
    }
}

/// Rootless Podman backend.
pub struct PodmanRuntime {
    binary: PathBuf,
    #[cfg(unix)]
    uid: u32,
    #[cfg(unix)]
    gid: u32,
}

impl PodmanRuntime {
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
            #[cfg(unix)]
            uid: current_uid(),
            #[cfg(unix)]
            gid: current_gid(),
        }
    }

    /// `AUTONOMICS_PODMAN_BINARY` avoids conflicts with the generic `DOCKER_HOST`
    /// style used by other tooling.
    pub fn from_env() -> Self {
        let binary = std::env::var_os("AUTONOMICS_PODMAN_BINARY")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("podman"));
        Self::new(binary)
    }

    fn build_args(&self, request: &ContainerRunRequest) -> Vec<String> {
        let mut args = vec![
            "run".into(),
            "--rm".into(),
            format!("--pull={}", request.pull_policy.as_cli_value()),
            format!("--name={}", request.name),
            format!("--network={}", request.network),
            "--security-opt=no-new-privileges".into(),
            "--userns=keep-id".into(),
        ];

        if request.read_only_rootfs {
            args.push("--read-only".into());
        }
        if let Some(cpus) = request.cpus {
            args.push(format!("--cpus={cpus}"));
        }
        if let Some(memory) = &request.memory {
            args.push(format!("--memory={memory}"));
        }
        if let Some(pids_limit) = request.pids_limit {
            args.push(format!("--pids-limit={pids_limit}"));
        }
        if let Some(shm_size) = &request.shm_size {
            args.push(format!("--shm-size={shm_size}"));
        }

        #[cfg(unix)]
        let user = request
            .user
            .clone()
            .unwrap_or_else(|| format!("{}:{}", self.uid, self.gid));
        #[cfg(not(unix))]
        let user = request.user.clone().unwrap_or_else(|| "0:0".into());
        args.push(format!("--user={user}"));

        args.push(format!(
            "--volume={}:{}:rw",
            request.host_workdir.display(),
            request.container_workdir
        ));
        for mount in &request.mounts {
            let mode = if mount.writable { "rw" } else { "ro" };
            args.push(format!(
                "--volume={}:{}:{mode}",
                mount.host_path, mount.container_path
            ));
        }
        for (key, value) in &request.env {
            args.push(format!("--env={key}={value}"));
        }
        args.push(format!("--workdir={}", request.container_workdir));
        args.push(request.image.clone());
        args.extend(request.command.iter().cloned());
        args
    }
}

#[async_trait]
impl ContainerRuntime for PodmanRuntime {
    async fn run(
        &self,
        request: ContainerRunRequest,
    ) -> Result<ContainerRunResult, ContainerRuntimeError> {
        validate_request(&request)?;
        let args = self.build_args(&request);
        let mut command = Command::new(&self.binary);
        command.args(&args);
        command.stdin(Stdio::null());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        command.kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);

        let mut child = command.spawn()?;
        let mut stdout = child.stdout.take().expect("stdout is piped");
        let mut stderr = child.stderr.take().expect("stderr is piped");
        let stdout_task = tokio::spawn(async move { capture(&mut stdout).await });
        let stderr_task = tokio::spawn(async move { capture(&mut stderr).await });

        let wait =
            tokio::time::timeout(Duration::from_secs(request.timeout_secs), child.wait()).await;
        let (status, timed_out) = match wait {
            Ok(status) => (status?, false),
            Err(_) => {
                if let Some(pid) = child.id() {
                    #[cfg(unix)]
                    kill_process_group(pid);
                    #[cfg(not(unix))]
                    let _ = pid;
                }
                let status = child.wait().await?;
                (status, true)
            }
        };

        let stdout_bytes = stdout_task
            .await
            .unwrap_or_else(|e| Err(std::io::Error::other(e)))
            .unwrap_or_default();
        let stderr_bytes = stderr_task
            .await
            .unwrap_or_else(|e| Err(std::io::Error::other(e)))
            .unwrap_or_default();
        let stdout = lossy_prefix(&stdout_bytes);
        let stderr = lossy_prefix(&stderr_bytes);
        let exit_code = status.code().unwrap_or(-1);

        if timed_out {
            self.remove_container(&request.name).await;
            return Err(ContainerRuntimeError::Timeout {
                timeout_secs: request.timeout_secs,
            });
        }
        if !status.success() {
            return Err(ContainerRuntimeError::ExitStatus { exit_code, stderr });
        }

        Ok(ContainerRunResult {
            exit_code,
            stdout,
            stderr,
        })
    }

    fn name(&self) -> &'static str {
        "podman"
    }
}

impl PodmanRuntime {
    async fn remove_container(&self, name: &str) {
        let _ = Command::new(&self.binary)
            .args(["rm", "--force", name])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
}

fn validate_request(request: &ContainerRunRequest) -> Result<(), ContainerRuntimeError> {
    if request.image.trim().is_empty() {
        return Err(ContainerRuntimeError::Invalid(
            "`image` cannot be empty".into(),
        ));
    }
    if request.command.is_empty() {
        return Err(ContainerRuntimeError::Invalid(
            "container command cannot be empty".into(),
        ));
    }
    if request.timeout_secs == 0 {
        return Err(ContainerRuntimeError::Invalid(
            "`timeout_secs` must be greater than zero".into(),
        ));
    }
    if !request.host_workdir.is_absolute() || !request.host_workdir.is_dir() {
        return Err(ContainerRuntimeError::Invalid(format!(
            "host workdir must be an existing absolute directory: {}",
            request.host_workdir.display()
        )));
    }
    if !Path::new(&request.container_workdir).is_absolute() {
        return Err(ContainerRuntimeError::Invalid(
            "container workdir must be absolute".into(),
        ));
    }
    if request.network.trim().is_empty() {
        return Err(ContainerRuntimeError::Invalid(
            "`network` cannot be empty".into(),
        ));
    }
    if let Some(cpus) = request.cpus
        && cpus <= 0.0
    {
        return Err(ContainerRuntimeError::Invalid(
            "`cpus` must be positive".into(),
        ));
    }
    if let Some(pids_limit) = request.pids_limit
        && pids_limit <= 0
    {
        return Err(ContainerRuntimeError::Invalid(
            "`pids_limit` must be positive".into(),
        ));
    }
    for value in request
        .image
        .chars()
        .chain(request.command.iter().flat_map(|item| item.chars()))
        .chain(request.network.chars())
    {
        if value == '\0' {
            return Err(ContainerRuntimeError::Invalid(
                "container arguments cannot contain NUL bytes".into(),
            ));
        }
    }
    for (key, value) in &request.env {
        if key.is_empty() || key.contains('=') || key.contains('\0') || value.contains('\0') {
            return Err(ContainerRuntimeError::Invalid(format!(
                "invalid container environment entry `{key}`"
            )));
        }
    }
    let mut mounted = std::collections::BTreeSet::new();
    for mount in &request.mounts {
        if !Path::new(&mount.host_path).is_absolute()
            || !Path::new(&mount.host_path).exists()
            || !Path::new(&mount.container_path).is_absolute()
            || mount.host_path.contains('\0')
            || mount.container_path.contains('\0')
        {
            return Err(ContainerRuntimeError::Invalid(format!(
                "invalid mount `{}` -> `{}`",
                mount.host_path, mount.container_path
            )));
        }
        if !mounted.insert(mount.container_path.clone()) {
            return Err(ContainerRuntimeError::Invalid(format!(
                "duplicate container mount path `{}`",
                mount.container_path
            )));
        }
        if mount.container_path == request.container_workdir {
            return Err(ContainerRuntimeError::Invalid(
                "additional mounts cannot target the node workdir".into(),
            ));
        }
    }
    Ok(())
}

async fn capture<R: tokio::io::AsyncRead + Unpin>(mut reader: R) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        if bytes.len() < MAX_CAPTURED_OUTPUT_BYTES {
            let remaining = MAX_CAPTURED_OUTPUT_BYTES.saturating_sub(bytes.len());
            bytes.extend_from_slice(&chunk[..read.min(remaining)]);
        }
    }
    Ok(bytes)
}

fn lossy_prefix(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes).into_owned();
    if bytes.len() >= MAX_CAPTURED_OUTPUT_BYTES {
        format!("{text}\n[output truncated at 64 KiB]")
    } else {
        text
    }
}

#[cfg(unix)]
fn kill_process_group(pid: u32) {
    unsafe {
        libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
    }
}

#[cfg(unix)]
fn current_uid() -> u32 {
    unsafe { libc::getuid() }
}

#[cfg(unix)]
fn current_gid() -> u32 {
    unsafe { libc::getgid() }
}

pub fn unique_container_name() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!(
        "autonomics-container-command-{}-{nanos}",
        std::process::id()
    )
}

pub type SharedContainerRuntime = Arc<dyn ContainerRuntime>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn podman_args_use_rootless_defaults_and_explicit_limits() {
        let runtime = PodmanRuntime::new("podman");
        let request = ContainerRunRequest {
            image: "quay.io/example/tool:1.0".into(),
            command: vec!["tool".into(), "--input".into(), "/work/input.txt".into()],
            host_workdir: Path::new("/tmp/autonomics-work").into(),
            container_workdir: "/work".into(),
            env: vec![("AUTONOMICS_INPUT0".into(), "/work/input.txt".into())],
            mounts: vec![ContainerMount {
                host_path: "/mnt/reference".into(),
                container_path: "/reference".into(),
                writable: false,
            }],
            network: "none".into(),
            read_only_rootfs: true,
            pull_policy: PullPolicy::Never,
            cpus: Some(4.0),
            memory: Some("8g".into()),
            pids_limit: Some(512),
            shm_size: Some("1g".into()),
            user: None,
            timeout_secs: 60,
            name: "test-container".into(),
        };

        let args = runtime.build_args(&request);
        let joined = args.join(" ");

        assert!(joined.contains("--rm"));
        assert!(joined.contains("--pull=never"));
        assert!(joined.contains("--network=none"));
        assert!(joined.contains("--read-only"));
        assert!(joined.contains("--security-opt=no-new-privileges"));
        assert!(joined.contains("--userns=keep-id"));
        let user = format!("--user={}:{}", runtime.uid, runtime.gid);
        assert!(joined.contains(&user));
        assert!(joined.contains("--cpus=4"));
        assert!(joined.contains("--memory=8g"));
        assert!(joined.contains("--pids-limit=512"));
        assert!(joined.contains("--shm-size=1g"));
        assert!(joined.contains("--volume=/mnt/reference:/reference:ro"));
        assert!(joined.contains("--volume=/tmp/autonomics-work:/work:rw"));
        assert!(joined.ends_with("quay.io/example/tool:1.0 tool --input /work/input.txt"));
    }

    #[test]
    fn request_rejects_missing_or_relative_paths() {
        let request = ContainerRunRequest {
            image: "tool".into(),
            command: vec!["tool".into()],
            host_workdir: Path::new("relative").into(),
            container_workdir: "/work".into(),
            env: Vec::new(),
            mounts: Vec::new(),
            network: "none".into(),
            read_only_rootfs: true,
            pull_policy: PullPolicy::Missing,
            cpus: None,
            memory: None,
            pids_limit: None,
            shm_size: None,
            user: None,
            timeout_secs: 10,
            name: "test".into(),
        };

        assert!(validate_request(&request).is_err());
    }
}
