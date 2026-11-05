//! Per-plugin development container sessions — the VSCode devcontainer
//! attach flow over the container-runtime primitives.
//!
//! One plugin workspace maps to one named, labeled container
//! (`io.autonomics.dev-session=<plugin>`): opening a dev window starts
//! it idempotently, all commands run through `exec` inside the resident
//! container (never a cold-start run), and the lifecycle is operator-
//! driven (stop keeps the container; restart never touches the
//! bind-mounted host workspace). The image is always the plugin
//! manifest's digest-pinned environment reference; the workspace is the
//! single read-write host mount, so "file sync" is the bind mount
//! itself.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use container_runtime::connection::PodmanConnection;
use container_runtime::dev_container::{
    DEV_CONTAINER_PREFIX, DEV_SESSION_LABEL, DevContainerState, DevContainerStatus,
    DevContainerSpec, DevExecStream,
};
use container_runtime::types::{ContainerNetwork, ContainerRunRequest, GpuRequest, PullPolicy, WorkspaceRef};
use container_runtime::{ContainerExecutionInfra, DEFAULT_CONTAINER_WORKDIR, trusted_workspace_ref, unique_container_name};

use crate::{Error, PluginStore, Result};

/// Default dev-session resource envelope. Generous enough for package
/// installs and test suites; egress stays opt-in per start call.
pub const DEV_SESSION_CPUS: f64 = 4.0;
pub const DEV_SESSION_MEMORY: &str = "2Gi";
pub const DEV_SESSION_PIDS_LIMIT: u64 = 512;
/// Cap for one-shot commands inside a session (`exec_once`).
pub const EXEC_ONCE_DEFAULT_TIMEOUT_SECS: u64 = 120;

/// API-facing session snapshot. `unknown` means the runtime probe
/// itself failed (e.g. no podman available) — `detail` carries why, and
/// callers can still render the rest.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DevSessionView {
    pub plugin: String,
    /// `absent` | `created` | `running` | `exited` | `unknown`.
    pub state: String,
    pub container_name: String,
    pub image: Option<String>,
    pub workspace_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Manages the dev container of every plugin workspace.
#[derive(Clone)]
pub struct DevSessionManager {
    pub(crate) execution: Arc<ContainerExecutionInfra>,
    pub(crate) store: Arc<PluginStore>,
}

impl DevSessionManager {
    pub fn new(execution: Arc<ContainerExecutionInfra>, store: Arc<PluginStore>) -> Self {
        Self { execution, store }
    }

    /// Deterministic container name for a plugin (kebab-case name is
    /// already a valid container name).
    pub fn container_name(plugin: &str) -> Result<String> {
        crate::validate_plugin_name(plugin)?;
        Ok(format!("{DEV_CONTAINER_PREFIX}{plugin}"))
    }

    /// Manifest image reference + host workspace path for an existing
    /// project. Unknown projects are an error (the manager only
    /// supervises real workspaces).
    fn project(&self, plugin: &str) -> Result<(String, std::path::PathBuf)> {
        let operator = self
            .store
            .develop(plugin)?
            .ok_or_else(|| Error::Validation(format!("unknown plugin project `{plugin}`")))?;
        Ok((
            operator.manifest().image.reference.as_str().to_string(),
            operator.workspace().path().to_path_buf(),
        ))
    }

    fn view(
        plugin: &str,
        container_name: String,
        image: Option<String>,
        workspace_path: Option<std::path::PathBuf>,
        state: DevContainerState,
        detail: Option<String>,
    ) -> DevSessionView {
        DevSessionView {
            plugin: plugin.to_string(),
            state: state.as_str().to_string(),
            container_name,
            image,
            workspace_path: workspace_path.map(|path| path.display().to_string()),
            detail,
        }
    }

    /// Probe the session without touching it.
    pub async fn status(&self, plugin: &str) -> Result<DevSessionView> {
        let name = Self::container_name(plugin)?;
        let (image, workspace) = match self.project(plugin) {
            Ok(found) => (Some(found.0), Some(found.1)),
            Err(error) => return Err(error),
        };
        let runtime = self.execution.runtime.as_ref();
        match runtime.dev_container_status(&name).await {
            Ok(status) => Ok(Self::view(
                plugin,
                name,
                image,
                workspace,
                status.state,
                status.container_id.map(|id| format!("container id {id}")),
            )),
            // A failed probe (no podman, broken store) is a renderable
            // state, not a transport error.
            Err(error) => Ok(Self::view(
                plugin,
                name,
                image,
                workspace,
                DevContainerState::NotFound,
                Some(error.to_string()),
            ))
            .map(|mut view| {
                view.state = "unknown".into();
                view
            }),
        }
    }

    /// Idempotently ensure the session's container exists and runs.
    pub async fn start(
        &self,
        plugin: &str,
        network: ContainerNetwork,
    ) -> Result<DevSessionView> {
        let name = Self::container_name(plugin)?;
        let (image, workspace) = self.project(plugin)?;
        let spec = self.spec(&name, plugin, image.clone(), &workspace, network)?;
        let runtime = self.execution.runtime.as_ref();
        let status: DevContainerStatus = runtime
            .dev_container_start(&spec)
            .await
            .map_err(|error| Error::Container(error.to_string()))?;
        Ok(Self::view(
            plugin,
            name,
            Some(image),
            Some(workspace),
            status.state,
            status.container_id.map(|id| format!("container id {id}")),
        ))
    }

    /// Stop the container (kept for the next start).
    pub async fn stop(&self, plugin: &str) -> Result<DevSessionView> {
        let name = Self::container_name(plugin)?;
        self.project(plugin)?;
        let runtime = self.execution.runtime.as_ref();
        runtime
            .dev_container_stop(&name)
            .await
            .map_err(|error| Error::Container(error.to_string()))?;
        self.status(plugin).await
    }

    /// Remove + start: the recreate step of the devcontainer loop. The
    /// bind-mounted workspace is untouched by construction.
    pub async fn restart(
        &self,
        plugin: &str,
        network: ContainerNetwork,
    ) -> Result<DevSessionView> {
        let name = Self::container_name(plugin)?;
        self.project(plugin)?;
        let runtime = self.execution.runtime.as_ref();
        runtime
            .dev_container_remove(&name)
            .await
            .map_err(|error| Error::Container(error.to_string()))?;
        self.start(plugin, network).await
    }

    /// Ensure the session is running (starting it on demand like a
    /// devcontainer attach), then open an interactive exec stream.
    ///
    /// `cols`/`rows` seed the pseudo-terminal; the podman CLI cannot
    /// resize a live exec, so callers offer a fresh shell instead.
    pub async fn exec(
        &self,
        plugin: &str,
        tty: bool,
        cols: u16,
        rows: u16,
    ) -> Result<DevExecStream> {
        let name = Self::container_name(plugin)?;
        self.project(plugin)?;
        let runtime = self.execution.runtime.as_ref();
        let status = runtime
            .dev_container_status(&name)
            .await
            .map_err(|error| Error::Container(error.to_string()))?;
        if status.state != DevContainerState::Running {
            self.start(plugin, ContainerNetwork::Isolated).await?;
        }
        let argv = vec!["sh".to_string(), "-l".to_string()];
        let env = vec![
            ("TERM".to_string(), "xterm-256color".to_string()),
            ("COLUMNS".to_string(), cols.to_string()),
            ("LINES".to_string(), rows.to_string()),
        ];
        runtime
            .dev_exec(&name, &argv, &env, tty)
            .await
            .map_err(|error| Error::Container(error.to_string()))
    }

    /// Run one argv inside the resident container (VSCode runs every
    /// one-shot command through exec). Falls back to a one-shot
    /// ephemeral run when no session is running.
    pub async fn exec_once(
        &self,
        plugin: &str,
        argv: &[String],
        env: &[(String, String)],
        timeout_secs: u64,
    ) -> Result<(i32, String, String)> {
        if argv.is_empty() {
            return Err(Error::InvalidRequest("argv cannot be empty".into()));
        }
        let timeout_secs = timeout_secs.clamp(1, 600);
        let name = Self::container_name(plugin)?;
        let (image, workspace) = self.project(plugin)?;
        let runtime = self.execution.runtime.as_ref();
        let probe = runtime
            .dev_container_status(&name)
            .await
            .map_err(|error| Error::Container(error.to_string()))?;
        let running = probe.state == DevContainerState::Running;
        if !running {
            let request = ContainerRunRequest {
                image,
                command: argv.to_vec(),
                workspace: trusted_workspace_ref(&workspace, DEFAULT_CONTAINER_WORKDIR)
                    .map_err(|error| Error::Container(error.to_string()))?,
                env: env.to_vec(),
                panels: Vec::new(),
                input_mounts: Vec::new(),
                network: ContainerNetwork::Isolated,
                read_only_rootfs: true,
                pull_policy: PullPolicy::Missing,
                cpus: Some(2.0),
                memory: Some("1Gi".into()),
                pids_limit: Some(256),
                shm_size: Some("256m".into()),
                gpus: GpuRequest::None,
                user: None,
                timeout_secs,
                name: unique_container_name(),
            };
            let output = runtime
                .run(request)
                .await
                .map_err(|error| Error::Container(error.to_string()))?;
            return Ok((output.exit_code, output.stdout, output.stderr));
        }

        let mut stream = runtime
            .dev_exec(&name, argv, env, false)
            .await
            .map_err(|error| Error::Container(error.to_string()))?;
        use tokio::io::AsyncWriteExt;
        // One-shot: no stdin beyond EOF.
        let _ = stream.stdin.shutdown().await;
        let mut stdout = stream.stdout;
        let stdout_task = tokio::spawn(async move {
            let mut buffer = Vec::new();
            use tokio::io::AsyncReadExt;
            let _ = stdout.read_to_end(&mut buffer).await;
            buffer
        });
        let mut stderr = stream.stderr.take();
        let stderr_task = tokio::spawn(async move {
            let mut buffer = Vec::new();
            use tokio::io::AsyncReadExt;
            if let Some(stderr) = stderr.as_mut() {
                let _ = stderr.read_to_end(&mut buffer).await;
            }
            buffer
        });
        // Bound the wait; on timeout abort the exec task so the child
        // (kill-on-drop) is terminated and never leaks.
        let mut exit = stream.exit;
        let code = match tokio::time::timeout(Duration::from_secs(timeout_secs), &mut exit).await
        {
            Ok(joined) => joined
                .map_err(|error| Error::Container(error.to_string()))?
                .map_err(|error| Error::Container(error.to_string()))?,
            Err(_) => {
                exit.abort();
                return Err(Error::Container(format!(
                    "exec timed out after {timeout_secs}s"
                )));
            }
        };
        let stdout_bytes = stdout_task.await.unwrap_or_default();
        let stderr_bytes = stderr_task.await.unwrap_or_default();
        Ok((
            code,
            container_runtime::connection::truncate_captured_bytes(&stdout_bytes),
            container_runtime::connection::truncate_captured_bytes(&stderr_bytes),
        ))
    }

    fn spec(
        &self,
        name: &str,
        plugin: &str,
        image: String,
        workspace: &std::path::Path,
        network: ContainerNetwork,
    ) -> Result<DevContainerSpec> {
        Ok(DevContainerSpec {
            name: name.to_string(),
            image,
            workspace: WorkspaceRef {
                host_path: workspace.to_path_buf(),
                container_workdir: DEFAULT_CONTAINER_WORKDIR.to_string(),
            },
            network,
            cpus: Some(DEV_SESSION_CPUS),
            memory: Some(DEV_SESSION_MEMORY.into()),
            pids_limit: Some(DEV_SESSION_PIDS_LIMIT),
            env: vec![(
                "AUTONOMICS_PLUGIN_NAME".to_string(),
                plugin.to_string(),
            )],
            labels: vec![(DEV_SESSION_LABEL.to_string(), plugin.to_string())],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use container_runtime::error::ContainerRuntimeError;
    use container_runtime::types::{ContainerRunRequest, ContainerRunResult};
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// Recording fake: the dev lifecycle calls land in `calls`, status is
    /// scripted per test.
    struct FakeRuntime {
        workspace_root: PathBuf,
        calls: Mutex<Vec<String>>,
        states: Mutex<VecDeque<DevContainerState>>,
        specs: Mutex<Vec<DevContainerSpec>>,
    }

    impl FakeRuntime {
        fn next_state(&self) -> DevContainerState {
            self.states.lock().unwrap().pop_front().unwrap_or(DevContainerState::NotFound)
        }
    }

    #[async_trait]
    impl PodmanConnection for FakeRuntime {
        async fn run(
            &self,
            _request: ContainerRunRequest,
        ) -> std::result::Result<ContainerRunResult, ContainerRuntimeError> {
            self.calls.lock().unwrap().push("run".into());
            Ok(ContainerRunResult { exit_code: 0, stdout: "one-shot".into(), stderr: String::new() })
        }

        fn workspace_root(&self) -> &std::path::Path {
            &self.workspace_root
        }

        async fn dev_container_start(
            &self,
            spec: &DevContainerSpec,
        ) -> std::result::Result<DevContainerStatus, ContainerRuntimeError> {
            self.calls.lock().unwrap().push(format!("start:{}", spec.name));
            self.specs.lock().unwrap().push(spec.clone());
            // start always yields Running for the fake.
            self.states.lock().unwrap().push_front(DevContainerState::Running);
            Ok(DevContainerStatus { state: DevContainerState::Running, container_id: Some("abc".into()) })
        }

        async fn dev_container_stop(&self, name: &str) -> std::result::Result<(), ContainerRuntimeError> {
            self.calls.lock().unwrap().push(format!("stop:{name}"));
            Ok(())
        }

        async fn dev_container_remove(&self, name: &str) -> std::result::Result<(), ContainerRuntimeError> {
            self.calls.lock().unwrap().push(format!("remove:{name}"));
            Ok(())
        }

        async fn dev_container_status(
            &self,
            name: &str,
        ) -> std::result::Result<DevContainerStatus, ContainerRuntimeError> {
            self.calls.lock().unwrap().push(format!("status:{name}"));
            Ok(DevContainerStatus { state: self.next_state(), container_id: Some("abc".into()) })
        }
    }

    fn manager_with_plugin() -> (DevSessionManager, Arc<FakeRuntime>, tempfile::TempDir) {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(PluginStore::open(
            root.path(),
            "main",
            "Test Author",
            "test@example.com",
        ));
        // Approve an environment and create the project so the manifest
        // exists (digest-pinned reference is mandatory).
        let environments = crate::EnvironmentRegistry::ephemeral(Default::default());
        environments
            .approve(
                "r-base",
                crate::Environment {
                    reference: "ghcr.io/auto-nomics/environments/r@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
                    interpreters: vec!["Rscript".into(), "sh".into()],
                },
            )
            .unwrap();
        let mut operator = store
            .create("my-plugin", "r-base", "test purpose", &Arc::new(environments.snapshot()))
            .unwrap();
        operator.refresh().unwrap();
        drop(operator);
        let fake = Arc::new(FakeRuntime {
            workspace_root: root.path().join("runs"),
            calls: Mutex::new(Vec::new()),
            states: Mutex::new(VecDeque::new()),
            specs: Mutex::new(Vec::new()),
        });
        let infra = Arc::new(ContainerExecutionInfra {
            runtime: fake.clone(),
            panel_cache: Arc::new(container_runtime::panel::PanelCache::new(
                root.path().join("panels"),
            )),
            config: container_runtime::PodmanConfig {
                program: "unused".into(),
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
            },
        });
        (
            DevSessionManager::new(infra, store),
            fake,
            root,
        )
    }

    #[tokio::test]
    async fn start_builds_labeled_spec_with_manifest_image() {
        let (manager, fake, _root) = manager_with_plugin();
        let view = manager
            .start("my-plugin", ContainerNetwork::Isolated)
            .await
            .unwrap();
        assert_eq!(view.state, "running");
        assert_eq!(view.container_name, "autonomics-dev-my-plugin");
        assert!(view.image.as_deref().unwrap().contains("ghcr.io/"));
        let specs = fake.specs.lock().unwrap();
        assert_eq!(specs.len(), 1);
        let spec = &specs[0];
        assert_eq!(spec.name, "autonomics-dev-my-plugin");
        assert!(
            spec.image.starts_with("ghcr.io/")
                && spec.image.contains("@sha256:"),
            "image must be the manifest's digest-pinned reference"
        );
        assert_eq!(
            spec.workspace.container_workdir, "/work",
            "the workspace is the single rw mount at /work"
        );
        assert!(spec
            .labels
            .iter()
            .any(|(key, value)| key == DEV_SESSION_LABEL && value == "my-plugin"));
        assert_eq!(spec.cpus, Some(DEV_SESSION_CPUS));
    }

    #[tokio::test]
    async fn restart_removes_then_starts_preserving_workspace() {
        let (manager, fake, _root) = manager_with_plugin();
        manager
            .restart("my-plugin", ContainerNetwork::Isolated)
            .await
            .unwrap();
        let calls = fake.calls.lock().unwrap();
        assert!(calls.iter().any(|c| c == "remove:autonomics-dev-my-plugin"));
        assert!(calls.iter().any(|c| c == "start:autonomics-dev-my-plugin"));
        // The workspace path in the new spec is the plugin dev dir.
        let specs = fake.specs.lock().unwrap();
        let workspace = specs.last().unwrap().workspace.host_path.display().to_string();
        assert!(workspace.contains("plugins/dev/my-plugin"), "{workspace}");
    }

    #[tokio::test]
    async fn unknown_project_is_rejected() {
        let (manager, _fake, _root) = manager_with_plugin();
        assert!(manager.start("missing-plugin", ContainerNetwork::Isolated).await.is_err());
        assert!(DevSessionManager::container_name("Bad Name").is_err());
    }

    #[tokio::test]
    async fn status_reports_unknown_when_probe_fails() {
        struct BrokenRuntime;
        #[async_trait]
        impl PodmanConnection for BrokenRuntime {
            async fn run(
                &self,
                _request: ContainerRunRequest,
            ) -> std::result::Result<ContainerRunResult, ContainerRuntimeError> {
                unreachable!()
            }
            fn workspace_root(&self) -> &std::path::Path {
                unreachable!()
            }
            async fn dev_container_status(
                &self,
                _name: &str,
            ) -> std::result::Result<DevContainerStatus, ContainerRuntimeError> {
                Err(ContainerRuntimeError::Invalid("no podman here".into()))
            }
        }
        let (manager, _fake, root) = manager_with_plugin();
        // Rebuild the manager on a broken runtime sharing the same store.
        let store = manager.store.clone();
        let infra = Arc::new(ContainerExecutionInfra {
            runtime: Arc::new(BrokenRuntime),
            panel_cache: Arc::new(container_runtime::panel::PanelCache::new(
                root.path().join("panels"),
            )),
            config: container_runtime::PodmanConfig {
                program: "unused".into(),
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
            },
        });
        let manager = DevSessionManager::new(infra, store);
        let view = manager.status("my-plugin").await.unwrap();
        assert_eq!(view.state, "unknown");
        assert!(view.detail.as_deref().unwrap().contains("no podman"));
    }
}
