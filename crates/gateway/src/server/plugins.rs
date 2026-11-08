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
        .create_plugin(&request.name, &request.environment_id, &request.purpose)
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
