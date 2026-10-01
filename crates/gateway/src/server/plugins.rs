//! Manifest plugin inspection handlers.

use std::collections::HashSet;

use axum::Json;
use axum::extract::State;

use super::state::GatewayState;
use crate::proto::*;

// ── plugins ──────────────────────────────────────────────────────────

/// Installed manifest plugin families: what the plugins root holds, what
/// `plugins.toml` beside it declares, and which families the live node
/// registry actually registered at startup. Built-in node bundles are
/// compiled into the binary — a node-listing endpoint covers those.
#[utoipa::path(
    get,
    path = "/api/v1/plugins",
    tag = "plugins",
    responses((status = 200, body = PluginListView))
)]
pub(crate) async fn list_plugins(State(state): State<GatewayState>) -> Json<PluginListView> {
    // Same resolution the registry build used: the startup sync pins
    // AUTONOMICS_PLUGIN_ROOT when plugins.toml exists, else the loader
    // default — so this is the root the daemon actually scanned.
    let root = container_plugin::loader::default_plugins_root();
    let declared = container_plugin::manager::declared_sources(
        &container_plugin::manager::default_config_path(&root),
    )
    .unwrap_or_default()
    .unwrap_or_default();
    let registered: HashSet<String> = state
        .infra
        .engine_manager
        .list_nodes()
        .into_iter()
        .map(|node| node.kind)
        .collect();

    let plugins = container_plugin::manager::inspect_root(&root)
        .into_iter()
        .map(|family| {
            let source = declared.get(&family.dir).map(plugin_source_view);
            match family.manifest {
                Ok(manifest) => {
                    let kinds: Vec<String> = manifest
                        .nodes
                        .iter()
                        .map(|node| node.kind.clone())
                        .collect();
                    // A nodeless family registers nothing; vacuous truth
                    // would report it as registered.
                    let is_registered =
                        !kinds.is_empty() && kinds.iter().all(|kind| registered.contains(kind));
                    PluginView {
                        name: manifest.plugin_name.clone(),
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
                        source,
                        registered: is_registered,
                        error: None,
                    }
                }
                Err(detail) => PluginView {
                    // The directory name is all the identity a broken
                    // family has.
                    name: family.dir.clone(),
                    image: None,
                    kinds: Vec::new(),
                    panels: Vec::new(),
                    source,
                    registered: false,
                    error: Some(detail),
                },
            }
        })
        .collect();

    Json(PluginListView {
        root: root.display().to_string(),
        plugins,
    })
}

/// Map a `plugins.toml` source entry onto its wire view.
fn plugin_source_view(source: &container_plugin::sync::PluginSource) -> PluginSourceView {
    PluginSourceView {
        git: source.git.clone(),
        rev: source.rev.clone(),
        path: source.path.as_ref().map(|path| path.display().to_string()),
    }
}
