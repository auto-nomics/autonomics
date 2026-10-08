//! Ephemeral containers executed by a local Podman CLI.
//!
//! The control process and Podman must resolve workspace and panel paths on
//! the same host. Requests are translated to argv only; no shell is involved.
//!
//! Every Podman invocation detaches stdin (`Stdio::null()`). An attached
//! `podman start` consumes inherited stdin even when the container was
//! created without `--interactive`, which would steal terminal input from
//! whatever host process (e.g. a TUI) shares that tty.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;

use crate::config::{default_panel_cache_root, podman_state_root};
use crate::connection::{
    PodmanConnection, request_user_ids, truncate_captured_bytes, validate_run_request,
};
use crate::error::ContainerRuntimeError;
use crate::types::{
    ContainerNetwork, ContainerRunRequest, ContainerRunResult, GpuRequest, PullPolicy,
};

const CLEANUP_TIMEOUT_SECS: u64 = 30;
const HOST_CWD: &str = "/";

/// Budget for `podman create`, which also performs the image pull when the
/// image is absent locally. It is deliberately decoupled from
/// [`ContainerRunRequest::timeout_secs`]: a first pull on a fresh host can take
/// far longer than a node's execution budget, especially for multi-GB images.
const DEFAULT_PULL_TIMEOUT_SECS: u64 = 3600;
const PULL_TIMEOUT_ENV: &str = "AUTONOMICS_PODMAN_PULL_TIMEOUT_SECS";

/// Podman's SQLite-backed container store wedges permanently when several
/// `create` invocations (each of which may also pull) race each other, so
/// storage-mutating creates are serialized process-wide. `start --attach`
/// stays parallel: by then the container already exists.
static CREATE_GATE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

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
impl PodmanConnection for PodmanRuntime {
    async fn run(
        &self,
        request: ContainerRunRequest,
    ) -> Result<ContainerRunResult, ContainerRuntimeError> {
        validate_run_request(&request)?;
        let args = build_create_args(&request)?;
        create_container(&self.config.program, &request, args).await?;
        let mut guard = ContainerCleanupGuard::new(&self.config.program, &request.name);

        let mut start = async_podman_command(&self.config.program);
        start
            .arg("start")
            .arg("--attach")
            .arg("--sig-proxy=false")
            .arg(&request.name)
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let start = start.spawn();
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
                        stdout,
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
    let timeout_secs = pull_timeout_secs();
    // Queue here, before the timeout below: a queued create must not burn its
    // pull budget while another create is pulling multi-GB layers.
    let _create_permit = CREATE_GATE.acquire().await.map_err(|error| {
        ContainerRuntimeError::Invalid(format!("podman create gate closed: {error}"))
    })?;
    let created = tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        async_podman_command(program)
            .arg("create")
            .args(args)
            .stdin(Stdio::null())
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
                Ok(()) => ContainerRuntimeError::Timeout { timeout_secs },
                Err(error) => ContainerRuntimeError::Invalid(format!(
                    "Podman create timed out after {}s and container cleanup failed: {error}",
                    timeout_secs
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
        async_podman_command(program)
            .arg("rm")
            .arg("--force")
            .arg("--time=0")
            .arg(name)
            .stdin(Stdio::null())
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
            let _ = blocking_podman_command(&program)
                .args(["rm", "--force", "--time=0", &name])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        });
    }
}

pub(crate) fn async_podman_command(program: &str) -> Command {
    let mut command = Command::new(program);
    command.current_dir(HOST_CWD);
    command
}

fn blocking_podman_command(program: &str) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    command.current_dir(HOST_CWD);
    command
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
        podman_network(request).into(),
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
        args.push(podman_size(size)?);
    }
    if let Some(cpus) = request.cpus {
        args.push("--cpus".into());
        args.push(cpus.to_string());
    }
    if let Some(memory) = &request.memory {
        args.push("--memory".into());
        args.push(podman_size(memory)?);
    }
    if let Some(pids_limit) = request.pids_limit {
        args.push("--pids-limit".into());
        args.push(pids_limit.to_string());
    }
    if let Some(gpus) = request.gpus.podman_value() {
        args.push("--gpus".into());
        args.push(gpus);
    }
    for (name, value) in &request.env {
        args.push("--env".into());
        args.push(format!("{name}={value}"));
    }
    args.push("--mount".into());
    args.push(format!(
        "type=bind,source={},destination={},rw=true",
        escape_mount_field(&request.workspace.host_path.to_string_lossy()),
        escape_mount_field(&request.workspace.container_workdir)
    ));
    for panel in &request.panels {
        args.push("--mount".into());
        args.push(format!(
            "type=bind,source={},destination={},readonly",
            escape_mount_field(&panel.host_path.to_string_lossy()),
            escape_mount_field(&panel.mount_path)
        ));
    }
    args.push("--workdir".into());
    args.push(request.workspace.container_workdir.clone());
    args.push("--entrypoint".into());
    args.push(
        request
            .command
            .first()
            .cloned()
            .ok_or_else(|| ContainerRuntimeError::Invalid("`command` cannot be empty".into()))?,
    );
    args.push(request.image.clone());
    args.extend(request.command.iter().skip(1).cloned());
    Ok(args)
}

fn escape_mount_field(value: &str) -> String {
    value.replace('\\', "\\\\").replace(',', "\\,")
}

/// Podman accepts bytes or decimal k/m/g suffixes, while container nodes use
/// Kubernetes quantities such as `16Gi`. Emit exact bytes so both SI and IEC
/// quantities keep their requested size.
fn podman_size(value: &str) -> Result<String, ContainerRuntimeError> {
    let invalid = || {
        ContainerRuntimeError::Invalid(format!(
            "`{value}` is not a supported Podman memory quantity"
        ))
    };
    let split = value
        .find(|character: char| !(character.is_ascii_digit() || character == '.'))
        .unwrap_or(value.len());
    let (number, suffix) = value.split_at(split);
    let number: f64 = number.parse().map_err(|_| invalid())?;
    if !number.is_finite() || number <= 0.0 {
        return Err(invalid());
    }

    let multiplier = match suffix.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1_u64,
        "k" => 1_000,
        "m" => 1_000_000,
        "g" => 1_000_000_000,
        "t" => 1_000_000_000_000,
        "ki" => 1 << 10,
        "mi" => 1 << 20,
        "gi" => 1 << 30,
        "ti" => 1 << 40,
        _ => return Err(invalid()),
    };
    let bytes = number * multiplier as f64;
    if !bytes.is_finite() || bytes <= 0.0 || bytes > u64::MAX as f64 {
        return Err(invalid());
    }
    Ok(format!("{}", bytes as u64))
}

fn validate_podman_request(request: &ContainerRunRequest) -> Result<(), ContainerRuntimeError> {
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

fn podman_network(request: &ContainerRunRequest) -> &'static str {
    match request.network {
        ContainerNetwork::Isolated => "none",
        ContainerNetwork::Egress => "default",
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

/// Budget for `podman create`, which performs the initial image pull when the
/// image is absent. Tunable through `AUTONOMICS_PODMAN_PULL_TIMEOUT_SECS`.
fn pull_timeout_secs() -> u64 {
    let value = std::env::var(PULL_TIMEOUT_ENV).ok();
    pull_timeout_from(value.as_deref())
}

fn pull_timeout_from(value: Option<&str>) -> u64 {
    value
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_PULL_TIMEOUT_SECS)
}

fn user_value(request: &ContainerRunRequest) -> String {
    let (uid, gid) = request_user_ids(request);
    format!("{uid}:{gid}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::unique_container_name;
    use crate::types::{CachedPanel, WorkspaceRef};
    use std::path::PathBuf;

    fn request(network: ContainerNetwork) -> ContainerRunRequest {
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
                container_workdir: "/work".into(),
            },
            env: vec![("EXAMPLE".into(), "value".into())],
            panels: Vec::new(),
            network,
            read_only_rootfs: true,
            pull_policy: PullPolicy::Never,
            cpus: Some(2.0),
            memory: Some("1Gi".into()),
            pids_limit: Some(512),
            shm_size: Some("64Mi".into()),
            gpus: GpuRequest::None,
            user: Some("1000:1000".into()),
            timeout_secs: 60,
            name: "podman-test".into(),
        }
    }

    #[test]
    fn podman_commands_do_not_inherit_daemon_cwd() {
        assert_eq!(
            blocking_podman_command("podman").get_current_dir(),
            Some(std::path::Path::new("/"))
        );
    }

    #[test]
    fn create_args_map_security_data_plane_and_resources() {
        let args = build_create_args(&request(ContainerNetwork::Isolated)).unwrap();
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
            "67108864",
            "--cpus",
            "2",
            "--memory",
            "1073741824",
            "--pids-limit",
            "512",
            "--env",
            "EXAMPLE=value",
            "--mount",
        ];
        for (index, value) in expected.iter().enumerate() {
            assert_eq!(&args[index], value);
        }
        let image_index = args
            .iter()
            .position(|arg| arg == "docker.io/library/debian:bookworm-slim")
            .unwrap();
        assert_eq!(args[image_index - 2], "--entrypoint");
        assert_eq!(args[image_index - 1], "cp");
        assert_eq!(args.last().unwrap(), "$output0");
        assert!(
            args.windows(2)
                .any(|args| args[0].starts_with("type=bind,source=")
                    && args[0].ends_with(",destination=/work,rw=true"))
        );
    }

    #[test]
    fn podman_sizes_convert_kubernetes_quantities_to_bytes() {
        assert_eq!(podman_size("16Gi").unwrap(), "17179869184");
        assert_eq!(podman_size("64Mi").unwrap(), "67108864");
        assert_eq!(podman_size("1G").unwrap(), "1000000000");
        assert_eq!(podman_size("512").unwrap(), "512");
        assert!(podman_size("16gi ").is_ok());
        assert!(podman_size("-1Gi").is_err());
        assert!(podman_size("1Xi").is_err());
        assert!(podman_size("Gi").is_err());
    }

    #[test]
    fn pull_timeout_uses_default_for_unset_invalid_or_zero_values() {
        assert_eq!(pull_timeout_from(None), DEFAULT_PULL_TIMEOUT_SECS);
        assert_eq!(pull_timeout_from(Some("")), DEFAULT_PULL_TIMEOUT_SECS);
        assert_eq!(pull_timeout_from(Some("0")), DEFAULT_PULL_TIMEOUT_SECS);
        assert_eq!(
            pull_timeout_from(Some("not-a-number")),
            DEFAULT_PULL_TIMEOUT_SECS
        );
    }

    #[test]
    fn pull_timeout_accepts_explicit_override() {
        assert_eq!(pull_timeout_from(Some("7200")), 7200);
        assert_eq!(pull_timeout_from(Some(" 1800 ")), 1800);
    }

    #[test]
    fn default_pull_timeout_is_generous_enough_for_multi_gigabyte_images() {
        // Compile-time check in the `const _` form: `assert_eq!` inside an
        // inline const block needs a const `assert_failed`, which the pinned
        // 1.96 toolchain does not provide yet.
        const _: () = assert!(DEFAULT_PULL_TIMEOUT_SECS == 3600);
    }

    #[test]
    fn isolated_disables_networking_and_egress_uses_default() {
        let isolated = build_create_args(&request(ContainerNetwork::Isolated)).unwrap();
        let isolated_network = isolated
            .windows(2)
            .find(|args| args[0] == "--network")
            .map(|args| args[1].clone())
            .unwrap();
        assert_eq!(isolated_network, "none");

        let egress = build_create_args(&request(ContainerNetwork::Egress)).unwrap();
        let egress_network = egress
            .windows(2)
            .find(|args| args[0] == "--network")
            .map(|args| args[1].clone())
            .unwrap();
        assert_eq!(egress_network, "default");
    }

    #[test]
    fn gpu_requests_add_gpus_flag_only_when_present() {
        let without = build_create_args(&request(ContainerNetwork::Isolated)).unwrap();
        assert!(!without.iter().any(|arg| arg == "--gpus"));

        let mut with = request(ContainerNetwork::Isolated);
        with.gpus = GpuRequest::All;
        let args = build_create_args(&with).unwrap();
        let flag = args
            .windows(2)
            .find(|args| args[0] == "--gpus")
            .map(|args| args[1].clone())
            .unwrap();
        assert_eq!(flag, "all");

        with.gpus = GpuRequest::Devices("0,2".into());
        let args = build_create_args(&with).unwrap();
        let flag = args
            .windows(2)
            .find(|args| args[0] == "--gpus")
            .map(|args| args[1].clone())
            .unwrap();
        assert_eq!(flag, "device=0,2");
    }

    #[test]
    fn panel_mounts_must_exist_and_not_overlap_workspace() {
        let panel = tempfile::tempdir().unwrap();
        let mut valid = request(ContainerNetwork::Isolated);
        let cache_path = panel.path().join("ldsc@sha256:a9efab57");
        std::fs::create_dir_all(&cache_path).unwrap();
        valid.panels.push(CachedPanel {
            id: "panel".into(),
            digest: format!("sha256:{}", "1".repeat(64)),
            host_path: cache_path,
            mount_path: "/panels/test".into(),
        });
        let args = build_create_args(&valid).unwrap();
        assert!(!args.iter().any(|arg| arg == "--volume"));
        assert!(
            args.windows(2)
                .any(|args| args[0].starts_with("type=bind,source=")
                    && args[0].contains("@sha256:a9efab57")
                    && args[0].ends_with(",destination=/panels/test,readonly"))
        );

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
            "#!/bin/sh\ncase \"$1\" in\n  create) pwd -P > \"${0}.cwd\"; exit 0 ;;\n  start) exec sleep 30 ;;\n  rm) printf '%s\\n' \"$*\" > \"${0}.removed\"; exit 0 ;;\n  *) exit 2 ;;\nesac\n",
        )
        .unwrap();
        make_executable(&program);
        let marker = root.path().join("fake-podman.removed");
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let mut request = request(ContainerNetwork::Isolated);
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
        assert_eq!(
            std::fs::read_to_string(root.path().join("fake-podman.cwd")).unwrap(),
            "/\n"
        );
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
                container_workdir: "/work".into(),
            },
            env: Vec::new(),
            panels: Vec::new(),
            network: ContainerNetwork::Isolated,
            read_only_rootfs: true,
            pull_policy: PullPolicy::Missing,
            cpus: None,
            memory: None,
            pids_limit: None,
            shm_size: None,
            gpus: GpuRequest::None,
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
