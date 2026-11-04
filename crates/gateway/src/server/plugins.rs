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

fn project_workspace(
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

pub(crate) async fn project_files(
    State(state): State<GatewayState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> super::error::GatewayResult<Json<serde_json::Value>> {
    let workspace = project_workspace(&state, &name)?;
    let files = workspace
        .list_files()
        .map_err(|e| super::error::GatewayError::Message(e.to_string()))?;
    Ok(Json(serde_json::json!({"files": files})))
}

pub(crate) async fn read_project_file(
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

pub(crate) async fn save_project_file(
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
