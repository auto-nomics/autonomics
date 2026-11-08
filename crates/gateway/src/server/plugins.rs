//! Plugin lifecycle inspection handlers.

use std::collections::HashSet;

use axum::{Json, extract::State};

use super::state::GatewayState;
use crate::proto::*;

/// Plugin families from the daemon-owned PluginStore: lifecycle state,
/// persistent source declaration, and live registry registration.
#[utoipa::path(
    get,
    path = "/api/v1/plugins",
    tag = "plugins",
    responses((status = 200, body = PluginListView))
)]
pub(crate) async fn list_plugins(State(state): State<GatewayState>) -> Json<PluginListView> {
    let store = &state.infra.rsi.store();
    let declared = store.registry_sources().unwrap_or_default();
    let registered: HashSet<String> = state
        .infra
        .engine_manager
        .list_nodes()
        .into_iter()
        .map(|node| node.kind)
        .collect();

    let plugins = match store.list() {
        Ok(manifests) => manifests
            .into_iter()
            .map(|manifest| {
                let kinds: Vec<String> = manifest
                    .nodes
                    .iter()
                    .map(|node| node.kind.clone())
                    .collect();
                let is_registered =
                    !kinds.is_empty() && kinds.iter().all(|kind| registered.contains(kind));
                PluginView {
                    name: manifest.plugin_name.clone(),
                    status: status_text(&manifest),
                    image: Some(manifest.image.reference.to_string()),
                    kinds,
                    panels: manifest
                        .panels
                        .iter()
                        .map(|panel| PluginPanelView {
                            binding: panel.binding.clone(),
                            mount: panel.mount.clone(),
                            bundle: panel.bundle.to_string(),
                        })
                        .collect(),
                    source: declared.get(&manifest.plugin_name).map(plugin_source_view),
                    registered: is_registered,
                }
            })
            .collect(),
        Err(error) => {
            tracing::warn!(error = %error, "cannot list plugin store for /plugins");
            Vec::new()
        }
    };

    Json(PluginListView {
        root: store.root().display().to_string(),
        plugins,
    })
}

fn status_label<T: serde::Serialize + std::fmt::Debug>(status: T) -> String {
    serde_json::to_value(&status)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{status:?}").to_lowercase())
}

fn status_text(manifest: &container_plugin::manifest::PluginManifest) -> String {
    if let Some(status) = manifest.installation.status {
        return status_label(status);
    }
    status_label(manifest.status)
}

fn plugin_source_view(source: &container_plugin::sync::PluginSource) -> PluginSourceView {
    PluginSourceView {
        git: source.git.clone(),
        rev: source.rev.clone(),
        path: source.path.as_ref().map(|path| path.display().to_string()),
        local_commit: source.local_commit.clone(),
        local_digest: source.local_digest.clone(),
    }
}

/// Development projects, including incomplete or invalid workspaces.
#[utoipa::path(get, path = "/api/v1/plugin-projects", tag = "plugins",
    responses((status = 200, body = PluginProjectListView)))]
pub(crate) async fn list_projects(
    State(state): State<GatewayState>,
) -> super::error::GatewayResult<Json<PluginProjectListView>> {
    let store = state.infra.rsi.store();
    let projects = store
        .list_projects()
        .map_err(|error| {
            super::error::GatewayError::Status(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                error.to_string(),
            )
        })?
        .into_iter()
        .map(|entry| {
            let manifest = entry.manifest;
            PluginProjectView {
                name: entry.name,
                status: manifest.as_ref().map(|m| status_label(m.status)),
                image: manifest.as_ref().map(|m| m.image.reference.to_string()),
                kinds: manifest
                    .as_ref()
                    .map(|m| m.nodes.iter().map(|n| n.kind.clone()).collect())
                    .unwrap_or_default(),
                error: entry.error,
            }
        })
        .collect();
    Ok(Json(PluginProjectListView { projects }))
}

/// Create a draft project bound to an approved development environment.
#[utoipa::path(post, path = "/api/v1/plugin-projects", tag = "plugins",
    request_body = CreatePluginProjectRequest,
    responses((status = 200, body = PluginProjectView), (status = 400, description = "Invalid project request")))]
pub(crate) async fn create_project(
    State(state): State<GatewayState>,
    Json(request): Json<CreatePluginProjectRequest>,
) -> super::error::GatewayResult<Json<PluginProjectView>> {
    let operator = state
        .infra
        .rsi
        .create_plugin_scaffolded(&request.name, &request.environment_id, &request.purpose)
        .map_err(|error| super::error::GatewayError::Message(error.to_string()))?;
    let manifest = operator.manifest();
    Ok(Json(PluginProjectView {
        name: manifest.plugin_name.clone(),
        status: Some(status_label(manifest.status)),
        image: Some(manifest.image.reference.to_string()),
        kinds: Vec::new(),
        error: None,
    }))
}

#[derive(serde::Deserialize)]
pub(crate) struct FileQuery {
    path: String,
}

#[derive(serde::Deserialize)]
pub(crate) struct SaveProjectFile {
    path: String,
    contents: String,
    original: String,
}

#[derive(serde::Deserialize)]
pub(crate) struct CreateProjectFile {
    path: String,
}

#[derive(serde::Deserialize)]
pub(crate) struct RenameProjectFile {
    path: String,
    new_path: String,
}

pub(crate) fn project_workspace(
    state: &GatewayState,
    name: &str,
) -> super::error::GatewayResult<plugin_rsi::PluginWorkspace> {
    use super::error::GatewayError;
    plugin_rsi::validate_plugin_name(name).map_err(|e| GatewayError::Message(e.to_string()))?;
    let root = state.infra.rsi.store().root().join(name);
    let metadata = std::fs::symlink_metadata(&root).map_err(|_| {
        GatewayError::Status(
            axum::http::StatusCode::NOT_FOUND,
            "Project not found".into(),
        )
    })?;
    if !metadata.file_type().is_dir() {
        return Err(GatewayError::Message("Project must be a directory".into()));
    }
    Ok(plugin_rsi::PluginWorkspace::new(root))
}

fn container_output(output: std::process::Output) -> super::error::GatewayResult<Vec<u8>> {
    use super::error::GatewayError;
    if output.status.success() {
        return Ok(output.stdout);
    }
    if output.status.code() == Some(42) {
        return Err(GatewayError::Status(
            axum::http::StatusCode::CONFLICT,
            "目标文件已存在或文件已在磁盘上更改".into(),
        ));
    }
    let detail = String::from_utf8_lossy(&output.stderr);
    Err(GatewayError::Message(format!(
        "开发容器文件操作失败：{}",
        detail.trim()
    )))
}

pub(crate) async fn create_project_file(
    State(state): State<GatewayState>,
    axum::extract::Path(name): axum::extract::Path<String>,
    Json(request): Json<CreateProjectFile>,
) -> super::error::GatewayResult<Json<serde_json::Value>> {
    super::devcontainer::validate_file_path(&request.path)?;
    let output = state
        .devcontainers
        .exec(
            &state.infra.container_execution.config,
            &name,
            &super::devcontainer::create_script(),
            &[&request.path],
            &[],
        )
        .await?;
    container_output(output)?;
    Ok(Json(serde_json::json!({"path": request.path})))
}

pub(crate) async fn create_project_directory(
    State(state): State<GatewayState>,
    axum::extract::Path(name): axum::extract::Path<String>,
    Json(request): Json<CreateProjectFile>,
) -> super::error::GatewayResult<Json<serde_json::Value>> {
    super::devcontainer::validate_file_path(&request.path)?;
    let output = state
        .devcontainers
        .exec(
            &state.infra.container_execution.config,
            &name,
            &super::devcontainer::create_directory_script(),
            &[&request.path],
            &[],
        )
        .await?;
    container_output(output)?;
    Ok(Json(serde_json::json!({"path": request.path})))
}

pub(crate) async fn delete_project_directory(
    State(state): State<GatewayState>,
    axum::extract::Path(name): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<FileQuery>,
) -> super::error::GatewayResult<Json<serde_json::Value>> {
    super::devcontainer::validate_file_path(&query.path)?;
    let output = state
        .devcontainers
        .exec(
            &state.infra.container_execution.config,
            &name,
            &super::devcontainer::delete_directory_script(),
            &[&query.path],
            &[],
        )
        .await?;
    container_output(output)?;
    Ok(Json(serde_json::json!({"path": query.path})))
}

pub(crate) async fn rename_project_directory(
    State(state): State<GatewayState>,
    axum::extract::Path(name): axum::extract::Path<String>,
    Json(request): Json<RenameProjectFile>,
) -> super::error::GatewayResult<Json<serde_json::Value>> {
    super::devcontainer::validate_directory_move(&request.path, &request.new_path)?;
    let output = state
        .devcontainers
        .exec(
            &state.infra.container_execution.config,
            &name,
            &super::devcontainer::rename_directory_script(),
            &[&request.path, &request.new_path],
            &[],
        )
        .await?;
    container_output(output)?;
    Ok(Json(serde_json::json!({"path": request.new_path})))
}

pub(crate) async fn delete_project_file(
    State(state): State<GatewayState>,
    axum::extract::Path(name): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<FileQuery>,
) -> super::error::GatewayResult<Json<serde_json::Value>> {
    super::devcontainer::validate_file_path(&query.path)?;
    let output = state
        .devcontainers
        .exec(
            &state.infra.container_execution.config,
            &name,
            &super::devcontainer::delete_script(),
            &[&query.path],
            &[],
        )
        .await?;
    container_output(output)?;
    Ok(Json(serde_json::json!({"path": query.path})))
}

pub(crate) async fn rename_project_file(
    State(state): State<GatewayState>,
    axum::extract::Path(name): axum::extract::Path<String>,
    Json(request): Json<RenameProjectFile>,
) -> super::error::GatewayResult<Json<serde_json::Value>> {
    super::devcontainer::validate_file_path(&request.path)?;
    super::devcontainer::validate_file_path(&request.new_path)?;
    let output = state
        .devcontainers
        .exec(
            &state.infra.container_execution.config,
            &name,
            &super::devcontainer::rename_script(),
            &[&request.path, &request.new_path],
            &[],
        )
        .await?;
    container_output(output)?;
    Ok(Json(serde_json::json!({"path": request.new_path})))
}

pub(crate) async fn project_files(
    State(state): State<GatewayState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> super::error::GatewayResult<Json<serde_json::Value>> {
    let script = super::devcontainer::LIST_SCRIPT;
    let output = state
        .devcontainers
        .exec(
            &state.infra.container_execution.config,
            &name,
            script,
            &[],
            &[],
        )
        .await?;
    let files = parse_container_paths(&container_output(output)?)?;
    let output = state
        .devcontainers
        .exec(
            &state.infra.container_execution.config,
            &name,
            super::devcontainer::LIST_DIRECTORIES_SCRIPT,
            &[],
            &[],
        )
        .await?;
    let directories = parse_container_paths(&container_output(output)?)?;
    Ok(Json(
        serde_json::json!({"files": files, "directories": directories}),
    ))
}

pub(crate) async fn project_snapshot(
    State(state): State<GatewayState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> super::error::GatewayResult<Json<serde_json::Value>> {
    let output = state
        .devcontainers
        .snapshot(&state.infra.container_execution.config, &name)
        .await?;
    let bytes = container_output(output)?;
    let entries = parse_project_snapshot(&bytes)?;
    Ok(Json(serde_json::json!({"entries": entries})))
}

pub(super) fn parse_project_snapshot(
    bytes: &[u8],
) -> super::error::GatewayResult<Vec<serde_json::Value>> {
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(super::error::GatewayError::Message(
            "项目文件监控快照过大".into(),
        ));
    }
    if bytes.is_empty() || bytes.last() != Some(&0) {
        return Err(super::error::GatewayError::Message(
            "文件监控快照不完整".into(),
        ));
    }
    let mut fields = bytes.split(|byte| *byte == 0);
    let mut entries = Vec::new();
    loop {
        let Some(kind) = fields.next() else {
            break;
        };
        if kind.is_empty() {
            if fields.next().is_some() {
                return Err(super::error::GatewayError::Message(
                    "文件监控快照无效".into(),
                ));
            }
            break;
        }
        let path = fields
            .next()
            .ok_or_else(|| super::error::GatewayError::Message("文件监控快照不完整".into()))?;
        let modified = fields
            .next()
            .ok_or_else(|| super::error::GatewayError::Message("文件监控快照不完整".into()))?;
        let size = fields
            .next()
            .ok_or_else(|| super::error::GatewayError::Message("文件监控快照不完整".into()))?;
        let path = std::str::from_utf8(path)
            .map_err(|_| super::error::GatewayError::Message("文件路径不是有效的 UTF-8".into()))?;
        if path == "." {
            continue;
        }
        let path = path
            .strip_prefix("./")
            .ok_or_else(|| super::error::GatewayError::Message("文件监控路径无效".into()))?;
        super::devcontainer::validate_file_path(path)?;
        let kind = match kind {
            b"f" => "file",
            b"d" => "directory",
            _ => {
                return Err(super::error::GatewayError::Message(
                    "文件监控类型无效".into(),
                ));
            }
        };
        let modified = std::str::from_utf8(modified)
            .map_err(|_| super::error::GatewayError::Message("文件监控元数据无效".into()))?;
        let size = std::str::from_utf8(size)
            .map_err(|_| super::error::GatewayError::Message("文件监控元数据无效".into()))?;
        entries.push(
            serde_json::json!({"path": path, "kind": kind, "stamp": format!("{modified}:{size}")}),
        );
    }
    Ok(entries)
}

fn parse_container_paths(data: &[u8]) -> super::error::GatewayResult<Vec<String>> {
    let mut files = Vec::new();
    for item in data
        .split(|byte| *byte == 0)
        .filter(|item| !item.is_empty())
    {
        let file = std::str::from_utf8(item)
            .map_err(|_| super::error::GatewayError::Message("文件路径不是有效的 UTF-8".into()))?;
        if file != "." {
            files.push(file.strip_prefix("./").unwrap_or(file).to_owned());
        }
    }
    files.sort();
    Ok(files)
}

pub(crate) async fn read_project_file(
    State(state): State<GatewayState>,
    axum::extract::Path(name): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<FileQuery>,
) -> super::error::GatewayResult<Json<serde_json::Value>> {
    super::devcontainer::validate_file_path(&query.path)?;
    let script = super::devcontainer::read_script();
    let output = state
        .devcontainers
        .exec(
            &state.infra.container_execution.config,
            &name,
            &script,
            &[&query.path],
            &[],
        )
        .await?;
    let contents = String::from_utf8(container_output(output)?)
        .map_err(|_| super::error::GatewayError::Message("文件不是有效的 UTF-8".into()))?;
    Ok(Json(
        serde_json::json!({"path": query.path, "contents": contents}),
    ))
}

pub(crate) async fn save_project_file(
    State(state): State<GatewayState>,
    axum::extract::Path(name): axum::extract::Path<String>,
    Json(request): Json<SaveProjectFile>,
) -> super::error::GatewayResult<Json<serde_json::Value>> {
    use super::error::GatewayError;
    super::devcontainer::validate_file_path(&request.path)?;
    if request.contents.len() > 1_048_576 || request.original.len() > 1_048_576 {
        return Err(GatewayError::Message("文件超过大小限制".into()));
    }
    let script = super::devcontainer::save_script();
    let mut input = request.original.as_bytes().to_vec();
    input.extend_from_slice(request.contents.as_bytes());
    let length = request.original.len().to_string();
    let output = state
        .devcontainers
        .exec(
            &state.infra.container_execution.config,
            &name,
            &script,
            &[&request.path, &length],
            &input,
        )
        .await?;
    container_output(output)?;
    Ok(Json(
        serde_json::json!({"path": request.path, "contents": request.contents}),
    ))
}

// ── Development sessions, validation, activation, test-run ─────────────
//
// The devcontainer loop behind the GUI dev window: session lifecycle,
// six-gate validation, local activation with hot reload, the latest
// validation report, and one-node test-runs. All mutations dispatch
// through PluginInfra so permission/lifecycle invariants stay in one
// place; the handlers only map errors onto the gateway convention.

/// Map a plugin-rsi error onto HTTP semantics: unknown projects and node
/// kinds are 404, everything else is a client-visible message (400).
fn rsi_error(error: plugin_rsi::Error) -> super::error::GatewayError {
    use super::error::GatewayError;
    let message = error.to_string();
    if message.contains("unknown plugin") || message.contains("has no node kind") {
        GatewayError::Status(axum::http::StatusCode::NOT_FOUND, message)
    } else {
        GatewayError::Message(message)
    }
}

fn sessions(state: &GatewayState) -> super::error::GatewayResult<plugin_rsi::DevSessionManager> {
    state
        .infra
        .rsi
        .dev_sessions()
        .map_err(|error| super::error::GatewayError::Status(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            error.to_string(),
        ))
}

#[derive(serde::Deserialize, utoipa::ToSchema)]
pub(crate) struct DevSessionAction {
    /// `start` | `stop` | `restart`.
    pub action: String,
    /// Opt into network egress for the session (default isolated).
    #[serde(default)]
    pub network: bool,
}

/// `GET /plugin-projects/{name}/dev-session` — probe the dev container.
#[utoipa::path(get, path = "/api/v1/plugin-projects/{name}/dev-session", tag = "plugins",
    params(("name" = String, Path, description = "Plugin project name")),
    responses((status = 200, description = "Session state: absent|created|running|exited|unknown (+detail)")))]
pub(crate) async fn dev_session_status(
    State(state): State<GatewayState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> super::error::GatewayResult<Json<plugin_rsi::DevSessionView>> {
    let manager = sessions(&state)?;
    let view = manager.status(&name).await.map_err(rsi_error)?;
    Ok(Json(view))
}

/// `POST /plugin-projects/{name}/dev-session` — start/stop/restart.
#[utoipa::path(post, path = "/api/v1/plugin-projects/{name}/dev-session", tag = "plugins",
    params(("name" = String, Path)),
    request_body = DevSessionAction,
    responses((status = 200, description = "Session state after the action")))]
pub(crate) async fn dev_session_action(
    State(state): State<GatewayState>,
    axum::extract::Path(name): axum::extract::Path<String>,
    Json(request): Json<DevSessionAction>,
) -> super::error::GatewayResult<Json<plugin_rsi::DevSessionView>> {
    use container_runtime::ContainerNetwork;
    let manager = sessions(&state)?;
    let network = if request.network {
        ContainerNetwork::Egress
    } else {
        ContainerNetwork::Isolated
    };
    let view = match request.action.as_str() {
        "start" => manager.start(&name, network).await,
        "stop" => manager.stop(&name).await,
        "restart" => manager.restart(&name, network).await,
        other => {
            return Err(super::error::GatewayError::Message(format!(
                "unknown dev-session action `{other}` (start|stop|restart)"
            )))
        }
    }
    .map_err(rsi_error)?;
    Ok(Json(view))
}

/// `POST /plugin-projects/{name}/validate` — run the six gates, keep the
/// workspace editable.
#[utoipa::path(post, path = "/api/v1/plugin-projects/{name}/validate", tag = "plugins",
    params(("name" = String, Path)),
    responses((status = 200, description = "{outcome: passed|needs_fix, report}")))]
pub(crate) async fn validate_project(
    State(state): State<GatewayState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> super::error::GatewayResult<Json<serde_json::Value>> {
    let outcome = state.infra.rsi.validate_local(&name).map_err(rsi_error)?;
    let (label, report) = match outcome {
        plugin_rsi::ValidationOutcome::Passed(report) => ("passed", report),
        plugin_rsi::ValidationOutcome::NeedsFix(report) => ("needs_fix", report),
        plugin_rsi::ValidationOutcome::Submitted(report) => ("submitted", report),
    };
    Ok(Json(serde_json::json!({
        "outcome": label,
        "report": serde_json::to_value(&report).unwrap_or_default(),
    })))
}

/// `POST /plugin-projects/{name}/activate` — validate, install the local
/// snapshot, hot-reload the registry (nodes appear as `plugin/node`).
#[utoipa::path(post, path = "/api/v1/plugin-projects/{name}/activate", tag = "plugins",
    params(("name" = String, Path)),
    responses((status = 200, description = "{activated: bool, report}")))]
pub(crate) async fn activate_project(
    State(state): State<GatewayState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> super::error::GatewayResult<Json<serde_json::Value>> {
    match state
        .infra
        .rsi
        .validate_and_activate_local(&name)
        .map_err(rsi_error)?
    {
        plugin_rsi::LocalActivationOutcome::Activated(report, source) => {
            Ok(Json(serde_json::json!({
                "activated": true,
                "report": serde_json::to_value(&report).unwrap_or_default(),
                "source": {
                    "commit": source.commit,
                    "digest": source.digest,
                },
            })))
        }
        plugin_rsi::LocalActivationOutcome::NeedsFix(report) => Ok(Json(serde_json::json!({
            "activated": false,
            "report": serde_json::to_value(&report).unwrap_or_default(),
        }))),
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct ReportQuery {
    attempt: Option<u32>,
}

/// `GET /plugin-projects/{name}/report` — the latest (or a specific)
/// validation report from the workspace's `.rsi/reports/`.
#[utoipa::path(get, path = "/api/v1/plugin-projects/{name}/report", tag = "plugins",
    params(
        ("name" = String, Path),
        ("attempt" = Option<u32>, Query, description = "Specific attempt; default latest"),
    ),
    responses((status = 200, description = "ValidationReport JSON"), (status = 404, description = "No report yet")))]
pub(crate) async fn latest_report(
    State(state): State<GatewayState>,
    axum::extract::Path(name): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<ReportQuery>,
) -> super::error::GatewayResult<Json<serde_json::Value>> {
    let workspace = project_workspace(&state, &name)?;
    // Attempt file names are `attempt-<n>.json`; pick the requested or
    // the highest without reading the manifest (which may be mid-edit).
    let reports_dir = workspace.path().join(".rsi/reports");
    let entries = std::fs::read_dir(&reports_dir).map_err(|_| {
        super::error::GatewayError::Status(
            axum::http::StatusCode::NOT_FOUND,
            format!("plugin `{name}` has no validation report yet"),
        )
    })?;
    let mut attempts: Vec<u32> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let file = entry.file_name();
            let file = file.to_str()?;
            file.strip_prefix("attempt-")?
                .strip_suffix(".json")?
                .parse()
                .ok()
        })
        .collect();
    attempts.sort_unstable();
    let attempt = query
        .attempt
        .or_else(|| attempts.last().copied())
        .ok_or_else(|| {
            super::error::GatewayError::Status(
                axum::http::StatusCode::NOT_FOUND,
                format!("plugin `{name}` has no validation report yet"),
            )
        })?;
    if !attempts.contains(&attempt) {
        return Err(super::error::GatewayError::Status(
            axum::http::StatusCode::NOT_FOUND,
            format!("plugin `{name}` has no report for attempt {attempt}"),
        ));
    }
    let text = workspace
        .read_text(&format!(".rsi/reports/attempt-{attempt}.json"))
        .map_err(|_| {
            super::error::GatewayError::Status(
                axum::http::StatusCode::NOT_FOUND,
                format!("cannot read attempt {attempt} of plugin `{name}`"),
            )
        })?;
    let report: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
        super::error::GatewayError::Status(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("corrupt report attempt {attempt}: {error}"),
        )
    })?;
    Ok(Json(report))
}

/// `POST /plugin-projects/{name}/nodes/{kind}/test-run` — run one node
/// against sample inputs (production container contract).
#[utoipa::path(post, path = "/api/v1/plugin-projects/{name}/nodes/{kind}/test-run", tag = "plugins",
    params(("name" = String, Path), ("kind" = String, Path)),
    request_body(content = DevSessionAction, description = "{kind is taken from the path; body is {inputs: {port: text}, params: {...}}}"),
    responses((status = 200, description = "TestRunResult: exit_code/stdout/stderr plus per-output {path, present, content, truncated}"), (status = 404, description = "Unknown project or kind")))]
pub(crate) async fn test_run_node(
    State(state): State<GatewayState>,
    axum::extract::Path((name, kind)): axum::extract::Path<(String, String)>,
    Json(mut request): Json<plugin_rsi::TestRunRequest>,
) -> super::error::GatewayResult<Json<plugin_rsi::TestRunResult>> {
    let manager = sessions(&state)?;
    request.kind = kind;
    // A container run is genuinely long; keep the request off the
    // async executor's sync paths.
    let result = manager
        .test_run(&name, &request)
        .await
        .map_err(rsi_error)?;
    Ok(Json(result))
}

#[derive(serde::Deserialize, utoipa::ToSchema)]
pub(crate) struct ForkPluginRequest {
    pub new_name: String,
    pub purpose: String,
}

/// `POST /plugins/{name}/fork` — fork an installed plugin into a new dev
/// workspace.
#[utoipa::path(post, path = "/api/v1/plugins/{name}/fork", tag = "plugins",
    params(("name" = String, Path)),
    request_body = ForkPluginRequest,
    responses((status = 200, description = "Forked project view")))]
pub(crate) async fn fork_plugin(
    State(state): State<GatewayState>,
    axum::extract::Path(name): axum::extract::Path<String>,
    Json(request): Json<ForkPluginRequest>,
) -> super::error::GatewayResult<Json<serde_json::Value>> {
    let operator = state
        .infra
        .rsi
        .fork_plugin(&name, &request.new_name, &request.purpose)
        .map_err(rsi_error)?;
    let manifest = operator.manifest();
    Ok(Json(serde_json::json!({
        "name": request.new_name,
        "status": status_label(manifest.status),
        "image": manifest.image.reference.to_string(),
        "kinds": manifest.nodes.iter().map(|node| node.kind.clone()).collect::<Vec<_>>(),
        "error": serde_json::Value::Null,
    })))
}

/// `POST /plugins/{name}/uninstall` — remove from the registry and
/// runtime source (workspace and snapshots are retained). Stops the dev
/// session first when one is running.
#[utoipa::path(post, path = "/api/v1/plugins/{name}/uninstall", tag = "plugins",
    params(("name" = String, Path)),
    responses((status = 200, description = "Uninstalled")))]
pub(crate) async fn uninstall_plugin(
    State(state): State<GatewayState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> super::error::GatewayResult<Json<serde_json::Value>> {
    // Best-effort dev-session teardown; uninstall works without podman.
    if let Ok(manager) = sessions(&state) {
        let _ = manager.stop(&name).await;
    }
    let commit = state
        .infra
        .rsi
        .uninstall(&name)
        .map_err(rsi_error)?
        .commit()
        .to_string();
    Ok(Json(serde_json::json!({ "name": name, "uninstalled": true, "commit": commit })))
}

/// `POST /plugins/{name}/rollback` — restore the previous immutable
/// source and refresh registration.
#[utoipa::path(post, path = "/api/v1/plugins/{name}/rollback", tag = "plugins",
    params(("name" = String, Path)),
    responses((status = 200, description = "Rolled back")))]
pub(crate) async fn rollback_plugin(
    State(state): State<GatewayState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> super::error::GatewayResult<Json<serde_json::Value>> {
    let commit = state
        .infra
        .rsi
        .rollback(&name)
        .map_err(rsi_error)?
        .commit()
        .to_string();
    Ok(Json(serde_json::json!({ "name": name, "rolled_back": true, "commit": commit })))
}

/// `GET /plugin-projects/{name}/nodes` — the manifest's node
/// declarations (kind + inlet/outlet ports) for the dev window's test
/// panel.
#[utoipa::path(get, path = "/api/v1/plugin-projects/{name}/nodes", tag = "plugins",
    params(("name" = String, Path)),
    responses((status = 200, description = "Node declarations: kinds with input port labels and declared outputs")))]
pub(crate) async fn list_project_nodes(
    State(state): State<GatewayState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> super::error::GatewayResult<Json<serde_json::Value>> {
    let workspace = project_workspace(&state, &name)?;
    let manifest: container_plugin::manifest::PluginManifest = toml::from_str(
        &workspace
            .read_text("manifest.toml")
            .map_err(|error| super::error::GatewayError::Status(
                axum::http::StatusCode::NOT_FOUND,
                format!("cannot read manifest of `{name}`: {error}"),
            ))?,
    )
    .map_err(|error| super::error::GatewayError::Message(format!("invalid manifest: {error}")))?;
    let nodes: Vec<serde_json::Value> = manifest
        .nodes
        .iter()
        .map(|node| {
            serde_json::json!({
                "kind": node.kind,
                "desc": node.desc,
                "inputs": node.ports.inputs.iter().map(|input| serde_json::json!({
                    "label": input.label,
                    "type": match input.r#type { container_plugin::node_definition::PortKind::File => "file", container_plugin::node_definition::PortKind::Tuple => "tuple" },
                })).collect::<Vec<_>>(),
                "outputs": node.ports.outputs.iter().map(|output| serde_json::json!({
                    "path": output.path,
                    "label": output.label,
                    "optional": output.optional,
                })).collect::<Vec<_>>(),
                "params": serde_json::to_value(container_plugin::compile::compile_schema(&node.params)).unwrap_or_default(),
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "nodes": nodes, "count": nodes.len() })))
}

pub(crate) async fn dev_project_files(
    State(state): State<GatewayState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> super::error::GatewayResult<Json<serde_json::Value>> {
    let workspace = project_workspace(&state, &name)?;
    let files = workspace
        .list_files()
        .map_err(|e| super::error::GatewayError::Message(e.to_string()))?;
    Ok(Json(serde_json::json!({"files": files})))
}

pub(crate) async fn read_dev_project_file(
    State(state): State<GatewayState>,
    axum::extract::Path(name): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<FileQuery>,
) -> super::error::GatewayResult<Json<serde_json::Value>> {
    let workspace = project_workspace(&state, &name)?;
    let contents = workspace
        .read_text(&query.path)
        .map_err(|e| super::error::GatewayError::Message(e.to_string()))?;
    Ok(Json(
        serde_json::json!({"path": query.path, "contents": contents}),
    ))
}

pub(crate) async fn save_dev_project_file(
    State(state): State<GatewayState>,
    axum::extract::Path(name): axum::extract::Path<String>,
    Json(request): Json<SaveProjectFile>,
) -> super::error::GatewayResult<Json<serde_json::Value>> {
    use super::error::GatewayError;
    let workspace = project_workspace(&state, &name)?;
    let current = workspace
        .read_text(&request.path)
        .map_err(|e| GatewayError::Message(e.to_string()))?;
    if current != request.original {
        return Err(GatewayError::Status(
            axum::http::StatusCode::CONFLICT,
            "File changed on disk; reload it before saving".into(),
        ));
    }
    workspace
        .write_text(&request.path, &request.contents)
        .map_err(|e| GatewayError::Message(e.to_string()))?;
    Ok(Json(
        serde_json::json!({"path": request.path, "contents": request.contents}),
    ))
}
