//! Long-lived development containers and exec streams — the
//! devcontainer-style half of the connection layer.
//!
//! The ordinary run path (`PodmanConnection::run`) is deliberately
//! ephemeral: one request maps to one container that a cleanup guard
//! force-removes on success, failure, or timeout. A development window
//! needs the opposite contract, modeled after the VSCode devcontainer
//! flow that has proven out at scale:
//!
//! - a **named, labeled** container per plugin workspace
//!   (`io.autonomics.dev-session=<plugin>`, the analogue of VSCode's
//!   `devcontainer.local_folder` label) whose lifecycle is explicit
//!   (idempotent start, operator-driven stop, restart that never touches
//!   the bind-mounted host workspace);
//! - an idle entrypoint (`sleep infinity`) so the container stays up
//!   between interactions;
//! - **exec, not cold-start run**, for every command — VSCode issues all
//!   one-shot commands through `docker exec` into the resident
//!   container, and we do the same through `podman exec`;
//! - a writable rootfs (package installs) with the run path's other
//!   hardening kept: `no-new-privileges`, `--userns keep-id`, tmpfs
//!   `/tmp`, explicit network policy (isolated unless the operator opts
//!   into egress).
//!
//! The workspace is the single read-write host mount; "sync" is the bind
//! mount itself, so removing or restarting the container can never lose
//! project files.

use crate::error::ContainerRuntimeError;
use crate::types::{ContainerNetwork, WorkspaceRef};

/// Label key associating a dev container with its plugin workspace
/// (VSCode's `devcontainer.local_folder` pattern: ownership travels with
/// the label, not just the container name).
pub const DEV_SESSION_LABEL: &str = "io.autonomics.dev-session";

/// Container name prefix for autonomics development sessions.
pub const DEV_CONTAINER_PREFIX: &str = "autonomics-dev-";

/// One long-lived interactive development container. Named and labeled,
/// never auto-removed; restart (`remove` + `start`) leaves the
/// bind-mounted host workspace untouched.
#[derive(Debug, Clone)]
pub struct DevContainerSpec {
    /// Exact container name, e.g. `autonomics-dev-my-plugin`.
    pub name: String,
    /// Image reference (digest-pinned by the caller).
    pub image: String,
    /// The plugin workspace, bind-mounted read-write.
    pub workspace: WorkspaceRef,
    /// Network policy; egress is an explicit operator opt-in.
    pub network: ContainerNetwork,
    pub cpus: Option<f64>,
    /// Kubernetes-style quantity (`2Gi`) translated to exact bytes.
    pub memory: Option<String>,
    pub pids_limit: Option<u64>,
    pub env: Vec<(String, String)>,
    /// Ownership labels; callers must always set [`DEV_SESSION_LABEL`].
    pub labels: Vec<(String, String)>,
}

/// Podman-side lifecycle state of a dev container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DevContainerState {
    /// No container with that name exists.
    NotFound,
    /// Created but not currently running.
    Created,
    Running,
    /// Ran and stopped.
    Exited,
}

impl DevContainerState {
    /// State as reported to API consumers.
    pub fn as_str(self) -> &'static str {
        match self {
            DevContainerState::NotFound => "absent",
            DevContainerState::Created => "created",
            DevContainerState::Running => "running",
            DevContainerState::Exited => "exited",
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DevContainerStatus {
    pub state: DevContainerState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_id: Option<String>,
}

/// A live bidirectional `podman exec` stream inside a dev container.
///
/// Dropping the struct drops the child (kill-on-drop), which closes the
/// exec session; stdin/stdout remain directly readable/writable while it
/// is alive.
pub struct DevExecStream {
    pub stdin: tokio::process::ChildStdin,
    pub stdout: tokio::process::ChildStdout,
    /// Present when the exec was not allocated a tty (with `--tty`,
    /// podman merges stderr into the pty stream).
    pub stderr: Option<tokio::process::ChildStderr>,
    /// Resolves with the exec's exit code once the process exits.
    pub exit: tokio::task::JoinHandle<Result<i32, ContainerRuntimeError>>,
}

/// Build the `podman create` argv for a dev container. Pure and
/// unit-tested, mirroring `build_create_args_for_policies`.
pub(crate) fn build_dev_create_args(spec: &DevContainerSpec) -> Result<Vec<String>, ContainerRuntimeError> {
    validate_dev_spec(spec)?;
    let mut args = vec![
        "--name".into(),
        spec.name.clone(),
        "--network".into(),
        match spec.network {
            ContainerNetwork::Isolated => "none".to_string(),
            ContainerNetwork::Egress => "default".to_string(),
        },
        // Run-path hardening that a dev container keeps.
        "--security-opt".into(),
        "no-new-privileges".into(),
        "--tmpfs".into(),
        "/tmp:rw,nosuid,nodev".into(),
        "--pull".into(),
        "missing".into(),
        "--user".into(),
        current_user_value(),
        "--userns".into(),
        "keep-id".into(),
    ];
    // Deliberately NO `--read-only`: a development container installs
    // packages and writes tool caches into its rootfs. The workspace is
    // still the only read-write HOST mount.
    for (key, value) in &spec.labels {
        args.push("--label".into());
        args.push(format!("{key}={value}"));
    }
    if let Some(cpus) = spec.cpus {
        args.push("--cpus".into());
        args.push(cpus.to_string());
    }
    if let Some(memory) = &spec.memory {
        args.push("--memory".into());
        args.push(crate::podman::podman_size(memory)?);
    }
    if let Some(pids_limit) = spec.pids_limit {
        args.push("--pids-limit".into());
        args.push(pids_limit.to_string());
    }
    for (name, value) in &spec.env {
        args.push("--env".into());
        args.push(format!("{name}={value}"));
    }
    args.push("--mount".into());
    args.push(format!(
        "type=bind,source={},destination={},rw=true",
        crate::podman::escape_mount_field(&spec.workspace.host_path.to_string_lossy()),
        crate::podman::escape_mount_field(&spec.workspace.container_workdir),
    ));
    args.push("--workdir".into());
    args.push(spec.workspace.container_workdir.clone());
    // Idle entrypoint: the container's job is to exist; interaction all
    // happens through exec.
    args.push("--entrypoint".into());
    args.push("sleep".into());
    args.push(spec.image.clone());
    args.push("infinity".into());
    Ok(args)
}

fn validate_dev_spec(spec: &DevContainerSpec) -> Result<(), ContainerRuntimeError> {
    if spec.name.is_empty()
        || !spec
            .name
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        || !spec
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    {
        return Err(ContainerRuntimeError::Invalid(format!(
            "dev container name `{}` must be alphanumeric/._- and start with a letter or digit",
            spec.name
        )));
    }
    if spec.image.trim().is_empty() {
        return Err(ContainerRuntimeError::Invalid(
            "dev container `image` cannot be empty".into(),
        ));
    }
    if !spec.workspace.host_path.is_absolute()
        || !spec.workspace.host_path.is_dir()
        || !spec.workspace.container_workdir.starts_with('/')
        || spec.workspace.container_workdir == "/"
    {
        return Err(ContainerRuntimeError::Invalid(
            "dev workspace must be an existing directory mapped to an absolute non-root container path"
                .into(),
        ));
    }
    if let Some(cpus) = spec.cpus
        && cpus <= 0.0
    {
        return Err(ContainerRuntimeError::Invalid(
            "dev container `cpus` must be positive".into(),
        ));
    }
    if let Some(pids_limit) = spec.pids_limit
        && pids_limit == 0
    {
        return Err(ContainerRuntimeError::Invalid(
            "dev container `pids_limit` must be positive".into(),
        ));
    }
    Ok(())
}

fn current_user_value() -> String {
    #[cfg(unix)]
    {
        let uid = unsafe { libc::getuid() } as i64;
        let gid = unsafe { libc::getgid() } as i64;
        format!("{uid}:{gid}")
    }
    #[cfg(not(unix))]
    {
        "1000:1000".to_string()
    }
}

/// Parse `podman ps -a --format {{.ID}}\t{{.State}}\t{{.Names}}` output
/// and return the row whose name equals `name` exactly. Podman's
/// `--filter name=` matches substrings, so the exact match happens here.
pub(crate) fn exact_container_row(ps_output: &str, name: &str) -> Option<(String, DevContainerState)> {
    ps_output.lines().find_map(|line| {
        let mut columns = line.split('\t');
        let id = columns.next()?.trim();
        let state = columns.next()?.trim();
        let names = columns.next()?.trim();
        // `ps` may report multiple names (aliases) space-separated.
        if names.split_whitespace().any(|candidate| candidate == name) {
            Some((id.to_string(), ps_state(state)?))
        } else {
            None
        }
    })
}

fn ps_state(raw: &str) -> Option<DevContainerState> {
    // Configured exists after `create` but before `start`; `ps -a` may
    // report it as Created, Exited (after stop), Running, or Degraded
    // ( restarting ) — the dev surface folds uncommon ones into Exited.
    match raw {
        "Running" | "Degraded" => Some(DevContainerState::Running),
        "Created" => Some(DevContainerState::Created),
        "Exited" | "Stopped" | "Dead" | "Paused" => Some(DevContainerState::Exited),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn spec_fixture() -> DevContainerSpec {
        let workspace = tempfile::tempdir().unwrap();
        let host_path = workspace.path().to_path_buf();
        std::mem::forget(workspace);
        DevContainerSpec {
            name: "autonomics-dev-my-plugin".into(),
            image: "ghcr.io/auto-nomics/environments/r:4.5.1@sha256:0123456789abcdef".into(),
            workspace: WorkspaceRef {
                host_path,
                container_workdir: "/work".into(),
            },
            network: ContainerNetwork::Isolated,
            cpus: Some(4.0),
            memory: Some("2Gi".into()),
            pids_limit: Some(512),
            env: vec![("TERM".into(), "xterm-256color".into())],
            labels: vec![(DEV_SESSION_LABEL.into(), "my-plugin".into())],
        }
    }

    #[test]
    fn create_args_match_the_devcontainer_contract() {
        let spec = spec_fixture();
        let args = build_dev_create_args(&spec).unwrap();
        let joined = args.join(" ");
        assert!(joined.contains("--name autonomics-dev-my-plugin"));
        assert!(joined.contains("--network none"));
        assert!(joined.contains("--security-opt no-new-privileges"));
        assert!(joined.contains("--tmpfs /tmp:rw,nosuid,nodev"));
        assert!(joined.contains("--userns keep-id"));
        // Writable rootfs: no --read-only anywhere.
        assert!(!joined.contains("--read-only"));
        assert!(joined.contains(&format!("--label {DEV_SESSION_LABEL}=my-plugin")));
        assert!(joined.contains("--cpus 4"));
        // Kubernetes quantity translated to exact bytes (2Gi).
        assert!(joined.contains("--memory 2147483648"));
        assert!(joined.contains("--pids-limit 512"));
        assert!(joined.contains("--env TERM=xterm-256color"));
        assert!(joined.contains(&format!(
            "type=bind,source={},destination=/work,rw=true",
            spec.workspace.host_path.to_string_lossy()
        )));
        assert!(joined.contains("--workdir /work"));
        // Idle entrypoint: the container's only job is to exist.
        let n = args.len();
        assert!(args[n - 4] == "--entrypoint");
        assert_eq!(args[n - 3], "sleep");
        assert!(args[n - 2].starts_with("ghcr.io/"));
        assert_eq!(args[n - 1], "infinity");
    }

    #[test]
    fn egress_network_uses_the_default_network() {
        let mut spec = spec_fixture();
        spec.network = ContainerNetwork::Egress;
        let joined = build_dev_create_args(&spec).unwrap().join(" ");
        assert!(joined.contains("--network default"));
    }

    #[test]
    fn invalid_specs_are_rejected() {
        let mut spec = spec_fixture();
        spec.name = "bad name!".into();
        assert!(build_dev_create_args(&spec).is_err());
        spec = spec_fixture();
        spec.image = "  ".into();
        assert!(build_dev_create_args(&spec).is_err());
        spec = spec_fixture();
        spec.workspace.container_workdir = "/".into();
        assert!(build_dev_create_args(&spec).is_err());
        spec = spec_fixture();
        spec.cpus = Some(0.0);
        assert!(build_dev_create_args(&spec).is_err());
        spec = spec_fixture();
        spec.memory = Some("not-a-quantity".into());
        assert!(build_dev_create_args(&spec).is_err());
    }

    #[test]
    fn ps_rows_match_exactly_not_by_substring() {
        let output = "abc123\tRunning\tautonomics-dev-my-plugin\ndef456\tExited\tautonomics-dev-my-plugin-old\n";
        let (id, state) = exact_container_row(output, "autonomics-dev-my-plugin").unwrap();
        assert_eq!(id, "abc123");
        assert_eq!(state, DevContainerState::Running);
        // A longer name sharing the prefix must not match.
        assert!(exact_container_row(output, "autonomics-dev-my").is_none());
        assert_eq!(
            exact_container_row("abc123\tCreated\tx", "x").map(|(_, s)| s),
            Some(DevContainerState::Created),
        );
        assert_eq!(
            exact_container_row("abc123\tExited\tx", "x").map(|(_, s)| s),
            Some(DevContainerState::Exited),
        );
    }
}

/// End-to-end dev-container lifecycle against a real rootless Podman:
/// idempotent start → exec echo → stop (kept) → restart → remove, and
/// the bind-mounted workspace survives every transition (the
/// devcontainer contract: restart never loses project files).
#[cfg(test)]
mod e2e_tests {
    use super::*;
    use crate::connection::PodmanConnection;
    use crate::podman::{PodmanConfig, PodmanRuntime};

    fn runtime() -> (PodmanRuntime, tempfile::TempDir) {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("project.txt"), "plugin project").unwrap();
        let runtime = PodmanRuntime::new(PodmanConfig {
            program: std::env::var("AUTONOMICS_PODMAN_PROGRAM")
                .unwrap_or_else(|_| "podman".into()),
            workspace_root: root.path().join("runs"),
            panel_cache_root: root.path().join("panels"),
            storage_root: None,
            run_root: None,
            storage_driver: None,
            cgroup_manager: None,
            cgroup_parent: None,
            inherit_user_namespace: false,
            disable_swap: false,
            worker_runtime_dir: None,
            worker_resource_control: None,
            worker_user_namespace: None,
            worker_oci_runtime: None,
        });
        (runtime, root)
    }

    fn spec_for(root: &std::path::Path, name: &str) -> DevContainerSpec {
        DevContainerSpec {
            name: name.into(),
            image: std::env::var("AUTONOMICS_CONTAINER_IT_IMAGE")
                .unwrap_or_else(|_| "docker.io/library/debian:bookworm-slim".into()),
            workspace: WorkspaceRef {
                host_path: root.join("workspace"),
                container_workdir: "/work".into(),
            },
            network: ContainerNetwork::Isolated,
            cpus: Some(2.0),
            memory: Some("1Gi".into()),
            pids_limit: Some(256),
            env: Vec::new(),
            labels: vec![(DEV_SESSION_LABEL.into(), "e2e-plugin".into())],
        }
    }

    #[tokio::test]
    #[ignore = "requires a working rootless Podman runtime and may pull an OCI image"]
    async fn dev_container_lifecycle_preserves_workspace() {
        let (runtime, root) = runtime();
        let name = format!("autonomics-dev-e2e-{}", std::process::id());
        let spec = spec_for(root.path(), &name);

        // Start twice: the second call is an idempotent no-op.
        let status = runtime.dev_container_start(&spec).await.unwrap();
        assert_eq!(status.state, DevContainerState::Running);
        let again = runtime.dev_container_start(&spec).await.unwrap();
        assert_eq!(again.state, DevContainerState::Running);

        // Exec a command in the resident container (VSCode-style: no
        // cold-start run) and write into the bind-mounted workspace.
        let mut stream = runtime
            .dev_exec(
                &name,
                &[
                    "sh".to_string(),
                    "-c".to_string(),
                    "cat /work/project.txt > /work/from-container.txt".to_string(),
                ],
                &[],
                false,
            )
            .await
            .unwrap();
        use tokio::io::AsyncWriteExt;
        let _ = stream.stdin.shutdown().await;
        let mut stdout = tokio::io::BufReader::new(stream.stdout);
        let mut captured = String::new();
        use tokio::io::AsyncReadExt;
        let _ = stdout.read_to_string(&mut captured).await;
        let code = stream.exit.await.unwrap().unwrap();
        assert_eq!(code, 0, "exec failed; stdout so far: {captured}");
        assert_eq!(
            std::fs::read_to_string(root.path().join("workspace/from-container.txt")).unwrap(),
            "plugin project"
        );

        // Stop keeps the container; start revives it.
        runtime.dev_container_stop(&name).await.unwrap();
        let stopped = runtime.dev_container_status(&name).await.unwrap();
        assert_eq!(stopped.state, DevContainerState::Exited);
        runtime.dev_container_start(&spec).await.unwrap();
        let revived = runtime.dev_container_status(&name).await.unwrap();
        assert_eq!(revived.state, DevContainerState::Running);

        // Remove is idempotent and never touches the workspace.
        runtime.dev_container_remove(&name).await.unwrap();
        runtime.dev_container_remove(&name).await.unwrap();
        let gone = runtime.dev_container_status(&name).await.unwrap();
        assert_eq!(gone.state, DevContainerState::NotFound);
        assert!(root.path().join("workspace/project.txt").is_file());
        assert!(root.path().join("workspace/from-container.txt").is_file());
    }
}
