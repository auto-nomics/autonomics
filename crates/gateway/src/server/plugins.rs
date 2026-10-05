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
    let store = &state.infra.plugins;
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
                    status: status_text(manifest.status),
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
                    error: None,
                }
            })
            .collect(),
        Err(error) => vec![PluginView {
            name: "<plugin-store>".into(),
            status: "error".into(),
            image: None,
            kinds: Vec::new(),
            panels: Vec::new(),
            source: None,
            registered: false,
            error: Some(error.to_string()),
        }],
    };

    Json(PluginListView {
        root: store.root().display().to_string(),
        plugins,
    })
}

fn status_text(status: container_plugin::manifest::PluginStatus) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{status:?}").to_lowercase())
}

fn plugin_source_view(source: &container_plugin::sync::PluginSource) -> PluginSourceView {
    PluginSourceView {
        git: source.git.clone(),
        rev: source.rev.clone(),
        path: source.path.as_ref().map(|path| path.display().to_string()),
    }
}
