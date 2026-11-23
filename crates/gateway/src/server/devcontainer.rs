//! Persistent development containers for plugin projects.
//!
//! A session owns the stdin pipe of an attached `podman run --rm`. Closing the
//! pipe makes the container's idle process exit, so a gateway shutdown also
//! tears down its containers. Sessions always use the daemon's Podman store.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    process::{Output, Stdio},
    sync::Arc,
    time::Duration,
};

use axum::http::StatusCode;
use container_runtime::PodmanConfig;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::{
    process::Child,
    sync::{Mutex, OwnedMutexGuard},
    time::{sleep, timeout},
};

use super::error::{GatewayError, GatewayResult};

mod files;
mod http;
#[cfg(test)]
mod tests;

use files::SNAPSHOT_SCRIPT;
pub(crate) use files::{
    LIST_DIRECTORIES_SCRIPT, LIST_SCRIPT, create_directory_script, create_script,
    delete_directory_script, delete_script, read_script, rename_directory_script, rename_script,
    save_script, validate_directory_move, validate_file_path,
};
pub(super) use http::routes;

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
