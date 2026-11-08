//! Ephemeral containers executed by a local Podman CLI.
//!
//! The control process and Podman must resolve workspace and panel paths on
//! the same host. Requests are translated to argv only; no shell is involved.
//!
//! Every Podman invocation detaches stdin (`Stdio::null()`). An attached
//! `podman start` consumes inherited stdin even when the container was
//! created without `--interactive`, which would steal terminal input from
//! whatever host process (e.g. a TUI) shares that tty. The single
//! exception is `dev_exec`: its piped streams belong to a gateway
//! WebSocket bridge that never touches a host tty, so interactive stdin
//! is safe there (see the development-container section below).

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
    ContainerNetwork, ContainerRunRequest, ContainerRunResult, GpuRequest, PrivateEnvironment,
    PullPolicy,
};

const CLEANUP_TIMEOUT_SECS: u64 = 30;
const HOST_CWD: &str = "/";

/// Podman's SQLite-backed container store wedges permanently when several
/// `create` invocations (each of which may also pull) race each other, so
/// storage-mutating creates are serialized process-wide. `start --attach`
/// stays parallel: by then the container already exists.
static CREATE_GATE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

/// Explicit operator choice for a private store. No backend auto-detection or
/// fallback may silently change a frozen formal execution policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivateStorageDriver {
    Vfs,
    Overlay,
}

impl PrivateStorageDriver {
    /// An explicit host setting; never auto-detect or reinterpret an existing store.
    /// Unset preserves the legacy private VFS store.
    pub fn parse_operator_setting(value: Option<&str>) -> Result<Self, String> {
        match value {
            None | Some("vfs") => Ok(Self::Vfs),
            Some("overlay") => Ok(Self::Overlay),
            Some(_) => Err("AUTONOMICS_CONTAINER_STORAGE_DRIVER must be vfs or overlay".into()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PodmanConfig {
    pub program: String,
    pub workspace_root: std::path::PathBuf,
    pub panel_cache_root: std::path::PathBuf,
    /// Isolated Podman storage. None preserves legacy embedded CLI behavior.
    pub storage_root: Option<std::path::PathBuf>,
    pub run_root: Option<std::path::PathBuf>,
    pub storage_driver: Option<String>,
    pub cgroup_manager: Option<String>,
    pub cgroup_parent: Option<String>,
    /// Reuse the operator-established subordinate user namespace. This is a
    /// runtime setting, never a node/agent environment override. Ordinary
    /// hosts retain keep-id; the formal worker already verifies its mapping.
    pub inherit_user_namespace: bool,
    /// Formal workers require an explicit memory cap and set the total
    /// memory-plus-swap cap equal to it, disabling swap in each OCI leaf.
    /// This is an execution-infrastructure setting, never a node override.
    pub disable_swap: bool,
    /// Ordinary cluster workers can explicitly isolate Podman's runtime,
    /// HOME/config and connection environment. None preserves daemon/formal
    /// behavior. This is operator infrastructure, never an agent field.
    pub worker_runtime_dir: Option<std::path::PathBuf>,
    /// Explicit ordinary-worker resource owner. None retains legacy behavior.
    pub worker_resource_control: Option<crate::PodmanWorkerResourceControl>,
    /// Operator-selected ordinary-worker user mapping; never a node override.
    pub worker_user_namespace: Option<crate::PodmanWorkerUserNamespace>,
    /// Explicit byte-pinned OCI runtime for an ordinary worker's private config.
    pub worker_oci_runtime: Option<(std::path::PathBuf, String)>,
}

impl Default for PodmanConfig {
    fn default() -> Self {
        Self::from_env()
    }
}

const WORKER_BACKEND_ENV_REMOVED: &[&str] = &[
    "CONTAINER_HOST",
    "CONTAINER_CONNECTION",
    "CONTAINER_SSHKEY",
    "CONTAINERS_CONF",
    "CONTAINERS_CONF_OVERRIDE",
    "CONTAINERS_CONF_MODULES",
    "CONTAINERS_STORAGE_CONF",
    "CONTAINERS_REGISTRIES_CONF",
    "CONTAINERS_REGISTRIES_CONF_DIR",
    "REGISTRY_AUTH_FILE",
    "CONTAINERS_AUTH_FILE",
    "PODMAN_CONNECTIONS_CONF",
    "DOCKER_HOST",
];

impl PodmanConfig {
    pub fn from_env() -> Self {
        let root = podman_state_root();
        Self {
            program: env_value("AUTONOMICS_PODMAN_PROGRAM", "podman"),
            workspace_root: crate::config::canonicalize_workspace_root(env_path(
                "AUTONOMICS_DAG_WORK_ROOT",
                env_path("AUTONOMICS_PODMAN_WORKSPACE_ROOT", root.join("work")),
            )),
            panel_cache_root: env_path("AUTONOMICS_PANEL_CACHE_ROOT", default_panel_cache_root()),
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
        }
    }

    /// RuntimeHost passes its resolved state directory, including builder
    /// overrides. No workspace or overlay escapes through ambient HOME. The
    /// panel cache is the one deliberate exception: it stays on the shared
    /// catalog root (`default_panel_cache_root`, env-overridable) so panel
    /// bundles exist exactly once locally and containers bind-mount the
    /// catalog's verified content directly instead of a private copy.
    pub fn for_state_dir(state_dir: &Path) -> Self {
        Self {
            program: env_value("AUTONOMICS_PODMAN_PROGRAM", "podman"),
            workspace_root: state_dir.join("containers/workspace"),
            panel_cache_root: crate::config::default_panel_cache_root(),
            storage_root: Some(state_dir.join("containers/storage")),
            run_root: Some(state_dir.join("containers/run")),
            // Deterministic private storage also works inside the trusted
            // user namespace without sharing the host overlay store.
            storage_driver: Some("vfs".into()),
            cgroup_manager: None,
            cgroup_parent: None,
            inherit_user_namespace: false,
            disable_swap: false,
            worker_runtime_dir: None,
            worker_resource_control: None,
            worker_user_namespace: None,
            worker_oci_runtime: None,
        }
    }

    pub fn for_state_dir_with_storage_driver(
        state_dir: &Path,
        driver: PrivateStorageDriver,
    ) -> Self {
        let mut config = Self::for_state_dir(state_dir);
        if driver == PrivateStorageDriver::Overlay {
            // Never reinterpret an existing VFS store or use host storage.
            // The panel cache keeps pointing at the shared catalog root in
            // both layouts — one local copy of every bundle.
            let root = state_dir.join("containers-overlay");
            config.workspace_root = root.join("workspace");
            config.storage_root = Some(root.join("storage"));
            config.run_root = Some(root.join("run"));
            config.storage_driver = Some("overlay".into());
        }
        config
    }

    /// Construct a Podman command using this runtime's configured private store.
    pub fn async_command(&self) -> Command {
        let mut command = async_podman_command(&self.program);
        if let Some(root) = &self.worker_runtime_dir {
            command.arg("--remote=false");
            for key in WORKER_BACKEND_ENV_REMOVED {
                command.env_remove(key);
            }
            command
                .env("XDG_RUNTIME_DIR", root)
                .env("HOME", root.join("home"))
                .env("XDG_CONFIG_HOME", root.join("config"))
                .env("XDG_DATA_HOME", root.join("data"))
                .env("XDG_CACHE_HOME", root.join("cache"))
                .env("TMPDIR", root.join("tmp"));
        }
        if let Some(root) = &self.storage_root {
            command.arg("--root").arg(root);
        }
        if let Some(root) = &self.run_root {
            command.arg("--runroot").arg(root);
            // runroot selects storage state, but libpod's runtime temporary
            // directory otherwise still defaults to /run/libpod in a user
            // namespace. Keep it explicit even with an empty environment.
            command.arg("--tmpdir").arg(root.join("libpod-tmp"));
            if self.worker_runtime_dir.is_none() {
                // Image-copy blob staging honours TMPDIR, not --tmpdir.
                // Multi-GB pull staging must land on the private disk, not
                // the host's tmpfs-backed /tmp (RAM pressure and hard
                // ENOSPC under concurrent pulls). Workers keep their own
                // redirect above.
                let tmpdir = root.join("libpod-tmp");
                let _ = std::fs::create_dir_all(&tmpdir);
                command.env("TMPDIR", tmpdir);
            }
        }
        if let Some(driver) = &self.storage_driver {
            command.arg("--storage-driver").arg(driver);
        }
        if let Some(manager) = &self.cgroup_manager {
            command.arg("--cgroup-manager").arg(manager);
        }
        if self.worker_oci_runtime.is_some() {
            command.arg("--runtime").arg("autonomics-crun");
        }
        command
    }

    /// Construct a blocking Podman command using this runtime's private store.
    pub fn blocking_command(&self) -> std::process::Command {
        let mut command = blocking_podman_command(&self.program);
        if let Some(root) = &self.worker_runtime_dir {
            command.arg("--remote=false");
            for key in WORKER_BACKEND_ENV_REMOVED {
                command.env_remove(key);
            }
            command
                .env("XDG_RUNTIME_DIR", root)
                .env("HOME", root.join("home"))
                .env("XDG_CONFIG_HOME", root.join("config"))
                .env("XDG_DATA_HOME", root.join("data"))
                .env("XDG_CACHE_HOME", root.join("cache"))
                .env("TMPDIR", root.join("tmp"));
        }
        if let Some(root) = &self.storage_root {
            command.arg("--root").arg(root);
        }
        if let Some(root) = &self.run_root {
            command.arg("--runroot").arg(root);
            command.arg("--tmpdir").arg(root.join("libpod-tmp"));
            if self.worker_runtime_dir.is_none() {
                // Same disk-backed TMPDIR redirect as the async builder:
                // pull staging must not hit the host's tmpfs /tmp.
                let tmpdir = root.join("libpod-tmp");
                let _ = std::fs::create_dir_all(&tmpdir);
                command.env("TMPDIR", tmpdir);
            }
        }
        if let Some(driver) = &self.storage_driver {
            command.arg("--storage-driver").arg(driver);
        }
        if let Some(manager) = &self.cgroup_manager {
            command.arg("--cgroup-manager").arg(manager);
        }
        if self.worker_oci_runtime.is_some() {
            command.arg("--runtime").arg("autonomics-crun");
        }
        command
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

    /// Read-only exact-name stop observation in this same explicit backend.
    /// Exit 1 is Podman's documented absence status. Errors/timeouts are unknown,
    /// never permission to remove an unowned container or declare it stopped.
    pub async fn verify_container_absent(&self, name: &str) -> Result<(), ContainerRuntimeError> {
        let result = self.observe_container_absence(name).await?;
        if result.exit_code != 1 {
            return Err(ContainerRuntimeError::Invalid(
                "container absence not proven".into(),
            ));
        }
        Ok(())
    }

    /// Fixed read-only exact-name observation, including actual returned logs.
    /// A caller must still require exit1; this API does not infer stoppage.
    pub async fn observe_container_absence(
        &self,
        name: &str,
    ) -> Result<ContainerRunResult, ContainerRuntimeError> {
        if name.is_empty()
            || name.starts_with('-')
            || name.len() > 128
            || matches!(name, "." | "..")
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        {
            return Err(ContainerRuntimeError::Invalid(
                "unsafe exact container name".into(),
            ));
        }
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            self.config
                .async_command()
                .args(["container", "exists", name])
                .stdin(std::process::Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| ContainerRuntimeError::Invalid("owned stop observation timed out".into()))?
        .map_err(|e| ContainerRuntimeError::Invalid(e.to_string()))?;
        if output.stdout.len() > crate::MAX_CAPTURED_OUTPUT_BYTES
            || output.stderr.len() > crate::MAX_CAPTURED_OUTPUT_BYTES
        {
            return Err(ContainerRuntimeError::Invalid(
                "exact absence observation output exceeds bound".into(),
            ));
        }
        let exit_code = output.status.code().ok_or_else(|| {
            ContainerRuntimeError::Invalid("exact absence observation has unknown exit".into())
        })?;
        Ok(ContainerRunResult {
            exit_code,
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    /// Recover only durable, kernel-fenced owners in this explicit store.
    /// The caller holds the formal execution lease at worker startup.
    pub async fn recover_interrupted_containers(
        &self,
    ) -> Result<Vec<String>, ContainerRuntimeError> {
        crate::container_intent::recover(&self.config)
            .await
            .map_err(ContainerRuntimeError::Invalid)
    }
}

impl Default for PodmanRuntime {
    fn default() -> Self {
        Self::from_env()
    }
}

#[async_trait]
impl PodmanConnection for PodmanRuntime {
    async fn run_observed(
        &self,
        request: ContainerRunRequest,
        private_env: Option<PrivateEnvironment>,
        observer: crate::observation::RunObserver,
    ) -> Result<ContainerRunResult, ContainerRuntimeError> {
        let private = private_env.is_some();
        let result = self
            .run_inner(request, private_env.as_ref(), observer)
            .await;
        if private {
            suppress_private_output(result)
        } else {
            result
        }
    }

    async fn run(
        &self,
        request: ContainerRunRequest,
    ) -> Result<ContainerRunResult, ContainerRuntimeError> {
        self.run_inner(request, None, Default::default()).await
    }

    async fn run_with_private_env(
        &self,
        request: ContainerRunRequest,
        private_env: PrivateEnvironment,
    ) -> Result<ContainerRunResult, ContainerRuntimeError> {
        suppress_private_output(
            self.run_inner(request, Some(&private_env), Default::default())
                .await,
        )
    }

    // ── Development containers (devcontainer-style) ────────────────────
    //
    // Unlike the run path these containers outlive the call: no cleanup
    // guard, no intent record, no timeout. The workspace bind mount is
    // the only host state they reference, so remove/restart can never
    // lose project files.

    async fn dev_container_start(
        &self,
        spec: &crate::dev_container::DevContainerSpec,
    ) -> Result<crate::dev_container::DevContainerStatus, ContainerRuntimeError> {
        use crate::dev_container::DevContainerState;
        if let Some((id, DevContainerState::Running)) = self.dev_ps_row(&spec.name).await? {
            return Ok(crate::dev_container::DevContainerStatus {
                state: DevContainerState::Running,
                container_id: Some(id),
            });
        }
        let existed = self.dev_ps_row(&spec.name).await?.is_some();
        if !existed {
            let args = crate::dev_container::build_dev_create_args(spec)?;
            // CREATE_GATE serialization happens inside create_container.
            // No cleanup guard: a failed create leaves nothing behind
            // anyway, and a successful one must stay.
            create_container(&self.config, args, None).await?;
        }
        self.dev_start_detached(&spec.name).await?;
        let (id, state) =
            self.dev_ps_row(&spec.name)
                .await?
                .ok_or_else(|| ContainerRuntimeError::Invalid(
                    "dev container vanished immediately after start".into(),
                ))?;
        Ok(crate::dev_container::DevContainerStatus {
            state,
            container_id: Some(id),
        })
    }

    async fn dev_container_stop(&self, name: &str) -> Result<(), ContainerRuntimeError> {
        if self.dev_ps_row(name).await?.is_none() {
            return Ok(()); // idempotent: nothing to stop
        }
        let program = &self.config.program;
        let stopped = tokio::time::timeout(
            Duration::from_secs(CLEANUP_TIMEOUT_SECS),
            self.config
                .async_command()
                .arg("stop")
                .arg("--time=10")
                .arg(name)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .output(),
        )
        .await;
        match stopped {
            Ok(Ok(output)) if output.status.success() => Ok(()),
            Ok(Ok(output)) => Err(ContainerRuntimeError::Invalid(format!(
                "`{program} stop` failed with status {}: {}",
                output.status.code().unwrap_or(1),
                truncate_captured_bytes(&output.stderr)
            ))),
            Ok(Err(error)) => Err(ContainerRuntimeError::Invalid(format!(
                "cannot spawn `{program} stop`: {error}"
            ))),
            Err(_) => Err(ContainerRuntimeError::Timeout {
                timeout_secs: CLEANUP_TIMEOUT_SECS,
            }),
        }
    }

    async fn dev_container_remove(&self, name: &str) -> Result<(), ContainerRuntimeError> {
        if self.dev_ps_row(name).await?.is_none() {
            return Ok(()); // idempotent
        }
        remove_container(&self.config, name)
            .await
            .map_err(ContainerRuntimeError::Invalid)
    }

    async fn dev_container_status(
        &self,
        name: &str,
    ) -> Result<crate::dev_container::DevContainerStatus, ContainerRuntimeError> {
        Ok(match self.dev_ps_row(name).await? {
            Some((id, state)) => crate::dev_container::DevContainerStatus {
                state,
                container_id: Some(id),
            },
            None => crate::dev_container::DevContainerStatus {
                state: crate::dev_container::DevContainerState::NotFound,
                container_id: None,
            },
        })
    }

    async fn dev_exec(
        &self,
        name: &str,
        argv: &[String],
        env: &[(String, String)],
        tty: bool,
    ) -> Result<crate::dev_container::DevExecStream, ContainerRuntimeError> {
        use crate::dev_container::DevExecStream;
        if argv.is_empty() {
            return Err(ContainerRuntimeError::Invalid(
                "dev exec `argv` cannot be empty".into(),
            ));
        }
        let program = self.config.program.clone();
        let mut command = self.config.async_command();
        command.arg("exec").arg("--interactive");
        if tty {
            command.arg("--tty");
        }
        command.arg("--sig-proxy=false");
        for (key, value) in env {
            command.arg("--env").arg(format!("{key}={value}"));
        }
        command
            .arg(name)
            .args(argv)
            // This is the one deliberate exception to the module's
            // stdin-null policy: the streams are owned by a gateway
            // WebSocket bridge, never a host tty (see the header note).
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|error| {
            ContainerRuntimeError::Invalid(format!("cannot spawn `{program} exec`: {error}"))
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| ContainerRuntimeError::Invalid("exec child lost stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ContainerRuntimeError::Invalid("exec child lost stdout".into()))?;
        // With --tty podman merges stderr into the pty; keep the pipe
        // (it simply stays empty) so callers have uniform handling.
        let stderr = child.stderr.take();
        let exit_program = program;
        let exit = tokio::spawn(async move {
            let status = child.wait().await.map_err(|error| {
                ContainerRuntimeError::Invalid(format!(
                    "`{exit_program} exec` wait failed: {error}"
                ))
            })?;
            status.code().ok_or_else(|| {
                ContainerRuntimeError::Invalid("`podman exec` terminated by a signal".into())
            })
        });
        Ok(DevExecStream {
            stdin,
            stdout,
            stderr,
            exit,
        })
    }

    fn name(&self) -> &'static str {
        "podman"
    }

    fn workspace_root(&self) -> &Path {
        self.config.workspace_root.as_path()
    }
}

fn suppress_private_output(
    result: Result<ContainerRunResult, ContainerRuntimeError>,
) -> Result<ContainerRunResult, ContainerRuntimeError> {
    match result {
        Ok(result) => Ok(ContainerRunResult {
            exit_code: result.exit_code,
            stdout: String::new(),
            stderr: String::new(),
        }),
        Err(ContainerRuntimeError::ExitStatus { exit_code, .. }) => {
            Err(ContainerRuntimeError::ExitStatus {
                exit_code,
                stdout: String::new(),
                stderr: "private credential container failed; captured output suppressed".into(),
            })
        }
        Err(ContainerRuntimeError::Timeout { timeout_secs }) => {
            Err(ContainerRuntimeError::Timeout { timeout_secs })
        }
        Err(_) => Err(ContainerRuntimeError::Invalid(
            "private credential container execution failed; diagnostic output suppressed".into(),
        )),
    }
}

impl PodmanRuntime {
    /// Probe one container's `(id, state)` by exact name from
    /// `podman ps -a`. Podman's `--filter name=` matches substrings, so
    /// the exact-name comparison happens in
    /// [`crate::dev_container::exact_container_row`].
    async fn dev_ps_row(
        &self,
        name: &str,
    ) -> Result<Option<(String, crate::dev_container::DevContainerState)>, ContainerRuntimeError>
    {
        let program = &self.config.program;
        let listed = tokio::time::timeout(
            Duration::from_secs(CLEANUP_TIMEOUT_SECS),
            self.config
                .async_command()
                .args([
                    "ps",
                    "-a",
                    "--filter",
                    &format!("name={name}"),
                    "--format",
                    "{{.ID}}\t{{.State}}\t{{.Names}}",
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output(),
        )
        .await;
        let output = match listed {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => {
                return Err(ContainerRuntimeError::Invalid(format!(
                    "cannot spawn `{program} ps`: {error}"
                )))
            }
            Err(_) => {
                return Err(ContainerRuntimeError::Timeout {
                    timeout_secs: CLEANUP_TIMEOUT_SECS,
                })
            }
        };
        if !output.status.success() {
            return Err(ContainerRuntimeError::Invalid(format!(
                "`{program} ps` failed with status {}: {}",
                output.status.code().unwrap_or(1),
                truncate_captured_bytes(&output.stderr)
            )));
        }
        Ok(crate::dev_container::exact_container_row(
            &String::from_utf8_lossy(&output.stdout),
            name,
        ))
    }

    /// Start an existing container detached (no `--attach`, so no output
    /// capture is needed and the container keeps running past the call).
    async fn dev_start_detached(&self, name: &str) -> Result<(), ContainerRuntimeError> {
        let program = &self.config.program;
        let started = tokio::time::timeout(
            Duration::from_secs(CLEANUP_TIMEOUT_SECS),
            self.config
                .async_command()
                .arg("start")
                .arg(name)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .output(),
        )
        .await;
        let output = match started {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => {
                return Err(ContainerRuntimeError::Invalid(format!(
                    "cannot spawn `{program} start`: {error}"
                )))
            }
            Err(_) => {
                return Err(ContainerRuntimeError::Timeout {
                    timeout_secs: CLEANUP_TIMEOUT_SECS,
                })
            }
        };
        if !output.status.success() {
            return Err(ContainerRuntimeError::Invalid(format!(
                "`{program} start` failed with status {}: {}",
                output.status.code().unwrap_or(1),
                truncate_captured_bytes(&output.stderr)
            )));
        }
        Ok(())
    }

    async fn run_inner(
        &self,
        request: ContainerRunRequest,
        private_env: Option<&PrivateEnvironment>,
        observer: crate::observation::RunObserver,
    ) -> Result<ContainerRunResult, ContainerRuntimeError> {
        validate_run_request(&request)?;
        if self.config.worker_runtime_dir.is_some() {
            crate::podman_worker::validate_worker_resources(&self.config, &request)?;
        }
        let slurm_allocation = matches!(
            self.config.worker_resource_control,
            Some(crate::PodmanWorkerResourceControl::SlurmAllocation { .. })
        );
        let single_uid_root = matches!(
            self.config.worker_user_namespace,
            Some(crate::PodmanWorkerUserNamespace::SingleUidRoot)
        );
        if (slurm_allocation || single_uid_root) && self.config.worker_runtime_dir.is_none() {
            return Err(ContainerRuntimeError::Invalid(
                "Slurm resource control and single-UID mapping require an explicit ordinary worker"
                    .into(),
            ));
        }
        if single_uid_root {
            crate::podman_worker::validate_single_uid_request(&request)?;
            let mapping = crate::podman_worker::verify_single_uid_namespace(
                &self.config,
                request.timeout_secs.saturating_mul(1000),
            )
            .await?;
            crate::podman_worker::write_worker_evidence(
                &self
                    .config
                    .workspace_root
                    .join(format!("{}.uid-map.json", request.name)),
                &mapping,
            )?;
        }
        validate_private_environment(&request.env, private_env)?;
        self.recover_interrupted_containers().await?;
        let mut args = build_create_args_for_policies(
            &request,
            self.config.inherit_user_namespace,
            self.config.disable_swap,
            slurm_allocation,
            single_uid_root,
        )?;
        let intent = crate::container_intent::OwnedIntent::create(&self.config, &request.name)
            .map_err(ContainerRuntimeError::Invalid)?;
        if let Some(intent) = &intent {
            args.splice(
                0..0,
                [
                    "--label".into(),
                    format!(
                        "{}={}",
                        crate::container_intent::INTENT_LABEL,
                        intent.digest
                    ),
                ],
            );
        }
        if let Some(parent) = &self.config.cgroup_parent {
            args.splice(0..0, ["--cgroup-parent".into(), parent.clone()]);
        }
        // Arm before the cancellable create, so cancellation cannot omit
        // cleanup merely because the await never returned a container ID.
        let mut guard = ContainerCleanupGuard::new(&self.config, &request.name);
        guard.intent = intent;
        observer.phase("creating_container", "creating the admitted container");
        create_container(&self.config, args, private_env).await?;

        let mut start = self.config.async_command();
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
                let cleanup = guard.cleanup().await;
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
            crate::observation::child_output(child, &observer, "container_attach"),
        )
        .await;
        let cleanup_error = guard.cleanup().await.err();
        guard.disarm();

        if let Some(error) = cleanup_error {
            return Err(ContainerRuntimeError::Invalid(format!(
                "container cleanup failed; evidence incomplete: {error}"
            )));
        }

        match attached {
            Ok(Ok(output)) => {
                let stdout = truncate_captured_bytes(&output.stdout);
                let stderr = truncate_captured_bytes(&output.stderr);
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
                "Podman attach stream failed: {error}"
            ))),
            Err(_) => Err(ContainerRuntimeError::Timeout {
                timeout_secs: request.timeout_secs,
            }),
        }
    }
}

fn validate_private_environment(
    ordinary_env: &[(String, String)],
    private_env: Option<&PrivateEnvironment>,
) -> Result<(), ContainerRuntimeError> {
    if let Some(private_env) = private_env {
        for (name, _) in private_env.values() {
            if ordinary_env.iter().any(|(ordinary, _)| ordinary == name) {
                return Err(ContainerRuntimeError::Invalid(
                    "private credential conflicts with ordinary environment".into(),
                ));
            }
        }
    }
    Ok(())
}

async fn create_container(
    config: &PodmanConfig,
    args: Vec<String>,
    private_env: Option<&PrivateEnvironment>,
) -> Result<(), ContainerRuntimeError> {
    let program = &config.program;
    let timeout_secs = crate::images::pull_timeout_secs();
    // Queue here, before the timeout below: a queued create must not burn its
    // pull budget while another create is pulling multi-GB layers.
    let _create_permit = CREATE_GATE.acquire().await.map_err(|error| {
        ContainerRuntimeError::Invalid(format!("podman create gate closed: {error}"))
    })?;
    let mut command = prepare_create_command(config, args, private_env);
    let created = tokio::time::timeout(Duration::from_secs(timeout_secs), command.output()).await;
    let output = match created {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            return Err(ContainerRuntimeError::Invalid(format!(
                "cannot spawn `{program} create`: {error}"
            )));
        }
        // The prearmed guard owns cleanup, including a cancelled create.
        // A name-only rm here would bypass its container identity fence.
        Err(_) => return Err(ContainerRuntimeError::Timeout { timeout_secs }),
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

fn prepare_create_command(
    config: &PodmanConfig,
    args: Vec<String>,
    private_env: Option<&PrivateEnvironment>,
) -> Command {
    let mut command = config.async_command();
    command.arg("create");
    if let Some(private_env) = private_env {
        for (name, value) in private_env.values() {
            command.arg("--env").arg(name).env(name, value);
        }
    }
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    command
}

async fn remove_container(config: &PodmanConfig, name: &str) -> Result<(), String> {
    let removed = tokio::time::timeout(
        Duration::from_secs(CLEANUP_TIMEOUT_SECS),
        config
            .async_command()
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
    config: PodmanConfig,
    name: String,
    armed: bool,
    intent: Option<crate::container_intent::OwnedIntent>,
}

impl ContainerCleanupGuard {
    fn new(config: &PodmanConfig, name: &str) -> Self {
        Self {
            config: config.clone(),
            name: name.to_string(),
            armed: true,
            intent: None,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }

    async fn cleanup(&self) -> Result<(), String> {
        match &self.intent {
            Some(intent) => intent.cleanup(&self.config).await,
            None => remove_container(&self.config, &self.name).await,
        }
    }
}

impl Drop for ContainerCleanupGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let config = self.config.clone();
        let name = self.name.clone();
        let intent = self.intent.take();
        std::thread::spawn(move || {
            if let Some(intent) = intent {
                if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    let _ = runtime.block_on(intent.cleanup(&config));
                }
                return;
            }
            let _ = config
                .blocking_command()
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
    command.env_remove("OPENGWAS_JWT");
    command.current_dir(HOST_CWD);
    command.kill_on_drop(true);
    command
}

fn blocking_podman_command(program: &str) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    command.env_remove("OPENGWAS_JWT");
    command.current_dir(HOST_CWD);
    command
}

#[cfg(test)]
pub(crate) fn build_create_args(
    request: &ContainerRunRequest,
) -> Result<Vec<String>, ContainerRuntimeError> {
    build_create_args_with_policy(request, false, false)
}

#[cfg(test)]
fn build_create_args_with_policy(
    request: &ContainerRunRequest,
    inherit_user_namespace: bool,
    disable_swap: bool,
) -> Result<Vec<String>, ContainerRuntimeError> {
    build_create_args_for_policies(request, inherit_user_namespace, disable_swap, false, false)
}

fn build_create_args_for_policies(
    request: &ContainerRunRequest,
    inherit_user_namespace: bool,
    disable_swap: bool,
    slurm_allocation: bool,
    single_uid_root: bool,
) -> Result<Vec<String>, ContainerRuntimeError> {
    validate_podman_request(request)?;
    if slurm_allocation && (inherit_user_namespace || disable_swap) {
        return Err(ContainerRuntimeError::Invalid(
            "Slurm allocation mode cannot replace formal namespace or no-swap enforcement".into(),
        ));
    }
    if single_uid_root {
        crate::podman_worker::validate_single_uid_request(request)?;
    }
    if disable_swap && request.memory.is_none() {
        return Err(ContainerRuntimeError::Invalid(
            "formal container execution requires an explicit memory cap without swap".into(),
        ));
    }
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
        if single_uid_root {
            "0:0".into()
        } else {
            user_value(request)
        },
        "--userns".into(),
        if inherit_user_namespace || single_uid_root {
            "host"
        } else {
            "keep-id"
        }
        .into(),
    ];
    if slurm_allocation {
        args.extend([
            "--cgroups".into(),
            "disabled".into(),
            "--cgroupns".into(),
            "host".into(),
        ]);
    }
    if slurm_allocation || single_uid_root {
        args.extend(["--cap-drop".into(), "ALL".into()]);
    }
    if request.read_only_rootfs {
        args.push("--read-only".into());
    }
    if let Some(size) = &request.shm_size {
        args.push("--shm-size".into());
        args.push(podman_size(size)?);
    }
    if let Some(cpus) = request.cpus.filter(|_| !slurm_allocation) {
        args.push("--cpus".into());
        args.push(cpus.to_string());
    }
    if let Some(memory) = request.memory.as_ref().filter(|_| !slurm_allocation) {
        args.push("--memory".into());
        let size = podman_size(memory)?;
        args.push(size.clone());
        if disable_swap {
            args.push("--memory-swap".into());
            args.push(size);
        }
    }
    if let Some(pids_limit) = request.pids_limit.filter(|_| !slurm_allocation) {
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
    for mount in &request.input_mounts {
        let field = escape_mount_field(&mount.host_dir.to_string_lossy());
        args.push("--mount".into());
        args.push(format!(
            "type=bind,source={field},destination={field},readonly"
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

pub(crate) fn escape_mount_field(value: &str) -> String {
    value.replace('\\', "\\\\").replace(',', "\\,")
}

/// Podman accepts bytes or decimal k/m/g suffixes, while container nodes use
/// Kubernetes quantities such as `16Gi` and explicit IEC quantities such as
/// `1GiB`. Emit exact bytes so both SI and IEC quantities keep their size.
pub(crate) fn podman_size(value: &str) -> Result<String, ContainerRuntimeError> {
    crate::memory_quantity_bytes(value).map(|bytes| bytes.to_string())
}

/// The same exact quantity interpretation used by both local and Slurm limits.
pub fn memory_quantity_bytes(value: &str) -> Result<u64, ContainerRuntimeError> {
    podman_size(value)?.parse().map_err(|_| {
        ContainerRuntimeError::Invalid("memory quantity is outside the supported range".into())
    })
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
    for input_mount in &request.input_mounts {
        let mount = input_mount.host_dir.as_path();
        let overlapping = !mounts.insert(mount.to_string_lossy().into_owned())
            || mount.starts_with(workspace_mount)
            || workspace_mount.starts_with(mount)
            || request.panels.iter().any(|panel| {
                let panel_mount = Path::new(&panel.mount_path);
                mount.starts_with(panel_mount) || panel_mount.starts_with(mount)
            });
        if overlapping {
            return Err(ContainerRuntimeError::Invalid(format!(
                "duplicate or overlapping input mount `{}`",
                mount.display()
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

fn user_value(request: &ContainerRunRequest) -> String {
    let (uid, gid) = request_user_ids(request);
    format!("{uid}:{gid}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::unique_container_name;
    use crate::types::{CachedPanel, InputMount, WorkspaceRef};
    use std::path::PathBuf;

    #[test]
    fn private_create_command_puts_only_the_variable_name_in_argv() {
        let private_env = PrivateEnvironment::new(vec![(
            "OPENGWAS_JWT".into(),
            "synthetic-private-credential".into(),
        )])
        .unwrap();
        let command = prepare_create_command(
            &PodmanConfig::default(),
            vec!["example-image".into()],
            Some(&private_env),
        );
        let args = command
            .as_std()
            .get_args()
            .map(|value| value.to_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(args, ["create", "--env", "OPENGWAS_JWT", "example-image"]);
        let env = command.as_std().get_envs().collect::<Vec<_>>();
        assert!(env.iter().any(|(name, value)| {
            *name == "OPENGWAS_JWT"
                && *value == Some(std::ffi::OsStr::new("synthetic-private-credential"))
        }));
        let ordinary = PodmanConfig::default().async_command();
        assert!(
            ordinary
                .as_std()
                .get_envs()
                .any(|(name, value)| { name == "OPENGWAS_JWT" && value.is_none() })
        );
        assert!(
            blocking_podman_command("podman")
                .get_envs()
                .any(|(name, value)| { name == "OPENGWAS_JWT" && value.is_none() })
        );
    }

    #[test]
    fn private_environment_refuses_ordinary_env_collision_before_execution() {
        let private_env = PrivateEnvironment::new(vec![(
            "OPENGWAS_JWT".into(),
            "synthetic-private-credential".into(),
        )])
        .unwrap();
        assert!(validate_private_environment(&[], Some(&private_env)).is_ok());
        assert!(
            validate_private_environment(
                &[("OPENGWAS_JWT".into(), "ordinary-value".into())],
                Some(&private_env),
            )
            .is_err()
        );
        assert!(
            validate_private_environment(
                &[("ORDINARY".into(), "ordinary-value".into())],
                Some(&private_env),
            )
            .is_ok()
        );
    }

    #[test]
    fn private_run_suppresses_all_captured_output_and_preserves_exit_status() {
        let result = suppress_private_output(Ok(ContainerRunResult {
            exit_code: 0,
            stdout: "raw or encoded synthetic-private-credential".into(),
            stderr: "credential fragment".into(),
        }))
        .unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.is_empty() && result.stderr.is_empty());
        for error in [
            ContainerRuntimeError::Invalid("synthetic-private-credential".into()),
            ContainerRuntimeError::ExitStatus {
                exit_code: 7,
                stdout: "synthetic-private-credential".into(),
                stderr: "synthetic-private-credential".into(),
            },
        ] {
            let sanitized = suppress_private_output(Err(error)).unwrap_err();
            assert!(!format!("{sanitized:?}").contains("synthetic-private-credential"));
            if let ContainerRuntimeError::ExitStatus { exit_code, .. } = sanitized {
                assert_eq!(exit_code, 7);
            }
        }
        assert!(matches!(
            suppress_private_output(Err(ContainerRuntimeError::Timeout { timeout_secs: 17 })),
            Err(ContainerRuntimeError::Timeout { timeout_secs: 17 })
        ));
    }

    #[test]
    fn protected_namespace_reuse_preserves_every_other_container_constraint() {
        let request = request(ContainerNetwork::Isolated);
        let ordinary = build_create_args_with_policy(&request, false, false).unwrap();
        let protected = build_create_args_with_policy(&request, true, false).unwrap();
        let index = ordinary.iter().position(|arg| arg == "--userns").unwrap() + 1;
        assert_eq!(ordinary[index], "keep-id");
        assert_eq!(protected[index], "host");
        let mut expected = ordinary;
        expected[index] = "host".into();
        assert_eq!(protected, expected);
        assert!(
            !PodmanConfig::for_state_dir(Path::new("/tmp/test-ordinary-host"))
                .inherit_user_namespace
        );
    }

    #[test]
    fn formal_memory_policy_disables_swap_for_the_exact_memory_quantity() {
        for (memory, bytes) in [
            ("8Gi", "8589934592"),
            ("256Mi", "268435456"),
            ("1GiB", "1073741824"),
            ("256MiB", "268435456"),
            ("1GB", "1000000000"),
        ] {
            let mut request = request(ContainerNetwork::Isolated);
            request.memory = Some(memory.into());
            let args = build_create_args_with_policy(&request, true, true).unwrap();
            let ram = args.iter().position(|arg| arg == "--memory").unwrap();
            let total = args.iter().position(|arg| arg == "--memory-swap").unwrap();
            assert_eq!(args[ram + 1], bytes);
            assert_eq!(args[total + 1], bytes);
            assert_eq!(args.iter().filter(|arg| *arg == "--memory-swap").count(), 1);
            let ordinary = build_create_args_with_policy(&request, false, false).unwrap();
            assert!(!ordinary.iter().any(|arg| arg == "--memory-swap"));
        }
    }

    #[test]
    fn memory_quantity_aliases_preserve_units_and_reject_invalid_limits() {
        for (quantity, bytes) in [
            ("1KiB", "1024"),
            ("1TiB", "1099511627776"),
            ("1kB", "1000"),
            ("1MB", "1000000"),
            ("1TB", "1000000000000"),
            ("1.5GiB", "1610612736"),
            ("512mib", "536870912"),
        ] {
            assert_eq!(podman_size(quantity).unwrap(), bytes);
        }
        for quantity in ["", "0GiB", "-1GiB", "NaN", "1GiBB", "1PB", "1e999GiB"] {
            assert!(podman_size(quantity).is_err(), "accepted {quantity}");
        }
        // Decimal and binary units must remain distinct.
        assert_ne!(podman_size("1g").unwrap(), podman_size("1GiB").unwrap());
    }

    #[test]
    fn formal_memory_policy_refuses_an_unbounded_container() {
        let mut request = request(ContainerNetwork::Isolated);
        request.memory = None;
        let error = build_create_args_with_policy(&request, true, true).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("explicit memory cap without swap")
        );
        assert!(build_create_args_with_policy(&request, false, false).is_ok());
        assert!(!PodmanConfig::for_state_dir(Path::new("/tmp/ordinary")).disable_swap);
    }

    #[tokio::test]
    #[ignore = "explicit synthetic subordinate-runtime deployment and admitted image required"]
    async fn protected_namespace_runs_actual_pinned_oci() {
        let root = PathBuf::from(std::env::var("AUTONOMICS_RESEARCH_VAULT_ROOT").unwrap());
        let evidence = root.join("evidence");
        let admitted: serde_json::Value = serde_json::from_slice(
            &std::fs::read(evidence.join("trusted-authority.json")).unwrap(),
        )
        .unwrap();
        let authority: serde_json::Value = serde_json::from_slice(
            &std::fs::read(
                evidence
                    .join("artifacts")
                    .join(admitted["sha256"].as_str().unwrap()),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            authority["allowed_studies"],
            serde_json::json!(["SYNTHETIC-RUNTIME-IO-TEST"])
        );
        let infra =
            crate::ContainerExecutionInfra::for_formal_worker_state_dir(&root.join("state"))
                .unwrap();
        assert!(infra.config.inherit_user_namespace);
        assert!(infra.config.disable_swap);
        let workspace = root.join("runtime").join(unique_container_name());
        std::fs::create_dir(&workspace).unwrap();
        let mut request = request(ContainerNetwork::Isolated);
        request.name = unique_container_name();
        request.workspace.host_path = workspace.clone();
        request.image = "ghcr.io/zj-2002/biotools-py@sha256:7ad6aeafa87672699d28b61b6bed12fc81fcdbfc43222a0abf17e2d56fb806f5".into();
        request.command = vec!["python3".into(), "-c".into(), "import pathlib; pathlib.Path('/work/SYNTHETIC.txt').write_text('SYNTHETIC RUNTIME LIBRARY ONLY'); print('SYNTHETIC_PROTECTED_RUNTIME_LIBRARY_OK')".into()];
        request.user = None;
        request.cpus = Some(1.0);
        request.memory = Some("256Mi".into());
        request.pids_limit = Some(64);
        let result = PodmanRuntime::new(infra.config.clone())
            .run(request)
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0, "{}", result.stderr);
        assert!(
            result
                .stdout
                .contains("SYNTHETIC_PROTECTED_RUNTIME_LIBRARY_OK")
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join("SYNTHETIC.txt")).unwrap(),
            "SYNTHETIC RUNTIME LIBRARY ONLY"
        );
    }

    #[test]
    fn instance_paths_and_every_command_use_isolated_container_storage() {
        let a = PodmanConfig::for_state_dir(Path::new("/tmp/test-instance-a"));
        let b = PodmanConfig::for_state_dir(Path::new("/tmp/test-instance-b"));
        assert!(a.workspace_root.starts_with("/tmp/test-instance-a"));
        // The panel cache is the one shared root: every instance mounts
        // the catalog's single verified copy instead of a private duplicate.
        assert_eq!(a.panel_cache_root, b.panel_cache_root);
        assert_ne!(a.storage_root, b.storage_root);
        assert_ne!(a.run_root, b.run_root);
        let asynchronous = a.async_command();
        let synchronous = a.blocking_command();
        let expected = [
            "--root",
            "/tmp/test-instance-a/containers/storage",
            "--runroot",
            "/tmp/test-instance-a/containers/run",
            "--tmpdir",
            "/tmp/test-instance-a/containers/run/libpod-tmp",
            "--storage-driver",
            "vfs",
        ];
        assert_eq!(
            asynchronous
                .as_std()
                .get_args()
                .map(|s| s.to_str().unwrap())
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(
            synchronous
                .get_args()
                .map(|s| s.to_str().unwrap())
                .collect::<Vec<_>>(),
            expected
        );
    }

    #[test]
    fn explicit_overlay_uses_a_separate_private_store_in_every_command() {
        let state = Path::new("/tmp/test-signed-overlay");
        let legacy = PodmanConfig::for_state_dir(state);
        let unchanged =
            PodmanConfig::for_state_dir_with_storage_driver(state, PrivateStorageDriver::Vfs);
        assert_eq!(legacy.storage_root, unchanged.storage_root);
        assert_eq!(legacy.storage_driver, unchanged.storage_driver);
        let overlay =
            PodmanConfig::for_state_dir_with_storage_driver(state, PrivateStorageDriver::Overlay);
        assert_ne!(overlay.storage_root, legacy.storage_root);
        // Storage/run/workspace move to the separate overlay store; the
        // panel cache stays on the shared catalog root in every layout.
        assert_eq!(overlay.panel_cache_root, legacy.panel_cache_root);
        for root in [
            &overlay.workspace_root,
            overlay.storage_root.as_ref().unwrap(),
            overlay.run_root.as_ref().unwrap(),
        ] {
            assert!(root.starts_with(state.join("containers-overlay")));
        }
        let expected = [
            "--root",
            "/tmp/test-signed-overlay/containers-overlay/storage",
            "--runroot",
            "/tmp/test-signed-overlay/containers-overlay/run",
            "--tmpdir",
            "/tmp/test-signed-overlay/containers-overlay/run/libpod-tmp",
            "--storage-driver",
            "overlay",
        ];
        assert_eq!(
            overlay
                .async_command()
                .as_std()
                .get_args()
                .map(|v| v.to_str().unwrap())
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(
            overlay
                .blocking_command()
                .get_args()
                .map(|v| v.to_str().unwrap())
                .collect::<Vec<_>>(),
            expected
        );
    }

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
            input_mounts: Vec::new(),
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
    fn slurm_policy_uses_inherited_limits_and_keeps_container_isolation() {
        let mut request = request(ContainerNetwork::Isolated);
        let workspace_cleanup = request.workspace.host_path.clone();
        request.user = None;
        let args = build_create_args_for_policies(&request, false, false, true, true).unwrap();
        for flag in [
            "--cpus",
            "--memory",
            "--memory-swap",
            "--pids-limit",
            "--cgroup-parent",
        ] {
            assert!(!args.iter().any(|arg| arg == flag), "unexpected {flag}");
        }
        for (flag, value) in [
            ("--cgroups", "disabled"),
            ("--userns", "host"),
            ("--user", "0:0"),
            ("--network", "none"),
            ("--cap-drop", "ALL"),
            ("--security-opt", "no-new-privileges"),
            ("--pull", "never"),
        ] {
            let position = args.iter().position(|arg| arg == flag).unwrap();
            assert_eq!(args[position + 1], value);
        }
        assert!(args.iter().any(|arg| arg == "--read-only"));
        assert!(args.iter().any(|arg| arg == "--mount"));
        assert!(args.iter().any(|arg| arg == "--shm-size"));
        for user in ["1000", "1000:1000", "root", "0:1000"] {
            request.user = Some(user.into());
            assert!(build_create_args_for_policies(&request, false, false, true, true).is_err());
        }
        request.user = None;
        assert!(build_create_args_for_policies(&request, true, false, true, false).is_err());
        assert!(build_create_args_for_policies(&request, false, true, true, false).is_err());
        std::fs::remove_dir_all(workspace_cleanup).unwrap();
    }

    #[test]
    fn podman_commands_do_not_inherit_daemon_cwd() {
        assert_eq!(
            blocking_podman_command("podman").get_current_dir(),
            Some(std::path::Path::new("/"))
        );
    }

    #[test]
    fn dag_work_root_env_is_preferred_over_legacy_podman_name() {
        let preferred = tempfile::tempdir().unwrap();
        let legacy = tempfile::tempdir().unwrap();
        let preferred_guard = EnvGuard::set("AUTONOMICS_DAG_WORK_ROOT", preferred.path());
        let _legacy_guard = EnvGuard::set("AUTONOMICS_PODMAN_WORKSPACE_ROOT", legacy.path());
        assert_eq!(
            PodmanConfig::from_env().workspace_root,
            crate::config::canonicalize_workspace_root(preferred.path().to_path_buf())
        );
        drop(preferred_guard);
        assert_eq!(
            PodmanConfig::from_env().workspace_root,
            crate::config::canonicalize_workspace_root(legacy.path().to_path_buf())
        );
    }

    struct EnvGuard(&'static str);
    impl EnvGuard {
        fn set(name: &'static str, value: &std::path::Path) -> Self {
            // SAFETY: process-global env mutation, restored on drop.
            unsafe { std::env::set_var(name, value) };
            Self(name)
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: process-global state restored at test end.
            unsafe { std::env::remove_var(self.0) };
        }
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
            manifest_bytes: None,
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
    async fn private_intent_covers_success_cleanup_failure_and_cancelled_create() {
        for mode in ["success", "remove-fails", "cancel-create"] {
            let root = tempfile::tempdir().unwrap();
            let program = root.path().join("fake-private-podman");
            std::fs::write(&program, r#"#!/usr/bin/python3
import json,os,pathlib,sys,time
a=sys.argv[1:];p=pathlib.Path(a[a.index('--root')+1]).parent.parent
s=p/'TEST-state.json';mode=(p/'TEST-mode').read_text()
if 'create' in a:
 label=a[a.index('--label')+1].split('=',1)[1]
 s.write_text(json.dumps({'label':label,'pid':os.getpid()}))
 if mode=='cancel-create':time.sleep(60)
elif 'container' in a:sys.exit(0 if s.exists() else 1)
elif 'inspect' in a:
 state=json.loads(s.read_text())
 print(json.dumps([{'Id':'a'*64,'Config':{'Labels':{'io.autonomics.container-intent':state['label']}}}]))
elif 'rm' in a:
 if mode=='remove-fails':sys.exit(125)
 assert a[-1]=='a'*64
 s.unlink();(p/'TEST-removed').write_text(a[-1])
elif 'start' in a:print('TEST only')
else:sys.exit(125)
"#).unwrap();
            make_executable(&program);
            std::fs::write(root.path().join("TEST-mode"), mode).unwrap();
            let mut config = PodmanConfig::for_state_dir(root.path());
            config.program = program.to_string_lossy().into();
            std::fs::create_dir_all(&config.workspace_root).unwrap();
            let mut request = request(ContainerNetwork::Isolated);
            request.workspace.host_path = config.workspace_root.clone();
            request.name = "TEST-owned".into();
            let journal = config
                .run_root
                .as_ref()
                .unwrap()
                .join("autonomics-container-intents/TEST-owned.json");
            let runtime = PodmanRuntime::new(config.clone());
            if mode == "cancel-create" {
                let job = tokio::spawn(async move { runtime.run(request).await });
                let pid = tokio::time::timeout(Duration::from_secs(10), async {
                    loop {
                        if let Ok(bytes) = std::fs::read(root.path().join("TEST-state.json")) {
                            if let Ok(state) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                                break state["pid"].as_u64().unwrap();
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .unwrap();
                assert!(journal.is_file());
                job.abort();
                assert!(job.await.unwrap_err().is_cancelled());
                tokio::time::timeout(Duration::from_secs(10), async {
                    while journal.exists() || Path::new(&format!("/proc/{pid}")).exists() {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .unwrap();
                assert_eq!(
                    std::fs::read_to_string(root.path().join("TEST-removed")).unwrap(),
                    "a".repeat(64)
                );
            } else {
                let result = runtime.run(request).await;
                if mode == "success" {
                    assert_eq!(result.unwrap().stdout.trim(), "TEST only");
                    assert!(!journal.exists());
                } else {
                    assert!(
                        result
                            .unwrap_err()
                            .to_string()
                            .contains("evidence incomplete")
                    );
                    assert!(journal.is_file());
                    assert!(!root.path().join("TEST-removed").exists());
                    std::fs::write(root.path().join("TEST-mode"), "success").unwrap();
                    // The still-live PID is retained even after its lock is released.
                    assert!(
                        PodmanRuntime::new(config)
                            .recover_interrupted_containers()
                            .await
                            .unwrap()
                            .is_empty()
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn exact_owned_absence_uses_same_backend_and_only_status_one() {
        let root = tempfile::tempdir().unwrap();
        let program = root.path().join("fake-podman");
        std::fs::write(&program,"#!/usr/bin/python3\nimport json,pathlib,sys\np=pathlib.Path(sys.argv[0])\np.with_suffix('.argv').write_text(json.dumps(sys.argv[1:]))\nname=sys.argv[-1]\nsys.exit(1 if name=='owned-absent' else 0 if name=='owned-present' else 2)\n").unwrap();
        make_executable(&program);
        let mut config = PodmanConfig::for_state_dir_with_storage_driver(
            root.path(),
            PrivateStorageDriver::Overlay,
        );
        config.program = program.to_str().unwrap().into();
        config.cgroup_manager = Some("cgroupfs".into());
        let runtime = PodmanRuntime::new(config.clone());
        runtime
            .verify_container_absent("owned-absent")
            .await
            .unwrap();
        let argv: Vec<String> =
            serde_json::from_slice(&std::fs::read(program.with_extension("argv")).unwrap())
                .unwrap();
        assert_eq!(
            argv,
            vec![
                "--root".to_string(),
                config.storage_root.unwrap().display().to_string(),
                "--runroot".into(),
                config.run_root.as_ref().unwrap().display().to_string(),
                "--tmpdir".into(),
                config
                    .run_root
                    .unwrap()
                    .join("libpod-tmp")
                    .display()
                    .to_string(),
                "--storage-driver".into(),
                "overlay".into(),
                "--cgroup-manager".into(),
                "cgroupfs".into(),
                "container".into(),
                "exists".into(),
                "owned-absent".into()
            ]
        );
        assert!(
            runtime
                .verify_container_absent("owned-present")
                .await
                .is_err()
        );
        assert!(
            runtime
                .verify_container_absent("owned-error")
                .await
                .is_err()
        );
        std::fs::remove_file(program.with_extension("argv")).unwrap();
        for name in ["--all", "owned/other", "", ".."] {
            assert!(runtime.verify_container_absent(name).await.is_err());
            assert!(!program.with_extension("argv").exists());
        }
    }

    #[tokio::test]
    async fn absence_observation_retains_actual_return_and_both_logs_without_claiming_stop() {
        let root = tempfile::tempdir().unwrap();
        let program = root.path().join("fake-podman");
        std::fs::write(&program,"#!/usr/bin/python3\nimport sys\nprint('actual stdout')\nprint('actual stderr',file=sys.stderr)\nsys.exit(1 if sys.argv[-1]=='absent' else 0)\n").unwrap();
        make_executable(&program);
        let mut config = PodmanConfig::for_state_dir(root.path());
        config.program = program.to_str().unwrap().into();
        let runtime = PodmanRuntime::new(config);
        for (name, code) in [("absent", 1), ("present", 0)] {
            let observed = runtime.observe_container_absence(name).await.unwrap();
            assert_eq!(observed.exit_code, code);
            assert_eq!(observed.stdout, "actual stdout\n");
            assert_eq!(observed.stderr, "actual stderr\n");
        }
        assert!(runtime.verify_container_absent("present").await.is_err());
    }

    #[tokio::test]
    async fn absence_observation_oversized_logs_refuse_without_silent_truncation() {
        let root = tempfile::tempdir().unwrap();
        let program = root.path().join("fake-podman");
        std::fs::write(
            &program,
            "#!/usr/bin/python3\nimport sys\nsys.stdout.write('x'*65537)\nsys.exit(1)\n",
        )
        .unwrap();
        make_executable(&program);
        let mut config = PodmanConfig::for_state_dir(root.path());
        config.program = program.to_str().unwrap().into();
        let runtime = PodmanRuntime::new(config);
        assert!(runtime.observe_container_absence("absent").await.is_err());
        assert!(runtime.verify_container_absent("absent").await.is_err());
    }

    #[tokio::test]
    async fn cancelled_create_stops_the_cli_and_runs_already_armed_cleanup() {
        let root = tempfile::tempdir().unwrap();
        let program = root.path().join("fake-podman");
        std::fs::write(&program,
            "#!/usr/bin/python3\nimport pathlib,sys,time,os\np=pathlib.Path(sys.argv[0])\nif sys.argv[1]=='create':\n p.with_suffix('.started').write_text(str(os.getpid()))\n time.sleep(60)\nelif sys.argv[1]=='rm':\n p.with_suffix('.removed').write_text(' '.join(sys.argv[1:]))\nelse:\n sys.exit(2)\n").unwrap();
        make_executable(&program);
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let runtime = PodmanRuntime::new(PodmanConfig {
            program: program.to_string_lossy().into_owned(),
            workspace_root: workspace.clone(),
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
        let mut request = request(ContainerNetwork::Isolated);
        request.workspace.host_path = workspace;
        request.name = unique_container_name();
        let job = tokio::spawn(async move { runtime.run(request).await });
        let pid: u32 = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Ok(text) = std::fs::read_to_string(program.with_extension("started")) {
                    if let Ok(pid) = text.parse::<u32>() {
                        break pid;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        job.abort();
        assert!(job.await.unwrap_err().is_cancelled());
        tokio::time::timeout(Duration::from_secs(10), async {
            while !program.with_extension("removed").is_file()
                || Path::new(&format!("/proc/{pid}")).exists()
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(
            std::fs::read_to_string(program.with_extension("removed"))
                .unwrap()
                .starts_with("rm --force --time=0 autonomics-container-command-")
        );
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
            input_mounts: Vec::new(),
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

    #[test]
    fn canonicalize_root_resolves_state_directory_symlink() {
        // Storage-migration layout: a symlinked state dir must yield the
        // canonical target so containment checks and identity mounts agree
        // with canonicalized workdirs.
        // Frozen acceptance source is read-only. Self-cleaning fixtures
        // belong to TMPDIR, which may itself use a symlinked path.
        let real = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let real_path = real.path().canonicalize().unwrap();
        let alias = home.path().join("state");
        std::os::unix::fs::symlink(real.path(), &alias).unwrap();
        let canonical = crate::config::canonicalize_workspace_root(alias.join("work"));
        assert_eq!(canonical, real_path.join("work"));
        // Nonexistent deep paths are created on demand and canonicalized.
        let nested = crate::config::canonicalize_workspace_root(alias.join("a/b/c"));
        assert_eq!(nested, real_path.join("a/b/c"));
        assert!(nested.is_dir());
    }

    #[test]
    fn create_args_emit_identity_readonly_input_mounts() {
        let root = tempfile::tempdir().unwrap();
        let source_dir = root.path().join("sources");
        std::fs::create_dir_all(&source_dir).unwrap();
        let mut req = request(ContainerNetwork::Isolated);
        req.input_mounts = vec![InputMount {
            host_dir: source_dir.clone(),
        }];
        let args = build_create_args(&req).unwrap();
        let expected = format!(
            "type=bind,source={},destination={},readonly",
            source_dir.display(),
            source_dir.display()
        );
        assert!(args.contains(&expected));
    }

    #[test]
    fn input_mounts_overlap_workspace_or_panels_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let source_dir = root.path().join("sources");
        std::fs::create_dir_all(&source_dir).unwrap();

        // Destination overlaps the container workdir `/work`.
        let mut req = request(ContainerNetwork::Isolated);
        req.input_mounts = vec![InputMount {
            host_dir: PathBuf::from("/work"),
        }];
        assert!(build_create_args(&req).is_err());

        // Overlaps a panel mount path.
        let panel_dir = root.path().join("panel");
        std::fs::create_dir_all(&panel_dir).unwrap();
        let mut req = request(ContainerNetwork::Isolated);
        req.panels = vec![CachedPanel {
            id: "p".into(),
            digest: "sha256:x".into(),
            host_path: panel_dir.clone(),
            mount_path: "/data/panel".into(),
            manifest_bytes: None,
        }];
        req.input_mounts = vec![InputMount {
            host_dir: PathBuf::from("/data"),
        }];
        assert!(build_create_args(&req).is_err());

        // Duplicate mounts are rejected.
        let mut req = request(ContainerNetwork::Isolated);
        req.input_mounts = vec![
            InputMount {
                host_dir: source_dir.clone(),
            },
            InputMount {
                host_dir: source_dir.clone(),
            },
        ];
        assert!(build_create_args(&req).is_err());
    }

    #[test]
    fn validate_run_request_rejects_unsafe_input_mount_roots() {
        let mut req = request(ContainerNetwork::Isolated);
        for unsafe_dir in [
            "/tmp", "/tmp/sub", "/dev", "/dev/shm", "/proc", "/sys", "/run", "/",
        ] {
            req.input_mounts = vec![InputMount {
                host_dir: PathBuf::from(unsafe_dir),
            }];
            let error = crate::connection::validate_run_request(&req)
                .expect_err(unsafe_dir)
                .to_string();
            assert!(error.contains("unsafe"), "{unsafe_dir}: {error}");
        }

        // A relative or nonexistent path is rejected too.
        req.input_mounts = vec![InputMount {
            host_dir: PathBuf::from("relative/dir"),
        }];
        assert!(crate::connection::validate_run_request(&req).is_err());
        req.input_mounts = vec![InputMount {
            host_dir: PathBuf::from("/nonexistent/absolute/dir"),
        }];
        assert!(crate::connection::validate_run_request(&req).is_err());
    }

    #[test]
    fn ordinary_absolute_input_mounts_pass_run_request_validation() {
        // Regression: `Path::starts_with("/")` matches every absolute path,
        // so a `/` entry in the unsafe-prefix list rejected ALL input mounts
        // in production (agent workspace dirs included) — FakeRuntime tests
        // never exercised validate_run_request. Ordinary data locations on
        // real disks must be accepted.
        // Prefer the managed TMPDIR without writing into frozen source.
        // A default /tmp (or /run) is deliberately forbidden as an input
        // mount, so use the ordinary Unix /var/tmp in that case. Both are
        // owned self-cleaning toy fixtures; the production policy is intact.
        let temporary = std::env::temp_dir();
        let mount_safe_temporary = if ["/tmp", "/dev", "/proc", "/sys", "/run"]
            .iter()
            .any(|root| temporary.starts_with(root))
        {
            Path::new("/var/tmp")
        } else {
            temporary.as_path()
        };
        let source_dir = tempfile::tempdir_in(mount_safe_temporary).unwrap();
        let mut req = request(ContainerNetwork::Isolated);
        req.input_mounts = vec![InputMount {
            host_dir: source_dir.path().to_path_buf(),
        }];
        crate::connection::validate_run_request(&req)
            .expect("ordinary absolute input mount must be accepted");

        // The filesystem root itself stays rejected (it would expose the
        // whole host read-only into the container).
        let mut req = request(ContainerNetwork::Isolated);
        req.input_mounts = vec![InputMount {
            host_dir: PathBuf::from("/"),
        }];
        let error = crate::connection::validate_run_request(&req)
            .expect_err("filesystem root must be rejected")
            .to_string();
        assert!(error.contains("filesystem root"), "{error}");
    }
}
