//! HTTP endpoints for development container sessions.

use axum::{
    Json, Router,
    extract::{Path as AxumPath, Query, State},
    http::StatusCode,
    routing::{get, post, put},
};
use serde_json::{Value, json};

use super::super::{
    error::{GatewayError, GatewayResult},
    state::GatewayState,
};
use super::{unavailable, validate_file_path};

pub(in crate::server) fn routes() -> Router<GatewayState> {
    Router::new()
        .route(
            "/plugin-projects/{name}/container",
            get(status).post(open).delete(close),
        )
        .route("/plugin-projects/{name}/vscode-session", post(vscode_url))
        .route(
            "/plugin-projects/{name}/vscode-control",
            post(vscode_control),
        )
        .route(
            "/plugin-projects/{name}/terminal",
            get(terminal_output)
                .post(terminal_open)
                .delete(terminal_close),
        )
        .route(
            "/plugin-projects/{name}/terminal/input",
            post(terminal_input),
        )
        .route(
            "/plugin-projects/{name}/terminal/size",
            put(terminal_resize),
        )
        .route(
            "/plugin-projects/{name}/language/{language}",
            post(language_open),
        )
        .route(
            "/plugin-projects/{name}/language/{language}/{client}",
            get(language_output)
                .post(language_input)
                .delete(language_close),
        )
        .route("/plugin-projects/{name}/debug/{language}", post(debug_open))
        .route(
            "/plugin-projects/{name}/debug/{language}/{client}",
            get(debug_output).post(debug_input).delete(debug_close),
        )
}

#[derive(Default, serde::Deserialize)]
struct OpenOptions {
    #[serde(default)]
    rebuild: bool,
}

async fn open(
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

async fn close(
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

async fn status(
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
async fn vscode_url(
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
async fn vscode_url() -> GatewayResult<axum::response::Response> {
    Err(unavailable("VS Code 容器服务需要 Unix socket"))
}

#[derive(serde::Deserialize)]
struct VscodeControl {
    action: String,
    path: Option<String>,
    line: Option<u32>,
    column: Option<u32>,
}

#[cfg(unix)]
async fn vscode_control(
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
async fn vscode_control() -> GatewayResult<Json<Value>> {
    Err(unavailable("VS Code 容器服务需要 Unix socket"))
}

#[derive(serde::Deserialize)]
struct TerminalOffset {
    #[serde(default)]
    offset: u64,
}

#[derive(serde::Deserialize)]
struct TerminalInput {
    data: String,
}

#[derive(serde::Deserialize)]
struct TerminalSize {
    rows: u16,
    cols: u16,
}

async fn terminal_open(
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
            super::super::devterminal::Terminal::start(
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

async fn terminal_output(
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

async fn terminal_input(
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

async fn terminal_resize(
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

async fn terminal_close(
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

async fn language_open(
    State(state): State<GatewayState>,
    AxumPath((project, language)): AxumPath<(String, String)>,
) -> GatewayResult<Json<Value>> {
    let client = state
        .devcontainers
        .open_language(&state.infra.container_execution.config, &project, &language)
        .await?;
    Ok(Json(json!({"client": client})))
}

async fn language_output(
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

async fn language_input(
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

async fn language_close(
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

async fn debug_open(
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

async fn debug_output(
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

async fn debug_input(
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

async fn debug_close(
    State(state): State<GatewayState>,
    AxumPath((project, language, client)): AxumPath<(String, String, String)>,
) -> GatewayResult<Json<Value>> {
    language_close(
        State(state),
        AxumPath((project, format!("debug:{language}"), client)),
    )
    .await
}
