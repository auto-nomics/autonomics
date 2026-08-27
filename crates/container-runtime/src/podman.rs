//! Ephemeral containers executed by a local Podman CLI.
//!
//! The control process and Podman must resolve workspace and panel paths on
//! the same host. Requests are translated to argv only; no shell is involved.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;

use crate::config::{default_panel_cache_root, podman_state_root};
use crate::error::ContainerRuntimeError;
use crate::runtime::{
    ContainerRuntime, request_user_ids, truncate_captured_bytes, validate_run_request,
};
use crate::types::{ContainerRunRequest, ContainerRunResult, PullPolicy};

const CLEANUP_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone)]
pub struct PodmanConfig {
    pub program: String,
    pub workspace_root: std::path::PathBuf,
    pub panel_cache_root: std::path::PathBuf,
}

impl Default for PodmanConfig {
    fn default() -> Self {
        Self::from_env()
    }
}

impl PodmanConfig {
    pub fn from_env() -> Self {
        let root = podman_state_root();
        Self {
            program: env_value("AUTONOMICS_PODMAN_PROGRAM", "podman"),
            workspace_root: env_path("AUTONOMICS_PODMAN_WORKSPACE_ROOT", root.join("workspace")),
            panel_cache_root: env_path("AUTONOMICS_PANEL_CACHE_ROOT", default_panel_cache_root()),
        }
    }
}

fn env_value(key: &str, default: &str) -> String {
    std::env::var_os(key)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| default.to_string())
}

fn env_path(key: &str, default: std::path::PathBuf) -> std::path::PathBuf {
    std::env::var_os(key)
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or(default)
}

pub struct PodmanRuntime {
    config: PodmanConfig,
}

impl PodmanRuntime {
    pub fn new(config: PodmanConfig) -> Self {
        Self { config }
    }

    pub fn from_env() -> Self {
        Self::new(PodmanConfig::from_env())
    }

    pub fn config(&self) -> &PodmanConfig {
        &self.config
    }
}

impl Default for PodmanRuntime {
    fn default() -> Self {
        Self::from_env()
    }
}

#[async_trait]
impl ContainerRuntime for PodmanRuntime {
    async fn run(
        &self,
        request: ContainerRunRequest,
    ) -> Result<ContainerRunResult, ContainerRuntimeError> {
        validate_run_request(&request)?;
        let args = build_create_args(&request)?;
        create_container(&self.config.program, &request, args).await?;
        let mut guard = ContainerCleanupGuard::new(&self.config.program, &request.name);

        let start = Command::new(&self.config.program)
            .arg("start")
            .arg("--attach")
            .arg("--sig-proxy=false")
            .arg(&request.name)
            .kill_on_drop(true)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let child = match start {
            Ok(child) => child,
            Err(error) => {
                let cleanup = remove_container(&self.config.program, &request.name).await;
                guard.disarm();
                let message = format!("cannot start Podman container: {error}");
                return Err(match cleanup {
                    Ok(()) => ContainerRuntimeError::Invalid(message),
                    Err(cleanup_error) => ContainerRuntimeError::Invalid(format!(
                        "{message}; container cleanup failed: {cleanup_error}"
                    )),
                });
            }
        };

        let attached = tokio::time::timeout(
            Duration::from_secs(request.timeout_secs),
            child.wait_with_output(),
        )
        .await;
        let cleanup_error = remove_container(&self.config.program, &request.name)
            .await
            .err();
        guard.disarm();

        match attached {
            Ok(Ok(output)) => {
                let stdout = truncate_captured_bytes(&output.stdout);
                let mut stderr = truncate_captured_bytes(&output.stderr);
                if let Some(error) = cleanup_error {
                    append_capture(&mut stderr, format!("container cleanup failed: {error}"));
                }
                if output.status.success() {
                    Ok(ContainerRunResult {
                        exit_code: 0,
                        stdout,
                        stderr,
                    })
                } else {
                    Err(ContainerRuntimeError::ExitStatus {
                        exit_code: output.status.code().unwrap_or(1),
                        stderr,
                    })
                }
            }
            Ok(Err(error)) => Err(ContainerRuntimeError::Invalid(format!(
                "Podman attach stream failed: {error}{}",
                cleanup_error
                    .map(|error| format!("; container cleanup failed: {error}"))
                    .unwrap_or_default()
            ))),
            Err(_) => Err(ContainerRuntimeError::Timeout {
                timeout_secs: request.timeout_secs,
            }),
        }
    }

    fn name(&self) -> &'static str {
        "podman"
    }

    fn workspace_root(&self) -> &Path {
        self.config.workspace_root.as_path()
    }
}

async fn create_container(
    program: &str,
    request: &ContainerRunRequest,
    args: Vec<String>,
) -> Result<(), ContainerRuntimeError> {
    let created = tokio::time::timeout(
        Duration::from_secs(request.timeout_secs),
        Command::new(program)
            .arg("create")
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output(),
    )
    .await;
    let output = match created {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            return Err(ContainerRuntimeError::Invalid(format!(
                "cannot spawn `{program} create`: {error}"
            )));
        }
        Err(_) => {
            let cleanup = remove_container(program, &request.name).await;
            return Err(match cleanup {
                Ok(()) => ContainerRuntimeError::Timeout {
                    timeout_secs: request.timeout_secs,
                },
                Err(error) => ContainerRuntimeError::Invalid(format!(
                    "Podman create timed out after {}s and container cleanup failed: {error}",
                    request.timeout_secs
                )),
            });
        }
    };
    if !output.status.success() {
        return Err(ContainerRuntimeError::Invalid(format!(
            "`{program} create` failed with status {}: {}",
            output.status.code().unwrap_or(1),
            truncate_captured_bytes(&output.stderr)
        )));
    }
    Ok(())
}

async fn remove_container(program: &str, name: &str) -> Result<(), String> {
    let removed = tokio::time::timeout(
        Duration::from_secs(CLEANUP_TIMEOUT_SECS),
        Command::new(program)
            .arg("rm")
            .arg("--force")
            .arg("--time=0")
            .arg(name)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output(),
    )
    .await;
    match removed {
        Ok(Ok(output)) if output.status.success() => Ok(()),
        Ok(Ok(output)) => Err(truncate_captured_bytes(&output.stderr)),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err("timed out waiting for container removal".into()),
    }
}

struct ContainerCleanupGuard {
    program: String,
    name: String,
    armed: bool,
}

impl ContainerCleanupGuard {
    fn new(program: &str, name: &str) -> Self {
        Self {
            program: program.to_string(),
            name: name.to_string(),
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ContainerCleanupGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let program = self.program.clone();
        let name = self.name.clone();
        std::thread::spawn(move || {
            let _ = std::process::Command::new(program)
                .args(["rm", "--force", "--time=0", &name])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        });
    }
}

fn append_capture(value: &mut String, message: String) {
    if value.is_empty() {
        *value = message;
    } else {
        value.push('\n');
        value.push_str(&message);
    }
}

pub(crate) fn build_create_args(
    request: &ContainerRunRequest,
) -> Result<Vec<String>, ContainerRuntimeError> {
    validate_podman_request(request)?;
    let mut args = vec![
        "--name".into(),
        request.name.clone(),
        "--network".into(),
        podman_network(request)?,
        "--security-opt".into(),
        "no-new-privileges".into(),
        "--tmpfs".into(),
        "/tmp:rw,nosuid,nodev".into(),
        "--pull".into(),
        podman_pull_policy(request.pull_policy).into(),
        "--user".into(),
        user_value(request),
        "--userns".into(),
        "keep-id".into(),
    ];
    if request.read_only_rootfs {
        args.push("--read-only".into());
    }
    if let Some(size) = &request.shm_size {
        args.push("--shm-size".into());
        args.push(size.clone());
    }
    if let Some(cpus) = request.cpus {
        args.push("--cpus".into());
        args.push(cpus.to_string());
    }
    if let Some(memory) = &request.memory {
        args.push("--memory".into());
        args.push(memory.clone());
    }
    if let Some(pids_limit) = request.pids_limit {
        args.push("--pids-limit".into());
        args.push(pids_limit.to_string());
    }
    for (name, value) in &request.env {
        args.push("--env".into());
        args.push(format!("{name}={value}"));
    }
    args.push("--volume".into());
    args.push(format!(
        "{}:{}:rw",
        request.workspace.host_path.to_string_lossy(),
        request.workspace.container_workdir
    ));
    for panel in &request.panels {
        args.push("--volume".into());
        args.push(format!(
            "{}:{}:ro",
            panel.host_path.to_string_lossy(),
            panel.mount_path
        ));
    }
    args.push("--workdir".into());
    args.push(request.workspace.container_workdir.clone());
    args.push(request.image.clone());
    args.extend(request.command.iter().cloned());
    Ok(args)
}

fn validate_podman_request(request: &ContainerRunRequest) -> Result<(), ContainerRuntimeError> {
    if matches!(request.network.as_str(), "cluster") {
        return Err(ContainerRuntimeError::Invalid(
            "network profile `cluster` is not supported by the Podman backend".into(),
        ));
    }
    let workspace_mount = Path::new(&request.workspace.container_workdir);
    if workspace_mount == Path::new("/") {
        return Err(ContainerRuntimeError::Invalid(
            "workspace mount path cannot be `/`".into(),
        ));
    }
    let mut mounts = BTreeSet::new();
    for panel in &request.panels {
        if !panel.host_path.is_absolute() || !panel.host_path.is_dir() {
            return Err(ContainerRuntimeError::Invalid(format!(
                "panel `{}` cache path must be an existing directory",
                panel.id
            )));
        }
        let mount = Path::new(&panel.mount_path);
        if !mount.is_absolute()
            || mount == Path::new("/")
            || !mounts.insert(panel.mount_path.clone())
            || mount.starts_with(workspace_mount)
            || workspace_mount.starts_with(mount)
        {
            return Err(ContainerRuntimeError::Invalid(format!(
                "duplicate or overlapping panel mount path `{}`",
                panel.mount_path
            )));
        }
    }
    Ok(())
}

fn podman_network(request: &ContainerRunRequest) -> Result<String, ContainerRuntimeError> {
    match request.network.as_str() {
        "isolated" | "none" => Ok("none".into()),
        "egress" => Ok("default".into()),
        _ => Err(ContainerRuntimeError::Invalid(format!(
            "network profile `{}` is not supported by the Podman backend",
            request.network
        ))),
    }
}

fn podman_pull_policy(policy: PullPolicy) -> &'static str {
    match policy {
        PullPolicy::Missing => "missing",
        PullPolicy::Always => "always",
        PullPolicy::Newer => "newer",
        PullPolicy::Never => "never",
    }
}

fn user_value(request: &ContainerRunRequest) -> String {
    let (uid, gid) = request_user_ids(request);
    format!("{uid}:{gid}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::unique_container_name;
    use crate::types::{CachedPanel, WorkspaceRef};
    use std::path::PathBuf;

    fn request(network: &str) -> ContainerRunRequest {
        let workspace = tempfile::tempdir().unwrap();
        let workspace_path = workspace.keep();
        ContainerRunRequest {
            image: "docker.io/library/debian:bookworm-slim".into(),
            command: vec![
                "cp".into(),
                "--".into(),
                "$input0".into(),
                "$output0".into(),
            ],
            workspace: WorkspaceRef {
                host_path: workspace_path,
                pvc_sub_path: "runs/test".into(),
                container_workdir: "/work".into(),
            },
            env: vec![("EXAMPLE".into(), "value".into())],
            panels: Vec::new(),
            network: network.into(),
            read_only_rootfs: true,
            pull_policy: PullPolicy::Never,
            cpus: Some(2.0),
            memory: Some("1Gi".into()),
            pids_limit: Some(512),
            shm_size: Some("64Mi".into()),
            user: Some("1000:1000".into()),
            timeout_secs: 60,
            name: "podman-test".into(),
        }
    }

    #[test]
    fn create_args_map_security_data_plane_and_resources() {
        let args = build_create_args(&request("isolated")).unwrap();
        let expected = [
            "--name",
            "podman-test",
            "--network",
            "none",
            "--security-opt",
            "no-new-privileges",
            "--tmpfs",
            "/tmp:rw,nosuid,nodev",
            "--pull",
            "never",
            "--user",
            "1000:1000",
            "--userns",
            "keep-id",
            "--read-only",
            "--shm-size",
            "64Mi",
            "--cpus",
            "2",
            "--memory",
            "1Gi",
            "--pids-limit",
            "512",
            "--env",
            "EXAMPLE=value",
            "--volume",
        ];
        for (index, value) in expected.iter().enumerate() {
            assert_eq!(&args[index], value);
        }
        let image_index = args
            .iter()
            .position(|arg| arg == "docker.io/library/debian:bookworm-slim")
            .unwrap();
        assert_eq!(args[image_index + 1], "cp");
        assert_eq!(args.last().unwrap(), "$output0");
        assert!(
            args.windows(2)
                .any(|args| args[0].starts_with('/') && args[0].ends_with(":/work:rw"))
        );
    }

    #[test]
    fn cluster_network_is_rejected_and_egress_uses_default() {
        assert!(build_create_args(&request("cluster")).is_err());
        let args = build_create_args(&request("egress")).unwrap();
        let network = args
            .windows(2)
            .find(|args| args[0] == "--network")
            .map(|args| args[1].clone())
            .unwrap();
        assert_eq!(network, "default");
    }

    #[test]
    fn panel_mounts_must_exist_and_not_overlap_workspace() {
        let panel = tempfile::tempdir().unwrap();
        let mut valid = request("isolated");
        valid.panels.push(CachedPanel {
            id: "panel".into(),
            digest: format!("sha256:{}", "1".repeat(64)),
            host_path: panel.keep(),
            pvc_sub_path: "panels/test".into(),
            mount_path: "/panels/test".into(),
        });
        assert!(build_create_args(&valid).is_ok());

        valid.panels[0].mount_path = "/work/panel".into();
        assert!(build_create_args(&valid).is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_force_removes_created_container() {
        let root = tempfile::tempdir().unwrap();
        let program = root.path().join("fake-podman");
        std::fs::write(
            &program,
            "#!/bin/sh\ncase \"$1\" in\n  create) exit 0 ;;\n  start) exec sleep 30 ;;\n  rm) printf '%s\\n' \"$*\" > \"${0}.removed\"; exit 0 ;;\n  *) exit 2 ;;\nesac\n",
        )
        .unwrap();
        make_executable(&program);
        let marker = root.path().join("fake-podman.removed");
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let mut request = request("isolated");
        request.workspace.host_path = workspace.clone();
        request.name = unique_container_name();
        request.timeout_secs = 1;

        let runtime = PodmanRuntime::new(PodmanConfig {
            program: program.to_string_lossy().into_owned(),
            workspace_root: workspace,
            panel_cache_root: root.path().join("panels"),
        });
        let error = runtime.run(request).await;

        assert!(matches!(
            error.unwrap_err(),
            ContainerRuntimeError::Timeout { timeout_secs: 1 }
        ));
        assert!(marker.is_file());
    }

    #[tokio::test]
    #[ignore = "requires a working rootless Podman runtime and may pull an OCI image"]
    async fn real_podman_copies_workspace_file() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("input.txt"), "podman-backend").unwrap();
        let runtime = PodmanRuntime::new(PodmanConfig {
            program: std::env::var("AUTONOMICS_PODMAN_PROGRAM").unwrap_or_else(|_| "podman".into()),
            workspace_root: workspace.clone(),
            panel_cache_root: root.path().join("panels"),
        });
        let request = ContainerRunRequest {
            image: std::env::var("AUTONOMICS_CONTAINER_IT_IMAGE")
                .unwrap_or_else(|_| "docker.io/library/debian:bookworm-slim".into()),
            command: vec![
                "cp".into(),
                "--".into(),
                "/work/input.txt".into(),
                "/work/output.txt".into(),
            ],
            workspace: WorkspaceRef {
                host_path: workspace.clone(),
                pvc_sub_path: "workspace".into(),
                container_workdir: "/work".into(),
            },
            env: Vec::new(),
            panels: Vec::new(),
            network: "isolated".into(),
            read_only_rootfs: true,
            pull_policy: PullPolicy::Missing,
            cpus: None,
            memory: None,
            pids_limit: None,
            shm_size: None,
            user: None,
            timeout_secs: 120,
            name: unique_container_name(),
        };

        runtime.run(request).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(workspace.join("output.txt")).unwrap(),
            "podman-backend"
        );
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).unwrap();
    }
}
