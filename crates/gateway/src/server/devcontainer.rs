//! Persistent development containers for plugin projects.
//!
//! A session owns the stdin pipe of an attached `podman run --rm`. Closing the
//! pipe makes the container's idle process exit, so a gateway shutdown also
//! tears down its containers. Sessions always use the daemon's Podman store.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Component, Path, PathBuf},
    process::{Output, Stdio},
    sync::Arc,
    time::Duration,
};

use axum::{
    Json,
    extract::{Path as AxumPath, Query, State},
    http::StatusCode,
};
use container_runtime::PodmanConfig;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::{
    process::Child,
    sync::{Mutex, OwnedMutexGuard},
    time::{sleep, timeout},
};

use super::{
    error::{GatewayError, GatewayResult},
    state::GatewayState,
};

#[derive(Default)]
pub(crate) struct DevContainers {
    sessions: Mutex<HashMap<String, Arc<Mutex<Option<Session>>>>>,
}

struct Session {
    name: String,
    settings: super::devcontainer_config::DevConfig,
    workspace: PathBuf,
    remote_env: BTreeMap<String, String>,
    child: Child,
    terminal: Option<super::devterminal::Terminal>,
    language_servers: HashMap<String, super::devlsp::LanguageServer>,
    vscode: Option<VscodeSession>,
}

struct VscodeSession {
    source: PathBuf,
    _socket_dir: tempfile::TempDir,
    child: Child,
    #[cfg(unix)]
    listener: super::devvscode::Listener,
}

fn vscode_source() -> GatewayResult<Option<PathBuf>> {
    let Some(path) = std::env::var_os("AUTONOMICS_VSCODE_SERVER_DIR") else {
        return Ok(None);
    };
    let path = std::fs::canonicalize(path)
        .map_err(|error| unavailable(format!("无法访问 VS Code 服务目录：{error}")))?;
    if !path.join("bin/code-server-oss").is_file() || !path.join("node").is_file() {
        return Err(unavailable("VS Code 服务目录缺少服务端程序"));
    }
    Ok(Some(path))
}

/// Persistent server data is separate from project files and scratch-GC names.
#[cfg(unix)]
fn vscode_data_dir(config: &PodmanConfig, project: &str) -> GatewayResult<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    plugin_rsi::validate_plugin_name(project)
        .map_err(|error| GatewayError::Message(error.to_string()))?;
    std::fs::create_dir_all(&config.workspace_root)
        .map_err(|error| unavailable(format!("无法创建容器工作目录：{error}")))?;
    let root = config.workspace_root.join(".devcontainer-vscode");
    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o700);
    match builder.create(&root) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(unavailable(format!("无法创建 VS Code 数据目录：{error}"))),
    }
    let metadata = std::fs::symlink_metadata(&root).map_err(|e| unavailable(e.to_string()))?;
    // SAFETY: geteuid has no preconditions and only reads the process identity.
    let uid = unsafe { libc::geteuid() };
    if !metadata.file_type().is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err(unavailable(
            "VS Code 数据目录必须由当前用户持有且仅当前用户可访问",
        ));
    }
    let data = root.join(project);
    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o777);
    match builder.create(&data) {
        Ok(()) => std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o777))
            .map_err(|e| unavailable(e.to_string()))?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(unavailable(format!(
                "无法创建项目 VS Code 数据目录：{error}"
            )));
        }
    }
    let metadata = std::fs::symlink_metadata(&data).map_err(|e| unavailable(e.to_string()))?;
    if !metadata.file_type().is_dir() || metadata.uid() != uid {
        return Err(unavailable("项目 VS Code 数据目录必须是当前用户持有的目录"));
    }
    let data = std::fs::canonicalize(data).map_err(|e| unavailable(e.to_string()))?;
    if data
        .to_str()
        .is_none_or(|path| path.contains([',', '\n', '\r']))
    {
        return Err(unavailable("VS Code 数据目录路径无效"));
    }
    Ok(data)
}

#[cfg(unix)]
async fn start_vscode(
    config: &PodmanConfig,
    container: &str,
    project: &str,
    source: PathBuf,
    socket_dir: tempfile::TempDir,
    port_file: PathBuf,
    remote_user: Option<&str>,
    remote_env: &BTreeMap<String, String>,
) -> GatewayResult<VscodeSession> {
    let mut command = config.async_command();
    command.args(["exec", "--workdir=/work"]);
    if let Some(user) = remote_user {
        command.arg(format!("--user={user}"));
    }
    command.args(
        remote_env
            .iter()
            .map(|(key, value)| format!("--env={key}={value}")),
    );
    command.arg("--env=AUTONOMICS_VSCODE_BRIDGE_DIR=/run/autonomics-vscode");
    let base = format!("/api/v1/plugin-projects/{project}/vscode");
    let mut child = command
        .args([container, "/bin/sh", "-c", "umask 000; exec /opt/autonomics-vscode/bin/code-server-oss --socket-path /run/autonomics-vscode/server.sock --server-data-dir /var/autonomics-vscode-data --without-connection-token --server-base-path \"$1\"", "vscode-server", &base])
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .kill_on_drop(true).spawn()
        .map_err(|error| unavailable(format!("无法启动 VS Code 服务：{error}")))?;
    let socket = socket_dir.path().join("socket/server.sock");
    let ready = timeout(Duration::from_secs(30), async {
        loop {
            if let Some(status) = child.try_wait().map_err(|e| unavailable(e.to_string()))? {
                return Err(unavailable(format!(
                    "VS Code 服务启动失败（{status}），请检查镜像兼容性"
                )));
            }
            if tokio::net::UnixStream::connect(&socket).await.is_ok() {
                return Ok(());
            }
            sleep(Duration::from_millis(150)).await;
        }
    })
    .await;
    match ready {
        Ok(Ok(())) => {
            let listener = super::devvscode::listen(socket, base, &port_file).await?;
            Ok(VscodeSession {
                source,
                _socket_dir: socket_dir,
                child,
                listener,
            })
        }
        Ok(Err(error)) => Err(error),
        Err(_) => Err(unavailable("VS Code 服务启动超时")),
    }
}

fn unavailable(message: impl Into<String>) -> GatewayError {
    GatewayError::Status(StatusCode::SERVICE_UNAVAILABLE, message.into())
}

async fn running(config: &PodmanConfig, name: &str) -> bool {
    config
        .async_command()
        .args(["exec", name, "/bin/sh", "-c", "exit 0"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|status| status.success())
}

/// Keep the session reserved until Podman confirms the container has exited.
async fn stop_session(slot: &mut Option<Session>) -> bool {
    let Some(mut session) = slot.take() else {
        return true;
    };
    drop(session.child.stdin.take());
    if matches!(
        timeout(Duration::from_secs(10), session.child.wait()).await,
        Ok(Ok(_))
    ) {
        true
    } else {
        *slot = Some(session);
        false
    }
}

impl DevContainers {
    #[cfg(unix)]
    async fn vscode_control(
        &self,
        config: &PodmanConfig,
        project: &str,
        input: &[u8],
    ) -> GatewayResult<Value> {
        let mut slot = self.active(project).await?;
        let session = slot.as_mut().ok_or_else(|| unavailable("开发容器未连接"))?;
        if session
            .child
            .try_wait()
            .map_err(|error| unavailable(error.to_string()))?
            .is_some()
        {
            return Err(unavailable("开发容器已退出"));
        }
        if session.vscode.is_none() {
            return Err(unavailable("VS Code 服务未启用"));
        }
        let name = session.name.clone();
        let user = session.settings.remote_user.clone();
        let env = session.remote_env.clone();
        drop(slot);
        let mut command = config.async_command();
        command.args(["exec", "--interactive", "--workdir=/work"]);
        if let Some(user) = user {
            command.arg(format!("--user={user}"));
        }
        command.args(
            env.iter()
                .map(|(key, value)| format!("--env={key}={value}")),
        );
        let mut child = command
            .args([
                &name,
                "/opt/autonomics-vscode/node",
                "/opt/autonomics-vscode/extensions/autonomics-workspace-bridge/client.js",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| unavailable(format!("无法连接 VS Code 扩展宿主：{error}")))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| unavailable("无法连接 VS Code 扩展宿主输入"))?;
        stdin
            .write_all(input)
            .await
            .map_err(|_| unavailable("无法发送 VS Code 操作"))?;
        drop(stdin);
        let output = timeout(Duration::from_secs(5), child.wait_with_output())
            .await
            .map_err(|_| unavailable("VS Code 扩展宿主响应超时"))?
            .map_err(|_| unavailable("无法读取 VS Code 扩展宿主响应"))?;
        if !output.status.success() || output.stdout.len() > 16384 {
            return Err(unavailable("VS Code 工作台尚未就绪或操作失败"));
        }
        let result: Value = serde_json::from_slice(&output.stdout)
            .map_err(|_| unavailable("VS Code 扩展宿主响应无效"))?;
        let slot = self.active(project).await?;
        if slot.as_ref().is_none_or(|session| session.name != name) {
            return Err(unavailable("开发容器已更换，请重试"));
        }
        Ok(result)
    }

    /// Connect while the session lock keeps the selected container alive.
    #[cfg(unix)]
    #[allow(dead_code)] // Used by the isolated VS Code listener when it is added.
    pub(crate) async fn vscode_connection(
        &self,
        project: &str,
    ) -> GatewayResult<tokio::net::UnixStream> {
        plugin_rsi::validate_plugin_name(project)
            .map_err(|error| GatewayError::Message(error.to_string()))?;
        let mut slot = self.active(project).await?;
        let session = slot.as_mut().ok_or_else(|| unavailable("开发容器未连接"))?;
        if session
            .child
            .try_wait()
            .map_err(|error| unavailable(error.to_string()))?
            .is_some()
        {
            return Err(unavailable("开发容器已退出"));
        }
        let server = session
            .vscode
            .as_mut()
            .ok_or_else(|| unavailable("VS Code 服务未启用"))?;
        if server
            .child
            .try_wait()
            .map_err(|error| unavailable(error.to_string()))?
            .is_some()
        {
            return Err(unavailable("VS Code 服务已退出"));
        }
        timeout(
            Duration::from_secs(2),
            tokio::net::UnixStream::connect(server._socket_dir.path().join("socket/server.sock")),
        )
        .await
        .map_err(|_| unavailable("连接 VS Code 服务超时"))?
        .map_err(|_| unavailable("无法连接 VS Code 服务"))
    }

    async fn slot(&self, project: &str, create: bool) -> Option<Arc<Mutex<Option<Session>>>> {
        let mut sessions = self.sessions.lock().await;
        if create {
            Some(
                sessions
                    .entry(project.to_owned())
                    .or_insert_with(|| Arc::new(Mutex::new(None)))
                    .clone(),
            )
        } else {
            sessions.get(project).cloned()
        }
    }

    async fn active(&self, project: &str) -> GatewayResult<OwnedMutexGuard<Option<Session>>> {
        let slot = self
            .slot(project, false)
            .await
            .ok_or_else(|| unavailable("开发容器未连接，请重新打开项目"))?;
        let guard = slot.lock_owned().await;
        if guard
            .as_ref()
            .is_none_or(|session| session.child.stdin.is_none())
        {
            return Err(unavailable("开发容器未连接，请重新打开项目"));
        }
        Ok(guard)
    }

    async fn open_language(
        &self,
        config: &PodmanConfig,
        project: &str,
        language: &str,
    ) -> GatewayResult<String> {
        self.open_service(config, project, language, false).await
    }

    async fn open_service(
        &self,
        config: &PodmanConfig,
        project: &str,
        language: &str,
        debug: bool,
    ) -> GatewayResult<String> {
        let mut session_slot = self.active(project).await?;
        let session = session_slot
            .as_mut()
            .ok_or_else(|| unavailable("开发容器未连接，请重新打开项目"))?;
        if session
            .child
            .try_wait()
            .map_err(|e| unavailable(e.to_string()))?
            .is_some()
        {
            return Err(unavailable("开发容器已退出"));
        }
        session
            .language_servers
            .retain(|_, server| !server.expired());
        if session.language_servers.len() >= 8 {
            return Err(unavailable("语言服务连接数已达上限"));
        }
        let container = session.name.clone();
        let remote_env = session.remote_env.clone();
        let remote_user = session.settings.remote_user.clone();
        drop(session_slot);
        let server = if debug {
            super::devlsp::LanguageServer::start_debug(
                config,
                &container,
                language,
                &remote_env,
                remote_user.as_deref(),
            )
            .await
        } else {
            super::devlsp::LanguageServer::start(
                config,
                &container,
                language,
                &remote_env,
                remote_user.as_deref(),
            )
            .await
        }
        .map_err(unavailable)?;
        let mut session_slot = self.active(project).await?;
        let session = session_slot
            .as_mut()
            .ok_or_else(|| unavailable("开发容器未连接，请重新打开项目"))?;
        if session.name != container
            || session
                .child
                .try_wait()
                .map_err(|e| unavailable(e.to_string()))?
                .is_some()
        {
            return Err(unavailable("开发容器已更换，请重新连接语言服务"));
        }
        session
            .language_servers
            .retain(|_, server| !server.expired());
        if session.language_servers.len() >= 8 {
            return Err(unavailable("语言服务连接数已达上限"));
        }
        let client = uuid::Uuid::new_v4().simple().to_string();
        session.language_servers.insert(
            format!("{}{language}:{client}", if debug { "debug:" } else { "" }),
            server,
        );
        Ok(client)
    }

    async fn connect(
        &self,
        config: &PodmanConfig,
        project: &str,
        image: &str,
        workspace: &Path,
    ) -> GatewayResult<String> {
        self.connect_mode(config, project, image, workspace, false)
            .await
    }

    async fn connect_mode(
        &self,
        config: &PodmanConfig,
        project: &str,
        image: &str,
        workspace: &Path,
        rebuild: bool,
    ) -> GatewayResult<String> {
        let settings = super::devcontainer_config::DevConfig::load(workspace, image)?;
        let vscode_source = vscode_source()?;
        let slot = self
            .slot(project, true)
            .await
            .expect("created session slot");
        let mut session_slot = slot.lock().await;
        if let Some(session) = session_slot.as_mut() {
            if session.child.stdin.is_none() {
                match session.child.try_wait() {
                    Ok(Some(_)) => *session_slot = None,
                    Ok(None) => return Err(unavailable("开发容器正在关闭，请稍后重试")),
                    Err(error) => return Err(unavailable(format!("无法检查开发容器：{error}"))),
                }
            }
        }
        if let Some(session) = session_slot.as_mut() {
            let mut container_settings = settings.clone();
            // remoteEnv belongs to newly spawned remote processes and can change
            // without replacing the container itself.
            container_settings.remote_env = session.settings.remote_env.clone();
            if !rebuild
                && session.settings == container_settings
                && session.workspace == workspace
                && session.vscode.as_ref().map(|server| &server.source) == vscode_source.as_ref()
            {
                match session.child.try_wait() {
                    Ok(None) => {
                        let server_alive = match session.vscode.as_mut() {
                            Some(server) => server
                                .child
                                .try_wait()
                                .map_err(|e| unavailable(e.to_string()))?
                                .is_none(),
                            None => true,
                        };
                        if running(config, &session.name).await && server_alive {
                            let remote_env = settings
                                .resolve_remote_env(config, &session.name, workspace)
                                .await?;
                            settings
                                .run_stages(
                                    config,
                                    &session.name,
                                    4,
                                    &remote_env,
                                    settings.remote_user.as_deref(),
                                )
                                .await?;
                            session.remote_env = remote_env;
                            session.settings = settings;
                            return Ok(session.name.clone());
                        }
                        if server_alive {
                            return Err(unavailable(
                                "开发容器暂时不可访问，现有会话已保留，请稍后重试",
                            ));
                        }
                    }
                    Ok(Some(_)) => {}
                    Err(error) => return Err(unavailable(format!("无法检查开发容器：{error}"))),
                }
            }
        }
        // Keep an existing session alive if the replacement image fails to build.
        let image = settings.build_image(config).await?;
        let (namespace, container_user) = settings.user_settings(config, &image).await?;
        #[cfg(unix)]
        let vscode_data = vscode_source
            .as_ref()
            .map(|_| vscode_data_dir(config, project))
            .transpose()?;
        if !stop_session(&mut session_slot).await {
            return Err(unavailable("旧开发容器仍在关闭，请稍后重试"));
        }
        let mount = workspace
            .to_str()
            .ok_or_else(|| unavailable("项目工作区路径不是有效的 UTF-8"))?;
        if mount.contains([',', '\n', '\r']) {
            return Err(unavailable("项目工作区路径不能包含逗号或换行"));
        }
        let name = format!("autonomics-dev-{}", uuid::Uuid::new_v4().simple());
        #[cfg(unix)]
        let vscode_socket_dir = if let Some(source) = &vscode_source {
            use std::os::unix::fs::PermissionsExt;
            if source.to_string_lossy().contains([',', '\n', '\r']) {
                return Err(unavailable("VS Code 服务路径不能包含逗号或换行"));
            }
            let dir = tempfile::Builder::new()
                .prefix("autonomics-vscode-")
                .tempdir()
                .map_err(|e| unavailable(format!("无法创建 VS Code 通信目录：{e}")))?;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
                .map_err(|e| unavailable(e.to_string()))?;
            let socket = dir.path().join("socket");
            std::fs::create_dir(&socket).map_err(|e| unavailable(e.to_string()))?;
            if socket
                .to_str()
                .is_none_or(|path| path.contains([',', '\n', '\r']))
            {
                return Err(unavailable("VS Code 通信目录路径无效"));
            }
            // The private parent remains 0700 on the host; the mounted child
            // must be writable by an arbitrary configured remoteUser.
            std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o777))
                .map_err(|e| unavailable(e.to_string()))?;
            Some(dir)
        } else {
            None
        };
        #[cfg(not(unix))]
        if vscode_source.is_some() {
            return Err(unavailable("VS Code 容器服务需要 Unix socket"));
        }
        let mut command = config.async_command();
        command.args([
            "run",
            "--rm",
            "--interactive",
            "--name",
            &name,
            "--pull=missing",
            "--security-opt=no-new-privileges",
            "--cap-drop=all",
            "--tmpfs=/tmp:rw,nosuid,nodev",
            "--pids-limit=512",
            "--mount",
            &format!("type=bind,src={mount},dst=/work,rw"),
        ]);
        #[cfg(unix)]
        if let (Some(source), Some(socket_dir)) = (&vscode_source, &vscode_socket_dir) {
            command.arg("--mount").arg(format!(
                "type=bind,src={},dst=/opt/autonomics-vscode,ro",
                source.display()
            ));
            command.arg("--mount").arg(format!(
                "type=bind,src={},dst=/run/autonomics-vscode,rw",
                socket_dir.path().join("socket").display()
            ));
            let data = vscode_data
                .as_ref()
                .ok_or_else(|| unavailable("VS Code 数据目录不可用"))?;
            command.arg("--mount").arg(format!(
                "type=bind,src={},dst=/var/autonomics-vscode-data,rw",
                data.display()
            ));
        }
        command.arg(namespace.as_deref().unwrap_or("--userns=keep-id"));
        command.arg(if container_user.is_some() {
            "--workdir=/"
        } else {
            "--workdir=/work"
        });
        if let Some(user) = &container_user {
            command.arg(format!("--user={user}"));
        }
        for (key, value) in &settings.env {
            command.arg(format!("--env={key}={value}"));
        }
        let mut child = command
            .args(["--entrypoint=/bin/sh", &image, "-c", "cat >/dev/null"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // Closing stdin lets the container exit and Podman perform --rm.
            // Killing the attached Podman process first can strand a container.
            .kill_on_drop(false)
            .spawn()
            .map_err(|error| unavailable(format!("无法启动开发容器：{error}")))?;
        let result = timeout(Duration::from_secs(120), async {
            loop {
                if let Some(status) = child
                    .try_wait()
                    .map_err(|error| unavailable(format!("无法检查开发容器：{error}")))?
                {
                    return Err(unavailable(format!(
                        "开发容器启动失败（{status}）；请检查镜像和容器运行环境"
                    )));
                }
                if running(config, &name).await {
                    return Ok(());
                }
                sleep(Duration::from_millis(300)).await;
            }
        })
        .await;
        match result {
            Ok(Ok(())) => {
                let remote_env = settings
                    .resolve_remote_env(config, &name, workspace)
                    .await?;
                settings
                    .run_stages(
                        config,
                        &name,
                        0,
                        &remote_env,
                        settings.remote_user.as_deref(),
                    )
                    .await?;
                #[cfg(unix)]
                let vscode = match (vscode_source, vscode_socket_dir) {
                    (Some(source), Some(socket_dir)) => Some(
                        start_vscode(
                            config,
                            &name,
                            project,
                            source,
                            socket_dir,
                            vscode_data
                                .as_ref()
                                .and_then(|path| path.parent())
                                .ok_or_else(|| unavailable("VS Code 数据目录不可用"))?
                                .join(format!(".port-{project}")),
                            settings.remote_user.as_deref(),
                            &remote_env,
                        )
                        .await?,
                    ),
                    _ => None,
                };
                #[cfg(not(unix))]
                let vscode = None;
                *session_slot = Some(Session {
                    name: name.clone(),
                    settings,
                    workspace: workspace.to_path_buf(),
                    remote_env,
                    child,
                    terminal: None,
                    language_servers: HashMap::new(),
                    vscode,
                });
                Ok(name)
            }
            Ok(Err(error)) => Err(error),
            Err(_) => Err(unavailable("开发容器启动超时")),
        }
    }

    async fn close(&self, project: &str) -> bool {
        let Some(slot) = self.slot(project, false).await else {
            return true;
        };
        let mut guard = slot.lock().await;
        stop_session(&mut guard).await
    }

    async fn is_connected(&self, config: &PodmanConfig, project: &str) -> bool {
        let Some(slot) = self.slot(project, false).await else {
            return false;
        };
        let mut session_slot = slot.lock().await;
        let Some(session) = session_slot.as_mut() else {
            return false;
        };
        match session.child.try_wait() {
            Ok(Some(_)) => {
                *session_slot = None;
                false
            }
            Ok(None) if session.child.stdin.is_none() => false,
            Ok(None) => {
                let server_alive = session
                    .vscode
                    .as_mut()
                    .is_none_or(|server| matches!(server.child.try_wait(), Ok(None)));
                server_alive && running(config, &session.name).await
            }
            Err(_) => false,
        }
    }

    pub(crate) async fn exec(
        &self,
        config: &PodmanConfig,
        project: &str,
        script: &str,
        args: &[&str],
        input: &[u8],
    ) -> GatewayResult<Output> {
        let mut session_slot = self.active(project).await?;
        let session = session_slot
            .as_mut()
            .ok_or_else(|| unavailable("开发容器未连接，请重新打开项目"))?;
        if session
            .child
            .try_wait()
            .map_err(|error| unavailable(error.to_string()))?
            .is_some()
        {
            *session_slot = None;
            return Err(unavailable("开发容器已退出，请重新打开项目"));
        }
        let mut command = config.async_command();
        command.args(["exec", "--interactive"]);
        if let Some(user) = &session.settings.remote_user {
            command.arg(format!("--user={user}"));
        }
        command.args(
            session
                .remote_env
                .iter()
                .map(|(key, value)| format!("--env={key}={value}")),
        );
        command.args([&session.name, "/bin/sh", "-c", script, "autonomics-file"]);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|error| unavailable(format!("无法访问开发容器：{error}")))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| unavailable("无法连接开发容器输入"))?;
        let writer = tokio::spawn({
            let input = input.to_vec();
            async move { stdin.write_all(&input).await }
        });
        let output = timeout(Duration::from_secs(30), child.wait_with_output())
            .await
            .map_err(|_| unavailable("开发容器文件操作超时"))?
            .map_err(|error| unavailable(format!("开发容器文件操作失败：{error}")))?;
        let write_result = writer
            .await
            .map_err(|error| unavailable(error.to_string()))?;
        if output.status.success() {
            write_result.map_err(|error| unavailable(error.to_string()))?;
        }
        Ok(output)
    }

    /// A read-only scan must not hold the session lock while a large tree is traversed.
    pub(crate) async fn snapshot(
        &self,
        config: &PodmanConfig,
        project: &str,
    ) -> GatewayResult<Output> {
        let mut slot = self.active(project).await?;
        let session = slot.as_mut().ok_or_else(|| unavailable("开发容器未连接"))?;
        if session
            .child
            .try_wait()
            .map_err(|error| unavailable(error.to_string()))?
            .is_some()
        {
            return Err(unavailable("开发容器已退出"));
        }
        let name = session.name.clone();
        let user = session.settings.remote_user.clone();
        let env = session.remote_env.clone();
        drop(slot);
        let mut command = config.async_command();
        command.arg("exec");
        if let Some(user) = user {
            command.arg(format!("--user={user}"));
        }
        command.args(
            env.iter()
                .map(|(key, value)| format!("--env={key}={value}")),
        );
        let mut child = command
            .args([&name, "/bin/sh", "-c", SNAPSHOT_SCRIPT])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| unavailable(format!("无法读取开发容器文件快照：{error}")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| unavailable("文件快照输出不可用"))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| unavailable("文件快照错误输出不可用"))?;
        let stderr_task = tokio::spawn(async move {
            let mut captured = Vec::new();
            let mut chunk = [0u8; 4096];
            while let Ok(size) = stderr.read(&mut chunk).await {
                if size == 0 {
                    break;
                }
                let remaining = 16384usize.saturating_sub(captured.len());
                captured.extend_from_slice(&chunk[..size.min(remaining)]);
            }
            captured
        });
        timeout(Duration::from_secs(30), async {
            let mut bytes = Vec::new();
            stdout
                .take(8 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .await
                .map_err(|error| unavailable(format!("读取文件快照失败：{error}")))?;
            if bytes.len() > 8 * 1024 * 1024 {
                let _ = child.kill().await;
                return Err(unavailable("项目文件监控快照过大"));
            }
            let status = child
                .wait()
                .await
                .map_err(|error| unavailable(format!("文件监控进程失败：{error}")))?;
            let stderr = stderr_task.await.unwrap_or_default();
            Ok(Output {
                status,
                stdout: bytes,
                stderr,
            })
        })
        .await
        .map_err(|_| unavailable("开发容器文件监控超时"))?
    }
}

pub(crate) fn validate_file_path(path: &str) -> GatewayResult<()> {
    if path.is_empty()
        || path.contains('\0')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || !Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(name) if name != ".git"))
    {
        return Err(GatewayError::Status(
            StatusCode::BAD_REQUEST,
            "无效的项目文件路径".into(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_directory_move(from: &str, to: &str) -> GatewayResult<()> {
    validate_file_path(from)?;
    validate_file_path(to)?;
    if Path::new(to).starts_with(Path::new(from)) {
        return Err(GatewayError::Message(
            "不能将文件夹移动到自身或其子目录".into(),
        ));
    }
    Ok(())
}

/// Verify each path component inside the mounted workspace before access.
/// The caller validates the relative path; the shell receives it as argv.
pub(crate) const CHECK_PATH: &str = r#"
set -eu
path=$1
rest=$path
current=/work
while :; do
  part=${rest%%/*}
  current=$current/$part
  if [ -L "$current" ]; then echo '不允许访问符号链接' >&2; exit 2; fi
  case "$rest" in */*) rest=${rest#*/} ;; *) break ;; esac
done
"#;

pub(crate) const LIST_SCRIPT: &str = r#"set -eu
cd /work
find . \( -name .git -o -name .venv \) -prune -o -type f -print0"#;

pub(crate) const LIST_DIRECTORIES_SCRIPT: &str = r#"set -eu
cd /work
find . \( -name .git -o -name .venv \) -prune -o -type d -print0"#;

// A portable, NUL-delimited snapshot made inside the development container.
// Dependency and VCS directories are excluded to keep periodic scans bounded.
pub(crate) const SNAPSHOT_SCRIPT: &str = r#"set -eu
cd /work
if find . -maxdepth 0 -printf '' >/dev/null 2>&1; then
  find . \( -name .git -o -name .venv -o -name node_modules \) -prune -o \( -type f -o -type d \) -printf '%y\0%p\0%T@\0%s\0'
else
  find . \( -name .git -o -name .venv -o -name node_modules \) -prune -o \( -type f -o -type d \) -exec sh -c '
    for file do
      if [ -d "$file" ]; then kind=d; else kind=f; fi
      metadata=$(stat -c "%Y:%s" "$file") || exit 1
      printf "%s\0%s\0%s\0%s\0" "$kind" "$file" "$metadata" ""
    done
  ' sh {} +
fi"#;

pub(crate) fn read_script() -> String {
    format!(
        "{}\n[ -f \"$current\" ] || {{ echo '文件不存在' >&2; exit 2; }}\n[ \"$(wc -c < \"$current\")\" -le 1048576 ] || {{ echo '文件超过大小限制' >&2; exit 2; }}\ncat -- \"$current\"",
        CHECK_PATH
    )
}

pub(crate) fn create_script() -> String {
    format!(
        r#"{}
[ ! -e "$current" ] || exit 42
(set -C; : > "$current") || {{ echo '无法创建文件' >&2; exit 2; }}
"#,
        CHECK_PATH
    )
}

pub(crate) fn create_directory_script() -> String {
    format!(
        r#"{}
[ ! -e "$current" ] || exit 42
mkdir -- "$current"
"#,
        CHECK_PATH
    )
}

pub(crate) fn rename_directory_script() -> String {
    format!(
        r#"{}
source=$current
[ -d "$source" ] || {{ echo '文件夹不存在' >&2; exit 2; }}
set -- "$2"
{}
[ ! -e "$current" ] || exit 42
mv -T -n -- "$source" "$current"
[ ! -e "$source" ] || exit 42
"#,
        CHECK_PATH, CHECK_PATH
    )
}

pub(crate) fn delete_directory_script() -> String {
    format!(
        r#"{}
[ -d "$current" ] || {{ echo '文件夹不存在' >&2; exit 2; }}
rm -r -- "$current"
"#,
        CHECK_PATH
    )
}

pub(crate) fn delete_script() -> String {
    format!(
        r#"{}
[ -f "$current" ] || {{ echo '文件不存在' >&2; exit 2; }}
rm -- "$current"
"#,
        CHECK_PATH
    )
}

pub(crate) fn rename_script() -> String {
    format!(
        r#"{}
source=$current
[ -f "$source" ] || {{ echo '文件不存在' >&2; exit 2; }}
set -- "$2"
{}
[ ! -e "$current" ] || exit 42
ln -- "$source" "$current" || {{ echo '无法重命名文件' >&2; exit 2; }}
if ! rm -- "$source"; then rm -f -- "$current"; echo '无法重命名文件' >&2; exit 2; fi
"#,
        CHECK_PATH, CHECK_PATH
    )
}

pub(crate) fn save_script() -> String {
    format!(
        r#"{}
[ -f "$current" ] || {{ echo '文件不存在' >&2; exit 2; }}
expected=$(mktemp /tmp/autonomics-expected.XXXXXXXX)
temp=$(mktemp "$(dirname "$current")/.autonomics-edit.XXXXXXXX")
trap 'rm -f "$expected" "$temp"' EXIT HUP INT TERM
dd bs=1 count="$2" of="$expected" 2>/dev/null
cat > "$temp"
cmp -s "$current" "$expected" || exit 42
chmod "$(stat -c %a "$current")" "$temp"
mv -f "$temp" "$current"
"#,
        CHECK_PATH
    )
}

#[derive(Default, serde::Deserialize)]
pub(crate) struct OpenOptions {
    #[serde(default)]
    rebuild: bool,
}

pub(crate) async fn open(
    State(state): State<GatewayState>,
    AxumPath(project): AxumPath<String>,
    Query(options): Query<OpenOptions>,
) -> GatewayResult<Json<Value>> {
    let store = state.infra.rsi.store();
    let operator = store
        .develop(&project)
        .map_err(|error| GatewayError::Message(error.to_string()))?
        .ok_or_else(|| {
            GatewayError::Status(StatusCode::NOT_FOUND, "项目不存在或 manifest 无效".into())
        })?;
    let image = operator.manifest().image.reference.as_str().to_owned();
    let workspace = store.root().join(&project);
    if !std::fs::symlink_metadata(&workspace)
        .map(|metadata| metadata.file_type().is_dir())
        .unwrap_or(false)
    {
        return Err(GatewayError::Message("项目工作区必须是目录".into()));
    }
    let name = if options.rebuild {
        state
            .devcontainers
            .connect_mode(
                &state.infra.container_execution.config,
                &project,
                &image,
                &workspace,
                true,
            )
            .await?
    } else {
        state
            .devcontainers
            .connect(
                &state.infra.container_execution.config,
                &project,
                &image,
                &workspace,
            )
            .await?
    };
    Ok(Json(
        json!({"project": project, "container": name, "status": "connected"}),
    ))
}

pub(crate) async fn close(
    State(state): State<GatewayState>,
    AxumPath(project): AxumPath<String>,
) -> GatewayResult<Json<Value>> {
    plugin_rsi::validate_plugin_name(&project)
        .map_err(|error| GatewayError::Message(error.to_string()))?;
    if !state.devcontainers.close(&project).await {
        return Err(unavailable("开发容器仍在关闭，请稍后重试"));
    }
    Ok(Json(json!({"project": project, "status": "disconnected"})))
}

pub(crate) async fn status(
    State(state): State<GatewayState>,
    AxumPath(project): AxumPath<String>,
) -> GatewayResult<Json<Value>> {
    plugin_rsi::validate_plugin_name(&project)
        .map_err(|error| GatewayError::Message(error.to_string()))?;
    let connected = state
        .devcontainers
        .is_connected(&state.infra.container_execution.config, &project)
        .await;
    Ok(Json(json!({"project": project, "connected": connected})))
}

/// Issue the local workbench URL only to an authenticated local gateway client.
#[cfg(unix)]
pub(crate) async fn vscode_url(
    State(state): State<GatewayState>,
    AxumPath(project): AxumPath<String>,
    headers: axum::http::HeaderMap,
) -> GatewayResult<axum::response::Response> {
    use axum::response::IntoResponse;
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(':').next());
    if !matches!(host, Some("127.0.0.1" | "localhost")) {
        return Err(unavailable(
            "VS Code 本地工作台仅支持通过 localhost 或 127.0.0.1 访问网关",
        ));
    }
    plugin_rsi::validate_plugin_name(&project)
        .map_err(|error| GatewayError::Message(error.to_string()))?;
    let mut slot = state.devcontainers.active(&project).await?;
    let session = slot.as_mut().ok_or_else(|| unavailable("开发容器未连接"))?;
    if session
        .child
        .try_wait()
        .map_err(|error| unavailable(error.to_string()))?
        .is_some()
    {
        return Err(unavailable("开发容器已退出"));
    }
    let server = session
        .vscode
        .as_mut()
        .ok_or_else(|| unavailable("VS Code 服务未启用"))?;
    if server
        .child
        .try_wait()
        .map_err(|error| unavailable(error.to_string()))?
        .is_some()
        || server.listener.task.is_finished()
    {
        return Err(unavailable("VS Code 服务已退出"));
    }
    Ok((
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Json(json!({"url": server.listener.url})),
    )
        .into_response())
}

#[cfg(not(unix))]
pub(crate) async fn vscode_url() -> GatewayResult<axum::response::Response> {
    Err(unavailable("VS Code 容器服务需要 Unix socket"))
}

#[derive(serde::Deserialize)]
pub(crate) struct VscodeControl {
    action: String,
    path: Option<String>,
    line: Option<u32>,
    column: Option<u32>,
}

#[cfg(unix)]
pub(crate) async fn vscode_control(
    State(state): State<GatewayState>,
    AxumPath(project): AxumPath<String>,
    Json(request): Json<VscodeControl>,
) -> GatewayResult<Json<Value>> {
    plugin_rsi::validate_plugin_name(&project)
        .map_err(|error| GatewayError::Message(error.to_string()))?;
    match request.action.as_str() {
        "state" if request.path.is_none() && request.line.is_none() && request.column.is_none() => {
        }
        "open" => {
            let path = request
                .path
                .as_deref()
                .ok_or_else(|| GatewayError::Message("缺少文件路径".into()))?;
            validate_file_path(path)?;
            if request
                .line
                .is_some_and(|value| value == 0 || value > 10_000_000)
                || request
                    .column
                    .is_some_and(|value| value == 0 || value > 10_000_000)
            {
                return Err(GatewayError::Message("文件位置无效".into()));
            }
        }
        _ => return Err(GatewayError::Message("不支持的 VS Code 操作".into())),
    }
    let input = serde_json::to_vec(&json!({"action":request.action, "path":request.path, "line":request.line, "column":request.column}))
        .map_err(|_| GatewayError::Message("VS Code 操作无效".into()))?;
    if input.len() > 16384 {
        return Err(GatewayError::Message("VS Code 操作过大".into()));
    }
    let result = state
        .devcontainers
        .vscode_control(&state.infra.container_execution.config, &project, &input)
        .await?;
    Ok(Json(result))
}

#[cfg(not(unix))]
pub(crate) async fn vscode_control() -> GatewayResult<Json<Value>> {
    Err(unavailable("VS Code 容器服务需要 Unix socket"))
}

#[derive(serde::Deserialize)]
pub(crate) struct TerminalOffset {
    #[serde(default)]
    offset: u64,
}

#[derive(serde::Deserialize)]
pub(crate) struct TerminalInput {
    data: String,
}

#[derive(serde::Deserialize)]
pub(crate) struct TerminalSize {
    rows: u16,
    cols: u16,
}

pub(crate) async fn terminal_open(
    State(state): State<GatewayState>,
    AxumPath(project): AxumPath<String>,
) -> GatewayResult<Json<Value>> {
    let mut session_slot = state.devcontainers.active(&project).await?;
    let session = session_slot
        .as_mut()
        .ok_or_else(|| unavailable("开发容器未连接，请重新打开项目"))?;
    if session
        .child
        .try_wait()
        .map_err(|error| unavailable(error.to_string()))?
        .is_some()
    {
        return Err(unavailable("开发容器已退出，请重新打开项目"));
    }
    let restart = match session.terminal.as_mut() {
        Some(terminal) => terminal.exited().map_err(unavailable)?,
        None => true,
    };
    if restart {
        session.terminal = Some(
            super::devterminal::Terminal::start(
                &state.infra.container_execution.config,
                &session.name,
                24,
                80,
                &session.remote_env,
                session.settings.remote_user.as_deref(),
            )
            .map_err(unavailable)?,
        );
    }
    Ok(Json(json!({"project": project})))
}

pub(crate) async fn terminal_output(
    State(state): State<GatewayState>,
    AxumPath(project): AxumPath<String>,
    Query(query): Query<TerminalOffset>,
) -> GatewayResult<Json<Value>> {
    let mut session_slot = state.devcontainers.active(&project).await?;
    let terminal = session_slot
        .as_mut()
        .and_then(|session| session.terminal.as_mut())
        .ok_or_else(|| unavailable("终端未连接"))?;
    let (data, offset, truncated) = terminal.read(query.offset);
    let exited = terminal.exited().map_err(unavailable)? && terminal.output_closed();
    Ok(Json(
        json!({"data": data, "offset": offset, "truncated": truncated, "exited": exited}),
    ))
}

pub(crate) async fn terminal_input(
    State(state): State<GatewayState>,
    AxumPath(project): AxumPath<String>,
    Json(input): Json<TerminalInput>,
) -> GatewayResult<Json<Value>> {
    let mut session_slot = state.devcontainers.active(&project).await?;
    let terminal = session_slot
        .as_mut()
        .and_then(|session| session.terminal.as_mut())
        .ok_or_else(|| unavailable("终端未连接"))?;
    if terminal.exited().map_err(unavailable)? {
        return Err(unavailable("终端已退出"));
    }
    terminal.write(input.data).map_err(unavailable)?;
    Ok(Json(json!({"ok": true})))
}

pub(crate) async fn terminal_resize(
    State(state): State<GatewayState>,
    AxumPath(project): AxumPath<String>,
    Json(size): Json<TerminalSize>,
) -> GatewayResult<Json<Value>> {
    let mut session_slot = state.devcontainers.active(&project).await?;
    let terminal = session_slot
        .as_mut()
        .and_then(|session| session.terminal.as_mut())
        .ok_or_else(|| unavailable("终端未连接"))?;
    terminal.resize(size.rows, size.cols).map_err(unavailable)?;
    Ok(Json(json!({"ok": true})))
}

pub(crate) async fn terminal_close(
    State(state): State<GatewayState>,
    AxumPath(project): AxumPath<String>,
) -> GatewayResult<Json<Value>> {
    if let Some(slot) = state.devcontainers.slot(&project, false).await {
        if let Some(session) = slot.lock().await.as_mut() {
            session.terminal.take();
        }
    }
    Ok(Json(json!({"ok": true})))
}

pub(crate) async fn language_open(
    State(state): State<GatewayState>,
    AxumPath((project, language)): AxumPath<(String, String)>,
) -> GatewayResult<Json<Value>> {
    let client = state
        .devcontainers
        .open_language(&state.infra.container_execution.config, &project, &language)
        .await?;
    Ok(Json(json!({"client": client})))
}

pub(crate) async fn language_output(
    State(state): State<GatewayState>,
    AxumPath((project, language, client)): AxumPath<(String, String, String)>,
    Query(query): Query<TerminalOffset>,
) -> GatewayResult<Json<Value>> {
    let mut session_slot = state.devcontainers.active(&project).await?;
    let server = session_slot
        .as_mut()
        .and_then(|session| {
            session
                .language_servers
                .get_mut(&format!("{language}:{client}"))
        })
        .ok_or_else(|| unavailable("语言服务未连接"))?;
    let (messages, offset, truncated, exited) = server.poll(query.offset).map_err(unavailable)?;
    Ok(Json(
        json!({"messages": messages, "offset": offset, "truncated": truncated, "exited": exited}),
    ))
}

pub(crate) async fn language_input(
    State(state): State<GatewayState>,
    AxumPath((project, language, client)): AxumPath<(String, String, String)>,
    Json(message): Json<Value>,
) -> GatewayResult<Json<Value>> {
    let mut session_slot = state.devcontainers.active(&project).await?;
    let server = session_slot
        .as_mut()
        .and_then(|session| {
            session
                .language_servers
                .get_mut(&format!("{language}:{client}"))
        })
        .ok_or_else(|| unavailable("语言服务未连接"))?;
    server.send(message).await.map_err(unavailable)?;
    Ok(Json(json!({"ok": true})))
}

pub(crate) async fn language_close(
    State(state): State<GatewayState>,
    AxumPath((project, language, client)): AxumPath<(String, String, String)>,
) -> GatewayResult<Json<Value>> {
    if let Some(slot) = state.devcontainers.slot(&project, false).await {
        if let Some(session) = slot.lock().await.as_mut() {
            session
                .language_servers
                .remove(&format!("{language}:{client}"));
        }
    }
    Ok(Json(json!({"ok": true})))
}

pub(crate) async fn debug_open(
    State(state): State<GatewayState>,
    AxumPath((project, language)): AxumPath<(String, String)>,
) -> GatewayResult<Json<Value>> {
    let client = state
        .devcontainers
        .open_service(
            &state.infra.container_execution.config,
            &project,
            &language,
            true,
        )
        .await?;
    Ok(Json(json!({"client": client})))
}

pub(crate) async fn debug_output(
    State(state): State<GatewayState>,
    AxumPath((project, language, client)): AxumPath<(String, String, String)>,
    Query(query): Query<TerminalOffset>,
) -> GatewayResult<Json<Value>> {
    language_output(
        State(state),
        AxumPath((project, format!("debug:{language}"), client)),
        Query(query),
    )
    .await
}

pub(crate) async fn debug_input(
    State(state): State<GatewayState>,
    AxumPath((project, language, client)): AxumPath<(String, String, String)>,
    Json(message): Json<Value>,
) -> GatewayResult<Json<Value>> {
    language_input(
        State(state),
        AxumPath((project, format!("debug:{language}"), client)),
        Json(message),
    )
    .await
}

pub(crate) async fn debug_close(
    State(state): State<GatewayState>,
    AxumPath((project, language, client)): AxumPath<(String, String, String)>,
) -> GatewayResult<Json<Value>> {
    language_close(
        State(state),
        AxumPath((project, format!("debug:{language}"), client)),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn debug_message(
        server: &mut super::super::devlsp::LanguageServer,
        offset: &mut u64,
        pending: &mut Vec<Value>,
        predicate: impl Fn(&Value) -> bool,
    ) -> Value {
        timeout(Duration::from_secs(30), async {
            loop {
                if let Some(index) = pending.iter().position(&predicate) {
                    return pending.remove(index);
                }
                let (messages, next, truncated, exited) = server.poll(*offset).unwrap();
                assert!(
                    !truncated && !exited,
                    "debug adapter ended before expected message: {pending:?}"
                );
                *offset = next;
                pending.extend(messages);
                sleep(Duration::from_millis(30)).await;
            }
        })
        .await
        .expect("debug adapter response")
    }

    #[test]
    fn file_paths_stay_inside_workspace() {
        for path in [
            "",
            "../secret",
            "/etc/passwd",
            ".git/config",
            "a/.git/config",
            "a//b",
            "a/./b",
            "a/",
            "a\0b",
        ] {
            assert!(validate_file_path(path).is_err(), "{path:?}");
        }
        for path in [
            "manifest.toml",
            "src/main.py",
            ".rsi/reports/test.json",
            "目录/a b.R",
        ] {
            assert!(validate_file_path(path).is_ok(), "{path:?}");
        }
    }

    #[test]
    fn directory_move_cannot_target_itself_or_descendants() {
        assert!(validate_directory_move("src", "src").is_err());
        assert!(validate_directory_move("src", "src/nested").is_err());
        assert!(validate_directory_move("src", "src/../elsewhere").is_err());
        assert!(validate_directory_move("src", "src2").is_ok());
    }

    #[tokio::test]
    async fn file_access_requires_a_session() {
        let manager = DevContainers::default();
        let result = manager
            .exec(&PodmanConfig::default(), "missing", "exit 0", &[], &[])
            .await;
        assert!(matches!(
            result,
            Err(GatewayError::Status(StatusCode::SERVICE_UNAVAILABLE, _))
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn closing_session_stays_reserved_until_container_exits() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("closing");
        let child = tokio::process::Command::new("/bin/sh")
            .args(["-c", "cat >/dev/null; : > \"$1\"; sleep 1", "test"])
            .arg(&marker)
            .stdin(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let manager = Arc::new(DevContainers::default());
        let slot = manager.slot("project", true).await.unwrap();
        *slot.lock().await = Some(Session {
            name: "container".into(),
            settings: super::super::devcontainer_config::DevConfig::load(dir.path(), "image")
                .unwrap_or_else(|_| panic!("default config")),
            workspace: dir.path().to_path_buf(),
            remote_env: BTreeMap::new(),
            child,
            terminal: None,
            language_servers: HashMap::new(),
            vscode: None,
        });
        let closing = {
            let manager = manager.clone();
            tokio::spawn(async move { manager.close("project").await })
        };
        timeout(Duration::from_secs(5), async {
            while !marker.exists() {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(
            timeout(Duration::from_millis(150), manager.active("project"))
                .await
                .is_err()
        );
        assert!(closing.await.unwrap());
        assert!(manager.active("project").await.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn vscode_data_is_private_separate_and_rejects_symlinks() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let mut config = PodmanConfig::default();
        config.workspace_root = dir.path().join("workspace");
        let alpha = vscode_data_dir(&config, "alpha").unwrap_or_else(|_| panic!("alpha data"));
        let beta = vscode_data_dir(&config, "beta").unwrap_or_else(|_| panic!("beta data"));
        assert_ne!(alpha, beta);
        assert_eq!(
            vscode_data_dir(&config, "alpha").unwrap_or_else(|_| panic!("reuse data")),
            alpha
        );
        assert_eq!(
            std::fs::metadata(alpha.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert!(vscode_data_dir(&config, "../escape").is_err());
        symlink(dir.path(), alpha.parent().unwrap().join("linked")).unwrap();
        assert!(vscode_data_dir(&config, "linked").is_err());
        std::fs::set_permissions(
            alpha.parent().unwrap(),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        assert!(vscode_data_dir(&config, "alpha").is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "requires a local test image, Podman state and AUTONOMICS_VSCODE_SERVER_DIR"]
    async fn real_container_starts_vscode_in_project_session() {
        use std::os::unix::fs::PermissionsExt;
        let state = std::env::var("AUTONOMICS_DEVCONTAINER_TEST_STATE").expect("test state");
        let image = std::env::var("AUTONOMICS_DEVCONTAINER_TEST_IMAGE").expect("test image");
        assert!(std::env::var_os("AUTONOMICS_VSCODE_SERVER_DIR").is_some());
        let mut config =
            container_runtime::ContainerExecutionInfra::for_operator_state_dir(Path::new(&state))
                .expect("container configuration")
                .config;
        let runtime_dir = tempfile::tempdir().unwrap();
        config.workspace_root = runtime_dir.path().join("runtime");
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(
            workspace.path().join("hello.py"),
            "print('container workbench')\n",
        )
        .unwrap();
        std::fs::write(
            workspace.path().join("second.py"),
            "print('bridge opened file')\n",
        )
        .unwrap();
        let manager = DevContainers::default();
        manager
            .connect(&config, "vscode-test", &image, workspace.path())
            .await
            .unwrap_or_else(|_| panic!("connect VS Code container"));
        let (socket_dir, workbench_url) = {
            let slot = manager
                .active("vscode-test")
                .await
                .unwrap_or_else(|_| panic!("active VS Code container"));
            let vscode = slot.as_ref().unwrap().vscode.as_ref().unwrap();
            (
                vscode._socket_dir.path().to_owned(),
                vscode.listener.url.clone(),
            )
        };
        assert_eq!(
            std::fs::metadata(&socket_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let stream = manager
            .vscode_connection("vscode-test")
            .await
            .unwrap_or_else(|_| panic!("connect to VS Code socket"));
        let request = axum::http::Request::builder()
            .uri("/api/v1/plugin-projects/vscode-test/vscode/?folder=/work")
            .header("host", "localhost")
            .body(axum::body::Body::empty())
            .unwrap();
        let response = timeout(
            Duration::from_secs(30),
            super::super::devvscode::forward(stream, request),
        )
        .await
        .unwrap()
        .unwrap_or_else(|_| panic!("proxy VS Code response"));
        assert!(
            response.status() == StatusCode::OK || response.status() == StatusCode::FOUND,
            "{}",
            response.status()
        );
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let response = client.get(&workbench_url).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("referrer-policy").unwrap(),
            "no-referrer"
        );
        let html = response.text().await.unwrap();
        assert!(html.contains("workbench"));
        let origin = workbench_url
            .split('/')
            .take(3)
            .collect::<Vec<_>>()
            .join("/");
        assert_eq!(
            client
                .get(format!(
                    "{origin}/wrong/api/v1/plugin-projects/vscode-test/vscode/"
                ))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            client
                .get(&workbench_url)
                .header("host", "attacker.example")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            client
                .get(&workbench_url)
                .header("connection", "Upgrade")
                .header("upgrade", "websocket")
                .header("origin", "http://attacker.example")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        if std::env::var_os("AUTONOMICS_DEVCONTAINER_TEST_BROWSER").is_some() {
            let parent_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let parent_url = format!("http://{}/", parent_listener.local_addr().unwrap());
            let parent_html = format!(
                r#"<iframe src="{workbench_url}" sandbox="allow-scripts allow-same-origin allow-forms allow-modals allow-popups allow-downloads allow-pointer-lock" style="width:1400px;height:900px"></iframe>"#
            );
            let parent_app = axum::Router::new().route(
                "/",
                axum::routing::get(move || {
                    let html = parent_html.clone();
                    async move { axum::response::Html(html) }
                }),
            );
            let parent_task =
                tokio::spawn(
                    async move { axum::serve(parent_listener, parent_app).await.unwrap() },
                );
            let script = r#"const {chromium}=require('playwright'); const fs=require('node:fs'); const path=require('node:path');
(async()=>{const browser=await chromium.launchPersistentContext(process.argv[2],{executablePath:'/usr/bin/chromium',args:['--no-sandbox','--no-proxy-server']});
try {const page=browser.pages()[0]||await browser.newPage(); let sockets=0; page.on('websocket',()=>sockets++);
page.on('pageerror',e=>console.log('PAGE ERROR',e.message));
page.on('console',m=>{if(m.type()==='error') console.log('CONSOLE ERROR',m.text())});
page.on('requestfailed',r=>console.log('REQUEST FAILED',r.url(),r.failure()?.errorText));
page.on('response',r=>{if(r.status()>=400) console.log('HTTP ERROR',r.status(),r.url())});
await page.goto(process.argv[1]); const frame=page.frameLocator('iframe');
try {await frame.locator('.monaco-workbench').waitFor({timeout:25000});}
catch(e){console.log('PAGE TEXT',(await frame.locator('body').innerText()).slice(0,1500));throw e;}
await frame.getByText('hello.py',{exact:true}).first().dblclick({timeout:30000});
await frame.getByText("print('container workbench')",{exact:false}).first().waitFor({timeout:30000});
await frame.locator('body').evaluate(()=>localStorage.setItem('autonomics-origin-test','persisted'));
await page.waitForTimeout(3000); if (!sockets) throw new Error('no remote websocket opened');
const dir=process.argv[3]; const wait=async name=>{for(let i=0;i<500;i++){if(fs.existsSync(path.join(dir,name)))return;await new Promise(r=>setTimeout(r,100));}throw Error('timeout '+name)};
fs.writeFileSync(path.join(dir,'browser-ready'),'1'); await wait('open-issued');
const line=frame.getByText("print('bridge opened file')",{exact:false}).first(); await line.waitFor({timeout:30000});
await line.click(); await page.keyboard.press('End'); await page.keyboard.type('#dirty');
await frame.getByText('#dirty',{exact:false}).first().waitFor({timeout:10000});
fs.writeFileSync(path.join(dir,'browser-dirty'),'1'); await wait('save-issued');
await page.keyboard.press('Control+s'); fs.writeFileSync(path.join(dir,'browser-saved'),'1'); await wait('browser-done');
console.log('workbench, file and remote websocket ready');
} finally {await browser.close();}})().catch(e=>{console.error(e);process.exit(1)});"#;
            let gui = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/gui");
            let browser_profile = runtime_dir.path().join("browser-profile");
            let mut browser = tokio::process::Command::new("node")
                .current_dir(gui)
                .args(["-e", script, &parent_url])
                .arg(&browser_profile)
                .arg(runtime_dir.path())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            let wait_file = |name: &'static str| {
                let path = runtime_dir.path().join(name);
                async move {
                    timeout(Duration::from_secs(45), async {
                        while !path.exists() {
                            sleep(Duration::from_millis(100)).await;
                        }
                    })
                    .await
                }
            };
            if wait_file("browser-ready").await.is_err() {
                let _ = browser.kill().await;
                let output = browser.wait_with_output().await.unwrap();
                panic!(
                    "browser failed: {}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            let state_request = br#"{"action":"state","debug":true}"#;
            timeout(Duration::from_secs(20), async {
                loop {
                    if manager
                        .vscode_control(&config, "vscode-test", state_request)
                        .await
                        .is_ok()
                    {
                        break;
                    }
                    sleep(Duration::from_millis(200)).await;
                }
            })
            .await
            .expect("bridge activation");
            let opened = manager
                .vscode_control(
                    &config,
                    "vscode-test",
                    br#"{"action":"open","path":"second.py","line":1,"column":1}"#,
                )
                .await
                .unwrap_or_else(|_| panic!("open file in VS Code"));
            assert_eq!(opened["opened"], true);
            std::fs::write(runtime_dir.path().join("open-issued"), "1").unwrap();
            if wait_file("browser-dirty").await.is_err() {
                let _ = browser.kill().await;
                let output = browser.wait_with_output().await.unwrap();
                panic!(
                    "open/edit failed: {}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            let dirty_state = timeout(Duration::from_secs(10), async {
                loop {
                    if manager
                        .vscode_control(&config, "vscode-test", state_request)
                        .await
                        .is_ok_and(|value| value["dirty"] == true)
                    {
                        break;
                    }
                    sleep(Duration::from_millis(200)).await;
                }
            })
            .await;
            if dirty_state.is_err() {
                let last = manager
                    .vscode_control(&config, "vscode-test", state_request)
                    .await
                    .ok();
                let disk =
                    std::fs::read_to_string(workspace.path().join("second.py")).unwrap_or_default();
                panic!("dirty state not reported: {last:?}; disk={disk:?}");
            }
            std::fs::write(runtime_dir.path().join("save-issued"), "1").unwrap();
            wait_file("browser-saved").await.expect("save in VS Code");
            timeout(Duration::from_secs(10), async {
                loop {
                    if manager
                        .vscode_control(&config, "vscode-test", state_request)
                        .await
                        .is_ok_and(|value| value["dirty"] == false)
                    {
                        break;
                    }
                    sleep(Duration::from_millis(200)).await;
                }
            })
            .await
            .expect("clean state");
            std::fs::write(runtime_dir.path().join("browser-done"), "1").unwrap();
            let output = timeout(Duration::from_secs(15), browser.wait_with_output())
                .await
                .unwrap()
                .unwrap();
            assert!(
                output.status.success(),
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            parent_task.abort();
        }
        assert!(manager.is_connected(&config, "vscode-test").await);
        let marker = manager.exec(&config, "vscode-test", "mkdir -p /var/autonomics-vscode-data/extensions/test-extension && printf persisted > /var/autonomics-vscode-data/extensions/test-extension/marker", &[], &[]).await.unwrap_or_else(|_| panic!("write VS Code extension data"));
        assert!(marker.status.success());
        manager.close("vscode-test").await;
        assert!(!socket_dir.exists());
        let previous = manager
            .connect(&config, "vscode-test", &image, workspace.path())
            .await
            .unwrap_or_else(|_| panic!("reopen VS Code container"));
        let reopened_url = {
            let slot = manager
                .active("vscode-test")
                .await
                .unwrap_or_else(|_| panic!("active reopened project"));
            slot.as_ref()
                .unwrap()
                .vscode
                .as_ref()
                .unwrap()
                .listener
                .url
                .clone()
        };
        assert_eq!(
            workbench_url.split('/').take(3).collect::<Vec<_>>(),
            reopened_url.split('/').take(3).collect::<Vec<_>>()
        );
        assert_ne!(workbench_url, reopened_url);
        if std::env::var_os("AUTONOMICS_DEVCONTAINER_TEST_BROWSER").is_some() {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let parent_url = format!("http://{}/", listener.local_addr().unwrap());
            let html = format!(
                r#"<iframe src="{reopened_url}" sandbox="allow-scripts allow-same-origin allow-forms allow-modals allow-popups allow-downloads allow-pointer-lock" style="width:1400px;height:900px"></iframe>"#
            );
            let app = axum::Router::new().route(
                "/",
                axum::routing::get(move || {
                    let html = html.clone();
                    async move { axum::response::Html(html) }
                }),
            );
            let parent_task =
                tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let script = r#"const {chromium}=require('playwright');
(async()=>{const ctx=await chromium.launchPersistentContext(process.argv[2],{executablePath:'/usr/bin/chromium',args:['--no-sandbox','--no-proxy-server']});
try {const page=ctx.pages()[0]||await ctx.newPage(); await page.goto(process.argv[1],{waitUntil:'domcontentloaded'});
const frame=page.frameLocator('iframe'); await frame.locator('.monaco-workbench').waitFor({timeout:25000});
if(await frame.locator('body').evaluate(()=>localStorage.getItem('autonomics-origin-test'))!=='persisted') throw new Error('browser storage not restored');
}finally{await ctx.close();}})().catch(e=>{console.error(e);process.exit(1)});"#;
            let output = timeout(
                Duration::from_secs(45),
                tokio::process::Command::new("node")
                    .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/gui"))
                    .args(["-e", script, &parent_url])
                    .arg(runtime_dir.path().join("browser-profile"))
                    .output(),
            )
            .await
            .unwrap()
            .unwrap();
            assert!(
                output.status.success(),
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            parent_task.abort();
        }
        let marker = manager
            .exec(
                &config,
                "vscode-test",
                "cat /var/autonomics-vscode-data/extensions/test-extension/marker",
                &[],
                &[],
            )
            .await
            .unwrap_or_else(|_| panic!("read persisted data"));
        assert!(marker.status.success());
        assert_eq!(marker.stdout, b"persisted");
        let rebuilt = manager
            .connect_mode(&config, "vscode-test", &image, workspace.path(), true)
            .await
            .unwrap_or_else(|_| panic!("rebuild VS Code container"));
        assert_ne!(previous, rebuilt);
        let marker = manager
            .exec(
                &config,
                "vscode-test",
                "cat /var/autonomics-vscode-data/extensions/test-extension/marker",
                &[],
                &[],
            )
            .await
            .unwrap_or_else(|_| panic!("read data after rebuild"));
        assert!(marker.status.success());
        assert_eq!(marker.stdout, b"persisted");
        manager
            .connect(&config, "vscode-other", &image, workspace.path())
            .await
            .unwrap_or_else(|_| panic!("open second project"));
        let isolated = manager
            .exec(
                &config,
                "vscode-other",
                "test ! -e /var/autonomics-vscode-data/extensions/test-extension/marker",
                &[],
                &[],
            )
            .await
            .unwrap_or_else(|_| panic!("check project isolation"));
        assert!(isolated.status.success());
        manager.close("vscode-other").await;
        manager.close("vscode-test").await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn temporary_health_probe_failure_does_not_destroy_session() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("fail");
        std::fs::write(&marker, "").unwrap();
        let program = dir.path().join("podman-mock");
        std::fs::write(
            &program,
            format!("#!/bin/sh\n[ ! -e '{}' ]\n", marker.display()),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut config = PodmanConfig::default();
        config.program = program.to_string_lossy().into_owned();
        config.storage_root = None;
        config.run_root = None;
        config.storage_driver = None;
        let child = tokio::process::Command::new("/bin/sh")
            .args(["-c", "cat >/dev/null"])
            .stdin(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let manager = DevContainers::default();
        let slot = manager.slot("project", true).await.unwrap();
        *slot.lock().await = Some(Session {
            name: "container".into(),
            settings: super::super::devcontainer_config::DevConfig::load(dir.path(), "image")
                .unwrap_or_else(|_| panic!("default config")),
            workspace: dir.path().to_path_buf(),
            remote_env: BTreeMap::new(),
            child,
            terminal: None,
            language_servers: HashMap::new(),
            vscode: None,
        });
        assert!(!manager.is_connected(&config, "project").await);
        assert!(manager.active("project").await.is_ok());
        assert!(
            manager
                .connect(&config, "project", "image", dir.path())
                .await
                .is_err()
        );
        assert!(manager.active("project").await.is_ok());
        std::fs::remove_file(&marker).unwrap();
        assert!(manager.is_connected(&config, "project").await);
        assert!(
            matches!(manager.connect(&config, "project", "image", dir.path()).await, Ok(name) if name == "container")
        );
        manager.close("project").await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn slow_language_start_does_not_block_session_or_survive_close() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("probe-started");
        let program = dir.path().join("podman-mock");
        std::fs::write(&program, format!("#!/bin/sh\ncase \"$*\" in *--interactive*) exec cat;; *) touch '{}'; sleep 2; exit 0;; esac\n", marker.display())).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut config = PodmanConfig::default();
        config.program = program.to_string_lossy().into_owned();
        config.storage_root = None;
        config.run_root = None;
        config.storage_driver = None;
        let child = tokio::process::Command::new("/bin/sh")
            .args(["-c", "cat >/dev/null"])
            .stdin(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let manager = Arc::new(DevContainers::default());
        let slot = manager.slot("project", true).await.unwrap();
        *slot.lock().await = Some(Session {
            name: "container".into(),
            settings: super::super::devcontainer_config::DevConfig::load(dir.path(), "image")
                .unwrap_or_else(|_| panic!("default config")),
            workspace: dir.path().to_path_buf(),
            remote_env: BTreeMap::new(),
            child,
            terminal: None,
            language_servers: HashMap::new(),
            vscode: None,
        });
        let task = {
            let manager = manager.clone();
            tokio::spawn(async move { manager.open_language(&config, "project", "python").await })
        };
        timeout(Duration::from_secs(5), async {
            while !marker.exists() {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("probe started");
        let guard = timeout(Duration::from_millis(500), manager.active("project"))
            .await
            .expect("session remained responsive")
            .unwrap_or_else(|_| panic!("session missing"));
        drop(guard);
        timeout(Duration::from_secs(1), manager.close("project"))
            .await
            .expect("close remained responsive");
        assert!(matches!(task.await, Ok(Err(_))));
        assert!(manager.active("project").await.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn slow_snapshot_does_not_block_session() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("scan-started");
        let program = dir.path().join("podman-mock");
        std::fs::write(
            &program,
            format!("#!/bin/sh\ntouch '{}'; sleep 2\n", marker.display()),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut config = PodmanConfig::default();
        config.program = program.to_string_lossy().into_owned();
        config.storage_root = None;
        config.run_root = None;
        config.storage_driver = None;
        let child = tokio::process::Command::new("/bin/sh")
            .args(["-c", "cat >/dev/null"])
            .stdin(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let manager = Arc::new(DevContainers::default());
        let slot = manager.slot("project", true).await.unwrap();
        *slot.lock().await = Some(Session {
            name: "container".into(),
            settings: super::super::devcontainer_config::DevConfig::load(dir.path(), "image")
                .unwrap_or_else(|_| panic!("default config")),
            workspace: dir.path().to_path_buf(),
            remote_env: BTreeMap::new(),
            child,
            terminal: None,
            language_servers: HashMap::new(),
            vscode: None,
        });
        let task = {
            let manager = manager.clone();
            tokio::spawn(async move { manager.snapshot(&config, "project").await })
        };
        timeout(Duration::from_secs(5), async {
            while !marker.exists() {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("snapshot started");
        let guard = timeout(Duration::from_millis(500), manager.active("project"))
            .await
            .expect("snapshot must not block session")
            .unwrap_or_else(|_| panic!("session missing"));
        drop(guard);
        timeout(Duration::from_secs(1), manager.close("project"))
            .await
            .expect("close remained responsive");
        assert!(matches!(task.await, Ok(Ok(_))));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn oversized_snapshot_is_rejected() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("podman-mock");
        std::fs::write(&program, "#!/bin/sh\nhead -c 8388609 /dev/zero\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut config = PodmanConfig::default();
        config.program = program.to_string_lossy().into_owned();
        config.storage_root = None;
        config.run_root = None;
        config.storage_driver = None;
        let child = tokio::process::Command::new("/bin/sh")
            .args(["-c", "cat >/dev/null"])
            .stdin(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let manager = DevContainers::default();
        let slot = manager.slot("project", true).await.unwrap();
        *slot.lock().await = Some(Session {
            name: "container".into(),
            settings: super::super::devcontainer_config::DevConfig::load(dir.path(), "image")
                .unwrap_or_else(|_| panic!("default config")),
            workspace: dir.path().to_path_buf(),
            remote_env: BTreeMap::new(),
            child,
            terminal: None,
            language_servers: HashMap::new(),
            vscode: None,
        });
        assert!(manager.snapshot(&config, "project").await.is_err());
        manager.close("project").await;
    }

    #[tokio::test]
    #[ignore = "requires AUTONOMICS_DEVCONTAINER_TEST_STATE and AUTONOMICS_DEVCONTAINER_TEST_ALPINE_IMAGE"]
    async fn real_container_snapshot_busybox() {
        let state =
            std::env::var("AUTONOMICS_DEVCONTAINER_TEST_STATE").expect("test state directory");
        let image =
            std::env::var("AUTONOMICS_DEVCONTAINER_TEST_ALPINE_IMAGE").expect("local Alpine image");
        let config =
            container_runtime::ContainerExecutionInfra::for_operator_state_dir(Path::new(&state))
                .expect("container configuration")
                .config;
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("a\nfile"), "data").unwrap();
        let manager = DevContainers::default();
        manager
            .connect(&config, "alpine-snapshot", &image, workspace.path())
            .await
            .unwrap_or_else(|_| panic!("Alpine container"));
        let output = manager
            .snapshot(&config, "alpine-snapshot")
            .await
            .unwrap_or_else(|_| panic!("Alpine snapshot"));
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let entries = super::super::plugins::parse_project_snapshot(&output.stdout)
            .unwrap_or_else(|_| panic!("parse Alpine snapshot"));
        assert!(
            entries
                .iter()
                .any(|entry| entry["path"] == "a\nfile" && entry["kind"] == "file")
        );
        manager.close("alpine-snapshot").await;
    }

    #[tokio::test]
    #[ignore = "requires AUTONOMICS_DEVCONTAINER_TEST_STATE and AUTONOMICS_DEVCONTAINER_TEST_IMAGE"]
    async fn real_container_uses_configured_remote_user() {
        let state =
            std::env::var("AUTONOMICS_DEVCONTAINER_TEST_STATE").expect("test state directory");
        let image = std::env::var("AUTONOMICS_DEVCONTAINER_TEST_IMAGE").expect("local test image");
        let mut config =
            container_runtime::ContainerExecutionInfra::for_operator_state_dir(Path::new(&state))
                .expect("container configuration")
                .config;
        let runtime_dir = tempfile::tempdir().unwrap();
        config.workspace_root = runtime_dir.path().join("runtime");
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join(".devcontainer.json"), r#"{"containerUser":"root","remoteUser":"nobody","onCreateCommand":"id -u > /work/lifecycle-uid"}"#).unwrap();
        let manager = DevContainers::default();
        let name = manager
            .connect(&config, "user-project", &image, workspace.path())
            .await
            .unwrap_or_else(|_| panic!("container with remote user failed to start"));
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("lifecycle-uid"))
                .unwrap()
                .trim(),
            "65534"
        );
        let output = manager
            .exec(
                &config,
                "user-project",
                "id -u; printf writable > /work/user-file",
                &[],
                &[],
            )
            .await
            .unwrap_or_else(|_| panic!("remote user exec"));
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "65534");
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("user-file")).unwrap(),
            "writable"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(
                std::fs::metadata(workspace.path().join("user-file"))
                    .unwrap()
                    .uid(),
                std::fs::metadata(workspace.path()).unwrap().uid()
            );
        }
        let container_user = config
            .async_command()
            .args(["exec", &name, "/bin/sh", "-c", "id -u"])
            .output()
            .await
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&container_user.stdout).trim(), "0");
        let terminal = super::super::devterminal::Terminal::start(
            &config,
            &name,
            24,
            80,
            &BTreeMap::new(),
            Some("nobody"),
        )
        .expect("terminal");
        terminal
            .write("id -u > /work/terminal-uid\n".into())
            .unwrap();
        timeout(Duration::from_secs(10), async {
            while std::fs::read_to_string(workspace.path().join("terminal-uid"))
                .map_or(true, |value| value.trim() != "65534")
            {
                sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("terminal ran");
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("terminal-uid"))
                .unwrap()
                .trim(),
            "65534"
        );
        drop(terminal);
        let mut language = super::super::devlsp::LanguageServer::launch(
            &config,
            &name,
            "id -u > /work/language-uid; exec cat",
            &BTreeMap::new(),
            Some("nobody"),
        )
        .expect("language transport");
        language.send(json!({"id":1})).await.unwrap();
        timeout(Duration::from_secs(10), async {
            while std::fs::read_to_string(workspace.path().join("language-uid"))
                .map_or(true, |value| value.trim() != "65534")
            {
                sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("language ran");
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("language-uid"))
                .unwrap()
                .trim(),
            "65534"
        );
        drop(language);
        manager.close("user-project").await;
        std::fs::write(
            workspace.path().join(".devcontainer.json"),
            r#"{"containerUser":"nobody"}"#,
        )
        .unwrap();
        let name = manager
            .connect(&config, "user-project", &image, workspace.path())
            .await
            .unwrap_or_else(|_| panic!("container user failed to start"));
        let output = config
            .async_command()
            .args(["exec", &name, "/bin/sh", "-c", "id -u"])
            .output()
            .await
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "65534");
        manager.close("user-project").await;
        std::fs::write(
            workspace.path().join(".devcontainer.json"),
            r#"{"remoteUser":"nobody"}"#,
        )
        .unwrap();
        let name = manager
            .connect(&config, "user-project", &image, workspace.path())
            .await
            .unwrap_or_else(|_| panic!("image default container user"));
        let output = config
            .async_command()
            .args(["exec", &name, "/bin/sh", "-c", "id -u"])
            .output()
            .await
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "0");
        let output = manager
            .exec(&config, "user-project", "id -u", &[], &[])
            .await
            .unwrap_or_else(|_| panic!("remote user"));
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "65534");
        manager.close("user-project").await;
    }

    #[tokio::test]
    #[ignore = "requires AUTONOMICS_DEVCONTAINER_TEST_STATE and AUTONOMICS_DEVCONTAINER_TEST_IMAGE"]
    async fn real_container_builds_project_dockerfile() {
        let state =
            std::env::var("AUTONOMICS_DEVCONTAINER_TEST_STATE").expect("test state directory");
        let image = std::env::var("AUTONOMICS_DEVCONTAINER_TEST_IMAGE").expect("local test image");
        let config =
            container_runtime::ContainerExecutionInfra::for_operator_state_dir(Path::new(&state))
                .expect("container configuration")
                .config;
        let workspace = tempfile::tempdir().unwrap();
        let folder = workspace.path().join(".devcontainer");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(workspace.path().join("marker"), "from-context").unwrap();
        let dockerfile = folder.join("Dockerfile");
        std::fs::write(&dockerfile, format!("FROM {image} AS development\nARG MESSAGE\nENV BUILD_MESSAGE=$MESSAGE\nCOPY marker /build-marker\n")).unwrap();
        let config_path = folder.join("devcontainer.json");
        std::fs::write(&config_path, r#"{"build":{"dockerfile":"Dockerfile","context":"..","args":{"MESSAGE":"hello"},"target":"development"}}"#).unwrap();
        let manager = DevContainers::default();
        let name = manager
            .connect(&config, "build-project", &image, workspace.path())
            .await
            .unwrap_or_else(|_| panic!("built container failed to start"));
        let output = manager
            .exec(
                &config,
                "build-project",
                "printf '%s:' \"$BUILD_MESSAGE\"; cat /build-marker",
                &[],
                &[],
            )
            .await
            .unwrap_or_else(|_| panic!("built image contents"));
        assert_eq!(output.stdout, b"hello:from-context");
        std::fs::write(workspace.path().join("marker"), "updated-context").unwrap();
        let rebuilt = manager
            .connect_mode(&config, "build-project", &image, workspace.path(), true)
            .await
            .unwrap_or_else(|_| panic!("force rebuild"));
        assert_ne!(name, rebuilt);
        let output = manager
            .exec(&config, "build-project", "cat /build-marker", &[], &[])
            .await
            .unwrap_or_else(|_| panic!("rebuilt image contents"));
        assert_eq!(output.stdout, b"updated-context");
        std::fs::write(&dockerfile, "NOT_A_DOCKERFILE invalid\n").unwrap();
        std::fs::write(
            &config_path,
            r#"{"build":{"dockerfile":"Dockerfile","context":"..","args":{"MESSAGE":"changed"}}}"#,
        )
        .unwrap();
        assert!(
            manager
                .connect(&config, "build-project", &image, workspace.path())
                .await
                .is_err()
        );
        assert!(running(&config, &rebuilt).await);
        assert!(manager.active("build-project").await.is_ok());
        manager.close("build-project").await;
    }

    /// Opt-in integration test: run against an operator-selected local store
    /// and image, using an isolated temporary workspace and --rm container.
    #[tokio::test]
    #[ignore = "requires AUTONOMICS_DEVCONTAINER_TEST_STATE and AUTONOMICS_DEVCONTAINER_TEST_IMAGE"]
    async fn real_container_reads_and_writes_mounted_project() {
        let state =
            std::env::var("AUTONOMICS_DEVCONTAINER_TEST_STATE").expect("test state directory");
        let image = std::env::var("AUTONOMICS_DEVCONTAINER_TEST_IMAGE").expect("local test image");
        let config =
            container_runtime::ContainerExecutionInfra::for_operator_state_dir(Path::new(&state))
                .expect("container configuration")
                .config;
        let workspace = tempfile::tempdir().expect("temporary workspace");
        std::fs::write(workspace.path().join("input.txt"), "before").unwrap();
        std::fs::create_dir(workspace.path().join(".devcontainer")).unwrap();
        std::fs::write(
            workspace.path().join(".devcontainer/devcontainer.json"),
            r#"{
          // Commands and variables are evaluated inside the project container.
          "containerEnv": {"DEV_TEST": "container-${containerWorkspaceFolderBasename}"},
          "remoteEnv": {"REMOTE_TEST": "${containerEnv:DEV_TEST}-remote", "PATH": "${containerEnv:PATH}"},
          "onCreateCommand": "printf %s \"$REMOTE_TEST\" > /work/.remote-lifecycle; printf create\\\\n >> /work/.lifecycle",
          "updateContentCommand": ["/bin/sh", "-c", "printf update\\\\n >> /work/.lifecycle"],
          "postCreateCommand": "printf postcreate\\\\n >> /work/.lifecycle",
          "postStartCommand": "printf start\\\\n >> /work/.lifecycle",
          "postAttachCommand": "printf attach\\\\n >> /work/.lifecycle",
        }"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                workspace.path().join("input.txt"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
        let manager = Arc::new(DevContainers::default());
        let first = match manager
            .connect(&config, "test-project", &image, workspace.path())
            .await
        {
            Ok(name) => name,
            Err(_) => panic!("container did not start"),
        };
        std::fs::create_dir_all(workspace.path().join("node_modules/test-package")).unwrap();
        std::fs::write(
            workspace
                .path()
                .join("node_modules/test-package/ignored.js"),
            "ignored",
        )
        .unwrap();
        std::fs::write(workspace.path().join("name\nwith-newline.txt"), "value").unwrap();
        let snapshot = manager
            .snapshot(&config, "test-project")
            .await
            .unwrap_or_else(|_| panic!("container snapshot"));
        assert!(snapshot.status.success());
        let entries = super::super::plugins::parse_project_snapshot(&snapshot.stdout)
            .unwrap_or_else(|_| panic!("parse snapshot"));
        assert!(
            entries
                .iter()
                .any(|entry| entry["path"] == "input.txt" && entry["kind"] == "file")
        );
        assert!(
            entries
                .iter()
                .any(|entry| entry["path"] == "name\nwith-newline.txt")
        );
        assert!(!entries.iter().any(|entry| {
            entry["path"]
                .as_str()
                .is_some_and(|path| path.starts_with("node_modules"))
        }));
        assert!(super::super::plugins::parse_project_snapshot(b"f\0./bad\0partial").is_err());
        let second = match manager
            .connect(&config, "test-project", &image, workspace.path())
            .await
        {
            Ok(name) => name,
            Err(_) => panic!("container did not reconnect"),
        };
        assert_eq!(first, second);
        let env = manager
            .exec(&config, "test-project", "printf %s \"$DEV_TEST\"", &[], &[])
            .await
            .unwrap_or_else(|_| panic!("read container env"));
        assert_eq!(env.stdout, b"container-work");
        let remote = manager
            .exec(
                &config,
                "test-project",
                "printf %s \"$REMOTE_TEST\"",
                &[],
                &[],
            )
            .await
            .unwrap_or_else(|_| panic!("read remote env"));
        assert_eq!(remote.stdout, b"container-work-remote");
        assert_eq!(
            std::fs::read_to_string(workspace.path().join(".remote-lifecycle")).unwrap(),
            "container-work-remote"
        );
        let container_only = config
            .async_command()
            .args([
                "exec",
                &first,
                "/bin/sh",
                "-c",
                "printf %s \"${REMOTE_TEST-unset}\"",
            ])
            .output()
            .await
            .unwrap();
        assert_eq!(container_only.stdout, b"unset");
        let remote_env = manager
            .active("test-project")
            .await
            .unwrap_or_else(|_| panic!("active session"))
            .as_ref()
            .unwrap()
            .remote_env
            .clone();
        let lifecycle = std::fs::read_to_string(workspace.path().join(".lifecycle")).unwrap();
        assert_eq!(
            lifecycle,
            "create\nupdate\npostcreate\nstart\nattach\nattach\n"
        );
        let config_path = workspace.path().join(".devcontainer/devcontainer.json");
        let original_config = std::fs::read_to_string(&config_path).unwrap();
        std::fs::write(
            &config_path,
            original_config.replace(
                "${containerEnv:DEV_TEST}-remote",
                "${containerEnv:DEV_TEST}-updated",
            ),
        )
        .unwrap();
        let reconnected = manager
            .connect(&config, "test-project", &image, workspace.path())
            .await
            .unwrap_or_else(|_| panic!("reconnect after remoteEnv change"));
        assert_eq!(reconnected, first);
        let changed = manager
            .exec(
                &config,
                "test-project",
                "printf %s \"$REMOTE_TEST\"",
                &[],
                &[],
            )
            .await
            .unwrap_or_else(|_| panic!("read changed remote env"));
        assert_eq!(changed.stdout, b"container-work-updated");
        std::fs::write(&config_path, original_config).unwrap();
        let restored = manager
            .connect(&config, "test-project", &image, workspace.path())
            .await
            .unwrap_or_else(|_| panic!("restore remote env"));
        assert_eq!(restored, first);
        let other_workspace = tempfile::tempdir().expect("second workspace");
        if manager
            .connect(&config, "other-project", &image, other_workspace.path())
            .await
            .is_err()
        {
            panic!("second container did not start");
        }
        let slow_manager = manager.clone();
        let slow_config = config.clone();
        let slow = tokio::spawn(async move {
            slow_manager
                .exec(
                    &slow_config,
                    "test-project",
                    "touch /work/slow-started; sleep 5",
                    &[],
                    &[],
                )
                .await
        });
        timeout(Duration::from_secs(10), async {
            while !workspace.path().join("slow-started").exists() {
                sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("slow operation started");
        let other = timeout(
            Duration::from_secs(3),
            manager.exec(&config, "other-project", "printf responsive", &[], &[]),
        )
        .await
        .expect("another project must remain responsive");
        let other = match other {
            Ok(output) => output,
            Err(_) => panic!("second project operation failed"),
        };
        assert_eq!(other.stdout, b"responsive");
        assert!(matches!(slow.await, Ok(Ok(output)) if output.status.success()));
        manager.close("other-project").await;
        let output = match manager
            .exec(
                &config,
                "test-project",
                "cat /work/input.txt; printf after > /work/output.txt",
                &[],
                &[],
            )
            .await
        {
            Ok(output) => output,
            Err(_) => panic!("container exec failed"),
        };
        assert!(output.status.success());
        assert_eq!(output.stdout, b"before");
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("output.txt")).unwrap(),
            "after"
        );
        let output = match manager
            .exec(&config, "test-project", &read_script(), &["input.txt"], &[])
            .await
        {
            Ok(output) => output,
            Err(_) => panic!("read failed"),
        };
        assert_eq!(output.stdout, b"before");
        let output = match manager
            .exec(
                &config,
                "test-project",
                &save_script(),
                &["input.txt", "6"],
                b"beforechanged",
            )
            .await
        {
            Ok(output) => output,
            Err(_) => panic!("save failed"),
        };
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("input.txt")).unwrap(),
            "changed"
        );
        let output = match manager
            .exec(
                &config,
                "test-project",
                &create_directory_script(),
                &["empty-folder"],
                &[],
            )
            .await
        {
            Ok(output) => output,
            Err(_) => panic!("create directory failed"),
        };
        assert!(output.status.success());
        let output = match manager
            .exec(
                &config,
                "test-project",
                &create_directory_script(),
                &["empty-folder"],
                &[],
            )
            .await
        {
            Ok(output) => output,
            Err(_) => panic!("duplicate directory failed"),
        };
        assert_eq!(output.status.code(), Some(42));
        let output = match manager
            .exec(&config, "test-project", LIST_DIRECTORIES_SCRIPT, &[], &[])
            .await
        {
            Ok(output) => output,
            Err(_) => panic!("list directories failed"),
        };
        assert!(
            output
                .stdout
                .split(|byte| *byte == 0)
                .any(|path| path == b"./empty-folder")
        );
        let output = manager
            .exec(
                &config,
                "test-project",
                &create_directory_script(),
                &["folder"],
                &[],
            )
            .await
            .unwrap_or_else(|_| panic!("create folder failed"));
        assert!(output.status.success());
        let output = manager
            .exec(
                &config,
                "test-project",
                &create_script(),
                &["folder/child.txt"],
                &[],
            )
            .await
            .unwrap_or_else(|_| panic!("create nested file failed"));
        assert!(output.status.success());
        let output = manager
            .exec(
                &config,
                "test-project",
                &rename_directory_script(),
                &["folder", "empty-folder"],
                &[],
            )
            .await
            .unwrap_or_else(|_| panic!("folder conflict failed"));
        assert_eq!(output.status.code(), Some(42));
        assert!(workspace.path().join("folder/child.txt").exists());
        let output = manager
            .exec(
                &config,
                "test-project",
                &rename_directory_script(),
                &["folder", "moved-folder"],
                &[],
            )
            .await
            .unwrap_or_else(|_| panic!("folder rename failed"));
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(workspace.path().join("moved-folder/child.txt").exists());
        let output = manager
            .exec(
                &config,
                "test-project",
                &delete_directory_script(),
                &["moved-folder"],
                &[],
            )
            .await
            .unwrap_or_else(|_| panic!("folder delete failed"));
        assert!(output.status.success());
        assert!(!workspace.path().join("moved-folder").exists());
        let output = match manager
            .exec(&config, "test-project", &create_script(), &["new.txt"], &[])
            .await
        {
            Ok(output) => output,
            Err(_) => panic!("create failed"),
        };
        assert!(output.status.success());
        let output = match manager
            .exec(&config, "test-project", &create_script(), &["new.txt"], &[])
            .await
        {
            Ok(output) => output,
            Err(_) => panic!("duplicate create failed"),
        };
        assert_eq!(output.status.code(), Some(42));
        let output = match manager
            .exec(
                &config,
                "test-project",
                &rename_script(),
                &["new.txt", "input.txt"],
                &[],
            )
            .await
        {
            Ok(output) => output,
            Err(_) => panic!("rename conflict failed"),
        };
        assert_eq!(output.status.code(), Some(42));
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("input.txt")).unwrap(),
            "changed"
        );
        let output = match manager
            .exec(
                &config,
                "test-project",
                &rename_script(),
                &["new.txt", "renamed.txt"],
                &[],
            )
            .await
        {
            Ok(output) => output,
            Err(_) => panic!("rename failed"),
        };
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!workspace.path().join("new.txt").exists());
        assert!(workspace.path().join("renamed.txt").exists());
        let output = match manager
            .exec(
                &config,
                "test-project",
                &delete_script(),
                &["renamed.txt"],
                &[],
            )
            .await
        {
            Ok(output) => output,
            Err(_) => panic!("delete failed"),
        };
        assert!(output.status.success());
        assert!(!workspace.path().join("renamed.txt").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(workspace.path().join("input.txt"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o755
            );
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("input.txt", workspace.path().join("link.txt")).unwrap();
            let output = match manager
                .exec(&config, "test-project", &read_script(), &["link.txt"], &[])
                .await
            {
                Ok(output) => output,
                Err(_) => panic!("symlink check failed"),
            };
            assert!(!output.status.success());
            let listing = match manager
                .exec(&config, "test-project", LIST_SCRIPT, &[], &[])
                .await
            {
                Ok(output) => output,
                Err(_) => panic!("list files"),
            };
            assert!(listing.status.success());
            assert!(
                !listing
                    .stdout
                    .split(|byte| *byte == 0)
                    .any(|file| file == b"./link.txt")
            );
            std::os::unix::fs::symlink("/tmp", workspace.path().join("linkdir")).unwrap();
            let output = match manager
                .exec(
                    &config,
                    "test-project",
                    &create_script(),
                    &["linkdir/escaped"],
                    &[],
                )
                .await
            {
                Ok(output) => output,
                Err(_) => panic!("symlink create check failed"),
            };
            assert!(!output.status.success());
            let output = match manager
                .exec(
                    &config,
                    "test-project",
                    &rename_script(),
                    &["input.txt", "linkdir/escaped"],
                    &[],
                )
                .await
            {
                Ok(output) => output,
                Err(_) => panic!("symlink rename check failed"),
            };
            assert!(!output.status.success());
            assert_eq!(
                std::fs::read_to_string(workspace.path().join("input.txt")).unwrap(),
                "changed"
            );
        }
        let output = match manager
            .exec(
                &config,
                "test-project",
                &save_script(),
                &["input.txt", "6"],
                b"beforeoverwritten",
            )
            .await
        {
            Ok(output) => output,
            Err(_) => panic!("conflict check failed"),
        };
        assert_eq!(output.status.code(), Some(42));
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("input.txt")).unwrap(),
            "changed"
        );
        let mut terminal =
            super::super::devterminal::Terminal::start(&config, &first, 24, 80, &remote_env, None)
                .expect("start terminal");
        terminal.resize(32, 100).expect("resize terminal");
        terminal
            .write("printf %s \"$REMOTE_TEST\" > /work/terminal-proof\n".into())
            .expect("terminal input");
        timeout(Duration::from_secs(10), async {
            loop {
                if workspace.path().join("terminal-proof").exists() {
                    break;
                }
                sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("terminal command executed");
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("terminal-proof")).unwrap(),
            "container-work-remote"
        );
        assert!(!terminal.exited().expect("terminal status"));
        drop(terminal);
        let mut language = super::super::devlsp::LanguageServer::launch(
            &config,
            &first,
            "printf %s \"$REMOTE_TEST\" > /work/lsp-env; exec cat",
            &remote_env,
            None,
        )
        .expect("language transport");
        let message = json!({"jsonrpc":"2.0", "id":7, "method":"initialize", "params":{"rootUri":"file:///work"}});
        language
            .send(message.clone())
            .await
            .expect("language input");
        let response = timeout(Duration::from_secs(10), async {
            loop {
                let messages = language.poll(0).unwrap().0;
                if !messages.is_empty() {
                    break messages;
                }
                sleep(Duration::from_millis(30)).await;
            }
        })
        .await
        .expect("language output");
        assert_eq!(response, vec![message]);
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("lsp-env")).unwrap(),
            "container-work-remote"
        );
        drop(language);
        if std::env::var_os("AUTONOMICS_DEVCONTAINER_TEST_INSTALL_TS").is_some() {
            let node = std::env::var("AUTONOMICS_DEVCONTAINER_TEST_NODE_BIN")
                .expect("host Node binary for isolated test");
            let bin = workspace.path().join(".test-bin");
            std::fs::create_dir(&bin).unwrap();
            std::fs::copy(node, bin.join("node")).unwrap();
            let install = timeout(
                Duration::from_secs(180),
                tokio::process::Command::new("npm")
                    .args([
                        "install",
                        "--no-save",
                        "--ignore-scripts",
                        "--no-audit",
                        "--no-fund",
                        "--prefix",
                    ])
                    .arg(workspace.path())
                    .arg("--cache")
                    .arg(workspace.path().join(".npm-cache"))
                    .args(["typescript@5.9.3", "typescript-language-server@4.4.1"])
                    .stdin(Stdio::null())
                    .kill_on_drop(true)
                    .output(),
            )
            .await
            .expect("npm install timeout")
            .expect("install TypeScript test dependencies");
            assert!(
                install.status.success(),
                "{}",
                String::from_utf8_lossy(&install.stderr)
            );
            let mut ts_env = remote_env.clone();
            let path = ts_env
                .get("PATH")
                .cloned()
                .unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into());
            ts_env.insert("PATH".into(), format!("/work/.test-bin:{path}"));
            let mut server = super::super::devlsp::LanguageServer::start(
                &config,
                &first,
                "typescript",
                &ts_env,
                None,
            )
            .await
            .expect("start TypeScript language server in container");
            server.send(json!({"jsonrpc":"2.0","id":10,"method":"initialize","params":{"processId":null,"rootUri":"file:///work","capabilities":{}}})).await.unwrap();
            let mut cursor = 0;
            let initialized = timeout(Duration::from_secs(30), async {
                loop {
                    let (messages, next, _, _) = server.poll(cursor).unwrap();
                    cursor = next;
                    if let Some(message) = messages.into_iter().find(|message| message["id"] == 10)
                    {
                        break message;
                    }
                    sleep(Duration::from_millis(50)).await;
                }
            })
            .await
            .expect("TypeScript initialization response");
            assert!(
                initialized["result"]["capabilities"]["completionProvider"].is_object(),
                "{initialized}"
            );
            server
                .send(json!({"jsonrpc":"2.0","method":"initialized","params":{}}))
                .await
                .unwrap();
            server.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///work/example.ts","languageId":"typescript","version":1,"text":"const greeting: string = 'hello';\ngree"}}})).await.unwrap();
            server.send(json!({"jsonrpc":"2.0","id":11,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///work/example.ts"},"position":{"line":1,"character":4}}})).await.unwrap();
            let completion = timeout(Duration::from_secs(30), async {
                loop {
                    let (messages, next, _, _) = server.poll(cursor).unwrap();
                    cursor = next;
                    if let Some(message) = messages.into_iter().find(|message| message["id"] == 11)
                    {
                        break message;
                    }
                    sleep(Duration::from_millis(50)).await;
                }
            })
            .await
            .expect("TypeScript completion response");
            let items = completion["result"]["items"]
                .as_array()
                .or_else(|| completion["result"].as_array())
                .expect("TypeScript completion items");
            assert!(
                items.iter().any(|item| item["label"] == "greeting"),
                "{completion}"
            );
            drop(server);
            let mut js_server = super::super::devlsp::LanguageServer::start(
                &config,
                &first,
                "javascript",
                &ts_env,
                None,
            )
            .await
            .expect("start JavaScript language server in container");
            js_server.send(json!({"jsonrpc":"2.0","id":12,"method":"initialize","params":{"processId":null,"rootUri":"file:///work","capabilities":{}}})).await.unwrap();
            let initialized = timeout(Duration::from_secs(30), async {
                loop {
                    let (messages, _, _, _) = js_server.poll(0).unwrap();
                    if let Some(message) = messages.into_iter().find(|message| message["id"] == 12)
                    {
                        break message;
                    }
                    sleep(Duration::from_millis(50)).await;
                }
            })
            .await
            .expect("JavaScript initialization response");
            assert!(
                initialized["result"]["capabilities"]["completionProvider"].is_object(),
                "{initialized}"
            );
            drop(js_server);
        }
        if std::env::var_os("AUTONOMICS_DEVCONTAINER_TEST_INSTALL_CLANGD").is_some() {
            let install = timeout(Duration::from_secs(300), config.async_command().args(["exec", "--user=0", "--workdir=/", &first, "/bin/sh", "-c", "mkdir -p /tmp/apt-archives/partial && apt-get -o APT::Sandbox::User=root update -qq && DEBIAN_FRONTEND=noninteractive apt-get -o APT::Sandbox::User=root -o Dir::Cache::archives=/tmp/apt-archives install -y -qq --no-install-recommends clangd"]).stdin(Stdio::null()).kill_on_drop(true).output()).await.expect("clangd install timeout").expect("install clangd in test container");
            assert!(
                install.status.success(),
                "{}",
                String::from_utf8_lossy(&install.stderr)
            );
            for (index, language) in ["c", "cpp"].iter().enumerate() {
                let mut server = super::super::devlsp::LanguageServer::start(
                    &config,
                    &first,
                    language,
                    &remote_env,
                    None,
                )
                .await
                .expect("start clangd in container");
                let init_id = 30 + index * 2;
                server.send(json!({"jsonrpc":"2.0","id":init_id,"method":"initialize","params":{"processId":null,"rootUri":"file:///work","capabilities":{}}})).await.unwrap();
                let mut cursor = 0;
                let initialized = timeout(Duration::from_secs(30), async {
                    loop {
                        let (messages, next, _, _) = server.poll(cursor).unwrap();
                        cursor = next;
                        if let Some(message) = messages
                            .into_iter()
                            .find(|message| message["id"] == init_id)
                        {
                            break message;
                        }
                        sleep(Duration::from_millis(50)).await;
                    }
                })
                .await
                .expect("clangd initialize");
                assert!(
                    initialized["result"]["capabilities"]["completionProvider"].is_object(),
                    "{initialized}"
                );
                server
                    .send(json!({"jsonrpc":"2.0","method":"initialized","params":{}}))
                    .await
                    .unwrap();
                let uri = format!(
                    "file:///work/example.{}",
                    if *language == "c" { "c" } else { "cpp" }
                );
                server.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":language,"version":1,"text":"int greeting = 1;\nint x = greet"}}})).await.unwrap();
                let completion_id = init_id + 1;
                server.send(json!({"jsonrpc":"2.0","id":completion_id,"method":"textDocument/completion","params":{"textDocument":{"uri":uri},"position":{"line":1,"character":13}}})).await.unwrap();
                let completion = timeout(Duration::from_secs(30), async {
                    loop {
                        let (messages, next, _, _) = server.poll(cursor).unwrap();
                        cursor = next;
                        if let Some(message) = messages
                            .into_iter()
                            .find(|message| message["id"] == completion_id)
                        {
                            break message;
                        }
                        sleep(Duration::from_millis(50)).await;
                    }
                })
                .await
                .expect("clangd completion");
                let items = completion["result"]["items"]
                    .as_array()
                    .or_else(|| completion["result"].as_array())
                    .expect("clangd completion items");
                assert!(
                    items.iter().any(|item| item["label"]
                        .as_str()
                        .is_some_and(|label| label.trim_start().starts_with("greeting"))),
                    "{completion}"
                );
                drop(server);
            }
        }
        if std::env::var_os("AUTONOMICS_DEVCONTAINER_TEST_INSTALL_GO").is_some() {
            let gopls = std::env::var("AUTONOMICS_DEVCONTAINER_TEST_GOPLS_BIN")
                .expect("gopls binary for isolated test");
            let goroot = std::env::var("AUTONOMICS_DEVCONTAINER_TEST_GO_ROOT")
                .expect("Go toolchain for isolated test");
            let copied = timeout(
                Duration::from_secs(180),
                tokio::process::Command::new("cp")
                    .arg("-R")
                    .arg(goroot)
                    .arg(workspace.path().join(".test-go"))
                    .stdin(Stdio::null())
                    .kill_on_drop(true)
                    .output(),
            )
            .await
            .expect("Go copy timeout")
            .expect("copy Go toolchain");
            assert!(
                copied.status.success(),
                "{}",
                String::from_utf8_lossy(&copied.stderr)
            );
            let tools = workspace.path().join(".test-tools");
            std::fs::create_dir(&tools).unwrap();
            std::fs::copy(gopls, tools.join("gopls")).unwrap();
            std::fs::write(
                workspace.path().join("go.mod"),
                "module example.com/container-test\n\ngo 1.25\n",
            )
            .unwrap();
            let source = "package main\nvar greeting = 1\nfunc main() { gree }\n";
            std::fs::write(workspace.path().join("example.go"), source).unwrap();
            let mut go_env = remote_env.clone();
            let path = go_env
                .get("PATH")
                .cloned()
                .unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into());
            go_env.insert(
                "PATH".into(),
                format!("/work/.test-tools:/work/.test-go/bin:{path}"),
            );
            go_env.insert("GOROOT".into(), "/work/.test-go".into());
            go_env.insert("GOCACHE".into(), "/tmp/autonomics-gocache".into());
            go_env.insert("GOMODCACHE".into(), "/tmp/autonomics-gomod".into());
            go_env.insert("GOPROXY".into(), "off".into());
            let mut server =
                super::super::devlsp::LanguageServer::start(&config, &first, "go", &go_env, None)
                    .await
                    .expect("start gopls in container");
            server.send(json!({"jsonrpc":"2.0","id":40,"method":"initialize","params":{"processId":null,"rootUri":"file:///work","capabilities":{}}})).await.unwrap();
            let mut cursor = 0;
            let initialized = timeout(Duration::from_secs(30), async {
                loop {
                    let (messages, next, _, _) = server.poll(cursor).unwrap();
                    cursor = next;
                    if let Some(message) = messages.into_iter().find(|message| message["id"] == 40)
                    {
                        break message;
                    }
                    sleep(Duration::from_millis(50)).await;
                }
            })
            .await
            .expect("gopls initialize");
            assert!(
                initialized["result"]["capabilities"]["completionProvider"].is_object(),
                "{initialized}"
            );
            server
                .send(json!({"jsonrpc":"2.0","method":"initialized","params":{}}))
                .await
                .unwrap();
            server.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///work/example.go","languageId":"go","version":1,"text":source}}})).await.unwrap();
            server.send(json!({"jsonrpc":"2.0","id":41,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///work/example.go"},"position":{"line":2,"character":18}}})).await.unwrap();
            let completion = timeout(Duration::from_secs(60), async {
                loop {
                    let (messages, next, _, _) = server.poll(cursor).unwrap();
                    cursor = next;
                    if let Some(message) = messages.into_iter().find(|message| message["id"] == 41)
                    {
                        break message;
                    }
                    sleep(Duration::from_millis(50)).await;
                }
            })
            .await
            .expect("gopls completion");
            let items = completion["result"]["items"]
                .as_array()
                .or_else(|| completion["result"].as_array())
                .expect("gopls completion items");
            assert!(
                items.iter().any(|item| item["label"] == "greeting"),
                "{completion}"
            );
            drop(server);
        }
        if std::env::var_os("AUTONOMICS_DEVCONTAINER_TEST_INSTALL_LSP").is_some() {
            let install = timeout(Duration::from_secs(180), config.async_command().args(["exec", "--workdir=/work", &first, "/bin/sh", "-c", "python3 -m pip install --disable-pip-version-check --no-cache-dir --target /tmp/lspdeps python-lsp-server==1.12.2 >/dev/null"]).stdin(Stdio::null()).output()).await.expect("install timeout").expect("install test language server");
            assert!(
                install.status.success(),
                "{}",
                String::from_utf8_lossy(&install.stderr)
            );
            let shim = manager.exec(&config, "test-project", "mkdir -p /work/.venv/bin; printf '#!/bin/sh\\nPYTHONPATH=/tmp/lspdeps exec python3 -m pylsp\\n' > /work/.venv/bin/pylsp; chmod +x /work/.venv/bin/pylsp", &[], &[]).await;
            assert!(
                matches!(shim, Ok(output) if output.status.success()),
                "create test virtual environment shim"
            );
            let listing = match manager
                .exec(&config, "test-project", LIST_SCRIPT, &[], &[])
                .await
            {
                Ok(output) => output,
                Err(_) => panic!("list files"),
            };
            assert!(
                !listing
                    .stdout
                    .split(|byte| *byte == 0)
                    .any(|file| file.starts_with(b"./.venv/"))
            );
            let mut server = super::super::devlsp::LanguageServer::start(
                &config,
                &first,
                "python",
                &remote_env,
                None,
            )
            .await
            .expect("start Python language server");
            server.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":null,"rootUri":"file:///work","capabilities":{}}})).await.unwrap();
            let mut cursor = 0;
            let initialized = timeout(Duration::from_secs(20), async {
                loop {
                    let (messages, next, _, _) = server.poll(cursor).unwrap();
                    cursor = next;
                    if let Some(message) = messages.into_iter().find(|message| message["id"] == 1) {
                        break message;
                    }
                    sleep(Duration::from_millis(50)).await;
                }
            })
            .await
            .expect("Python initialization response");
            assert!(
                initialized["result"]["capabilities"].is_object(),
                "{initialized}"
            );
            server
                .send(json!({"jsonrpc":"2.0","method":"initialized","params":{}}))
                .await
                .unwrap();
            server.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///work/test.py","languageId":"python","version":1,"text":"pr"}}})).await.unwrap();
            server.send(json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":"file:///work/test.py","version":2},"contentChanges":[{"text":"pri"}]}})).await.unwrap();
            server.send(json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///work/test.py"},"position":{"line":0,"character":3}}})).await.unwrap();
            let completion = timeout(Duration::from_secs(20), async {
                loop {
                    let (messages, next, _, _) = server.poll(cursor).unwrap();
                    cursor = next;
                    if let Some(message) = messages.into_iter().find(|message| message["id"] == 2) {
                        break message;
                    }
                    sleep(Duration::from_millis(50)).await;
                }
            })
            .await
            .expect("Python completion response");
            let items = completion["result"]["items"]
                .as_array()
                .or_else(|| completion["result"].as_array())
                .expect("completion items");
            assert!(
                items.iter().any(|item| item["insertText"] == "print"
                    || item["label"]
                        .as_str()
                        .is_some_and(|label| label.starts_with("print"))),
                "{completion}"
            );
            let created = manager
                .exec(
                    &config,
                    "test-project",
                    "printf 'value = 1\\nprint(value)\\n' > /work/test.py",
                    &[],
                    &[],
                )
                .await;
            assert!(matches!(created, Ok(output) if output.status.success()));
            server.send(json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":"file:///work/test.py","version":3},"contentChanges":[{"text":"value = 1\nprint(value)\n"}]}})).await.unwrap();
            server.send(json!({"jsonrpc":"2.0","id":3,"method":"textDocument/rename","params":{"textDocument":{"uri":"file:///work/test.py"},"position":{"line":0,"character":1},"newName":"renamed"}})).await.unwrap();
            let rename = timeout(Duration::from_secs(20), async {
                loop {
                    let (messages, next, _, _) = server.poll(cursor).unwrap();
                    cursor = next;
                    if let Some(message) = messages.into_iter().find(|message| message["id"] == 3) {
                        break message;
                    }
                    sleep(Duration::from_millis(50)).await;
                }
            })
            .await
            .expect("Python rename response");
            let edits = rename["result"]["documentChanges"][0]["edits"]
                .as_array()
                .expect("rename edits");
            assert!(
                edits
                    .iter()
                    .filter_map(|edit| edit["newText"].as_str())
                    .any(|text| text.matches("renamed").count() >= 2),
                "{rename}"
            );
            drop(server);
        }
        if std::env::var_os("AUTONOMICS_DEVCONTAINER_TEST_INSTALL_DEBUGPY").is_some() {
            let install = timeout(Duration::from_secs(180), config.async_command().args(["exec", "--workdir=/work", &first, "/bin/sh", "-c", "python3 -m venv /work/.venv && /work/.venv/bin/python -m pip install --disable-pip-version-check --no-cache-dir debugpy==1.8.21 >/dev/null && printf 'x = 1\\nx += 1\\nprint(x)\\n' > /work/debug.py"]).stdin(Stdio::null()).output()).await.expect("debugpy install timeout").expect("install debugpy");
            assert!(
                install.status.success(),
                "{}",
                String::from_utf8_lossy(&install.stderr)
            );
            let mut adapter = super::super::devlsp::LanguageServer::start_debug(
                &config,
                &first,
                "python",
                &remote_env,
                None,
            )
            .await
            .expect("start debug adapter");
            let mut offset = 0;
            let mut pending = Vec::new();
            adapter.send(json!({"seq":1,"type":"request","command":"initialize","arguments":{"clientID":"autonomics","adapterID":"python","pathFormat":"path","linesStartAt1":true,"columnsStartAt1":true,"supportsRunInTerminalRequest":false}})).await.unwrap();
            let initialized = debug_message(&mut adapter, &mut offset, &mut pending, |m| {
                m["type"] == "response" && m["request_seq"] == 1
            })
            .await;
            assert_eq!(initialized["success"], true, "{initialized}");
            adapter.send(json!({"seq":2,"type":"request","command":"launch","arguments":{"name":"test","type":"python","request":"launch","program":"/work/debug.py","cwd":"/work","console":"internalConsole","justMyCode":true}})).await.unwrap();
            debug_message(&mut adapter, &mut offset, &mut pending, |m| {
                m["type"] == "event" && m["event"] == "initialized"
            })
            .await;
            adapter.send(json!({"seq":3,"type":"request","command":"setBreakpoints","arguments":{"source":{"path":"/work/debug.py"},"breakpoints":[{"line":2}]}})).await.unwrap();
            let breakpoints = debug_message(&mut adapter, &mut offset, &mut pending, |m| {
                m["type"] == "response" && m["request_seq"] == 3
            })
            .await;
            assert_eq!(
                breakpoints["body"]["breakpoints"][0]["verified"], true,
                "{breakpoints}"
            );
            adapter
                .send(
                    json!({"seq":4,"type":"request","command":"configurationDone","arguments":{}}),
                )
                .await
                .unwrap();
            let stopped = debug_message(&mut adapter, &mut offset, &mut pending, |m| {
                m["type"] == "event" && m["event"] == "stopped"
            })
            .await;
            assert_eq!(stopped["body"]["reason"], "breakpoint");
            let thread = stopped["body"]["threadId"]
                .as_i64()
                .expect("stopped thread");
            adapter.send(json!({"seq":5,"type":"request","command":"stackTrace","arguments":{"threadId":thread}})).await.unwrap();
            let stack = debug_message(&mut adapter, &mut offset, &mut pending, |m| {
                m["type"] == "response" && m["request_seq"] == 5
            })
            .await;
            assert_eq!(stack["body"]["stackFrames"][0]["line"], 2, "{stack}");
            let frame = stack["body"]["stackFrames"][0]["id"]
                .as_i64()
                .expect("stack frame");
            adapter.send(json!({"seq":6,"type":"request","command":"scopes","arguments":{"frameId":frame}})).await.unwrap();
            let scopes = debug_message(&mut adapter, &mut offset, &mut pending, |m| {
                m["type"] == "response" && m["request_seq"] == 6
            })
            .await;
            let locals = scopes["body"]["scopes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|scope| scope["name"] == "Locals")
                .expect("locals scope")["variablesReference"]
                .as_i64()
                .unwrap();
            adapter.send(json!({"seq":7,"type":"request","command":"variables","arguments":{"variablesReference":locals}})).await.unwrap();
            let variables = debug_message(&mut adapter, &mut offset, &mut pending, |m| {
                m["type"] == "response" && m["request_seq"] == 7
            })
            .await;
            assert!(
                variables["body"]["variables"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|variable| variable["name"] == "x" && variable["value"] == "1"),
                "{variables}"
            );
            adapter.send(json!({"seq":8,"type":"request","command":"continue","arguments":{"threadId":thread}})).await.unwrap();
            let continued = debug_message(&mut adapter, &mut offset, &mut pending, |m| {
                m["type"] == "response" && m["request_seq"] == 8
            })
            .await;
            assert_eq!(continued["success"], true, "{continued}");
            debug_message(&mut adapter, &mut offset, &mut pending, |m| {
                m["type"] == "event" && m["event"] == "terminated"
            })
            .await;
            drop(adapter);
        }
        manager.close("test-project").await;
        assert!(
            manager
                .exec(&config, "test-project", "exit 0", &[], &[])
                .await
                .is_err()
        );
        assert!(!running(&config, &first).await);
        let manager = DevContainers::default();
        let container = match manager
            .connect(&config, "test-project", &image, workspace.path())
            .await
        {
            Ok(name) => name,
            Err(_) => panic!("container did not start"),
        };
        drop(manager);
        timeout(Duration::from_secs(10), async {
            loop {
                let exists = config
                    .async_command()
                    .args(["container", "exists", &container])
                    .status()
                    .await
                    .expect("container exists check");
                if !exists.success() {
                    break;
                }
                sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("dropping session must remove its container");
        let failing_workspace = tempfile::tempdir().unwrap();
        std::fs::write(
            failing_workspace.path().join(".devcontainer.json"),
            r#"{"postCreateCommand":"exit 7"}"#,
        )
        .unwrap();
        let manager = DevContainers::default();
        assert!(
            manager
                .connect(&config, "failed-project", &image, failing_workspace.path())
                .await
                .is_err()
        );
        assert!(manager.active("failed-project").await.is_err());
    }
}
