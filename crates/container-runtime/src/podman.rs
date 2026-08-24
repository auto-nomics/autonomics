use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use async_trait::async_trait;
use tokio::process::Command;

use crate::error::ContainerRuntimeError;
use crate::process::{current_gid, current_uid, execute_podman};
use crate::runtime::{ContainerRuntime, MAX_CAPTURED_OUTPUT_BYTES};
use crate::types::{ContainerMount, ContainerRunRequest, ContainerRunResult, PullPolicy};

/// Rootless Podman backend.
pub struct PodmanRuntime {
    pub(crate) binary: PathBuf,
    #[cfg(unix)]
    pub(crate) uid: u32,
    #[cfg(unix)]
    pub(crate) gid: u32,
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

    /// `AUTONOMICS_PODMAN_BINARY` is deliberately independent from generic
    /// Docker-compatible endpoint variables.
    pub fn from_env() -> Self {
        let binary = std::env::var_os("AUTONOMICS_PODMAN_BINARY")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("podman"));
        Self::new(binary)
    }

    pub(crate) fn build_args(&self, request: &ContainerRunRequest) -> Vec<String> {
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

#[async_trait]
impl ContainerRuntime for PodmanRuntime {
    async fn run(
        &self,
        request: ContainerRunRequest,
    ) -> Result<ContainerRunResult, ContainerRuntimeError> {
        validate_request(&request)?;
        let args = self.build_args(&request);
        let timeout_secs = request.timeout_secs;
        let output = match execute_podman(
            &self.binary,
            &args,
            timeout_secs,
            MAX_CAPTURED_OUTPUT_BYTES,
        )
        .await
        {
            Ok(output) => output,
            Err(ContainerRuntimeError::Timeout { timeout_secs }) => {
                let name = request.name.clone();
                self.remove_container(&name).await;
                return Err(ContainerRuntimeError::Timeout { timeout_secs });
            }
            Err(error) => return Err(error),
        };

        if !output.success {
            return Err(ContainerRuntimeError::ExitStatus {
                exit_code: output.exit_code,
                stderr: output.stderr,
            });
        }

        Ok(ContainerRunResult {
            exit_code: output.exit_code,
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    fn name(&self) -> &'static str {
        "podman"
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
            "`command` cannot be empty".into(),
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
    if request
        .image
        .chars()
        .chain(request.command.iter().flat_map(|item| item.chars()))
        .chain(request.network.chars())
        .any(|value| value == '\0')
    {
        return Err(ContainerRuntimeError::Invalid(
            "container arguments cannot contain NUL bytes".into(),
        ));
    }
    for (key, value) in &request.env {
        if key.is_empty() || key.contains('=') || key.contains('\0') || value.contains('\0') {
            return Err(ContainerRuntimeError::Invalid(format!(
                "invalid container environment entry `{key}`"
            )));
        }
    }

    let mut mounted = BTreeSet::new();
    for mount in &request.mounts {
        validate_mount(mount)?;
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

pub(crate) fn validate_mount(mount: &ContainerMount) -> Result<(), ContainerRuntimeError> {
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
    Ok(())
}

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
