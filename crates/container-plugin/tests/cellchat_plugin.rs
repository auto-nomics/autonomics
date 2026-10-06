//! Contract lock for the cellchat plugin family (tier-1 official wrap):
//! official CellChat (jinworks/CellChat, GPL-3) pinned to post-#438 main
//! SHA 75253cd0 inside a digest-pinned image — the v2.1.2 release tag
//! carries a bootstrap parallelization regression (10-20x slowdown at
//! >=40k cells), so the pin deliberately tracks main past the fix.
//! CellChatDB ships inside the package (LazyData): no runtime downloads.
//! The plugin directory lives outside this repository
//! (`/mnt/projects/node-plugins` by default, overridable via
//! `NODE_PLUGINS_ROOT`); the test is skipped when absent so CI without
//! the plugin checkout stays green.

use std::path::PathBuf;

use container_plugin::compile::spec_compile::compile_container_spec;
use container_plugin::manifest::PluginManifest;
use container_plugin::node_definition::ParamType;
use serde_json::json;

/// Registry digest of the pushed image — filled after the ghcr push;
/// the local-build digest is NOT valid (podman converts the manifest
/// format during push, see the cellphonedb family notes).
const IMAGE: &str =
    "ghcr.io/zj-2002/cellchat@sha256:6fdc08a21395fd2710cb9ac0d632079adb1364ae621fc4f8f809f5b368d09f4b";

fn plugin_root() -> Option<PathBuf> {
    let explicit = std::env::var_os("NODE_PLUGINS_ROOT");
    let root = explicit
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/projects/node-plugins"));
    let manifest = root.join("cellchat").join("manifest.toml");
    if manifest.is_file() {
        return Some(root);
    }
    if explicit.is_some() {
        // R04 hygiene: an explicitly set NODE_PLUGINS_ROOT means the
        // contract lock was requested; a missing family must fail loudly.
        panic!(
            "NODE_PLUGINS_ROOT is set but the cellchat family is not \
             deployed under it ({}) — the contract lock cannot run. Deploy \
             the family or unset NODE_PLUGINS_ROOT to skip deliberately.",
            manifest.display()
        );
    }
    eprintln!("skipping: cellchat plugin directory not present (NODE_PLUGINS_ROOT unset)");
    None
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let text = std::fs::read_to_string(root.join("cellchat").join("manifest.toml")).unwrap();
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        if let Some(relative) = node.command.script_file.clone() {
            let source = std::fs::read_to_string(root.join("cellchat").join(&relative)).unwrap();
            node.command.script = Some(source);
            node.command.script_file = None;
        }
    }
    manifest
}

fn node_by_kind<'a>(
    manifest: &'a PluginManifest,
    kind: &str,
) -> &'a container_plugin::node_definition::NodeDefinition {
    manifest
        .nodes
        .iter()
        .find(|n| n.kind == kind)
        .unwrap_or_else(|| panic!("{kind} missing from manifest"))
}

#[test]
fn cellchat_inference_compiles_to_the_official_wrap_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    assert_eq!(manifest.plugin_name, "cellchat");
    let node = node_by_kind(&manifest, "cellchat_inference");

    let compiled =
        compile_container_spec(node, &manifest.image, &manifest.panels, &json!({})).unwrap();

    // Tier-1 identity: digest pins the official package (post-#438 main
    // SHA) together with its bundled CellChatDB.
    assert_eq!(compiled.image, IMAGE);

    // Baked R driver, no injected script, no panels, no mounted files.
    assert_eq!(
        compiled.command,
        vec![
            "Rscript".to_string(),
            "/opt/autonomics/driver.R".to_string()
        ]
    );
    assert_eq!(compiled.script, None);
    assert!(compiled.panels.is_empty());
    assert!(compiled.files.is_empty());
    assert_eq!(compiled.panel_bundles.len(), 0);

    // Single required input (log-normalized H5AD; meta derived from obs
    // inside the container — asserted at the manifest level).
    assert_eq!(node.ports.inputs.len(), 1);

    // Stable 7-file output contract: five official tables, the full
    // CellChat object for the record, and the provenance JSON.
    assert_eq!(compiled.outputs.len(), 7);
    let contract: Vec<(&str, &str)> = compiled
        .outputs
        .iter()
        .map(|o| (o.path.as_str(), o.format.as_deref().unwrap_or("")))
        .collect();
    assert_eq!(
        contract,
        vec![
            ("cellchat_lr_pairs.csv", "cellchat_table"),
            ("cellchat_pathway_probs.csv", "cellchat_table"),
            ("cellchat_net_count.csv", "cellchat_matrix"),
            ("cellchat_net_weight.csv", "cellchat_matrix"),
            ("cellchat_centrality.csv", "cellchat_table"),
            ("cellchat_object.rds", "cellchat_rds"),
            ("cellchat_provenance.json", "cellchat_provenance_json"),
        ]
    );

    // DB is bundled in the package: the node never needs egress.
    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert!(matches!(
        compiled.pull_policy,
        container_runtime::PullPolicy::Missing
    ));
    // Bootstrap permutations are long-running at real dataset sizes.
    assert_eq!(compiled.timeout_secs, 86400);
    assert_eq!(compiled.artifact_prefix, "/artifacts/cellchat_inference");
    assert_eq!(compiled.workdir, None);
    assert_eq!(compiled.cpus, Some(8.0));
    assert_eq!(compiled.memory.as_deref(), Some("32Gi"));
    assert_eq!(compiled.pids_limit, Some(1024));
    assert_eq!(compiled.shm_size.as_deref(), Some("1Gi"));

    // Knobs flow through CCHAT_* env; no bool/flag params by design (the
    // engine base for this lock predates the flag variant, and every
    // v1 knob is naturally string/int).
    assert_eq!(compiled.env.len(), 6);
    assert_eq!(compiled.env.get("CCHAT_CLUSTER_COLUMN").unwrap(), "cluster");
    assert_eq!(compiled.env.get("CCHAT_SPECIES").unwrap(), "human");
    assert_eq!(compiled.env.get("CCHAT_NBOOT").unwrap(), "100");
    assert_eq!(compiled.env.get("CCHAT_WORKERS").unwrap(), "4");
    assert_eq!(compiled.env.get("CCHAT_DB_SCOPE").unwrap(), "full");
    assert_eq!(compiled.env.get("CCHAT_NORMALIZED").unwrap(), "auto");

    // Param schema: nboot floored and CAPPED (1000 is day-scale at 50k
    // cells — inherited physical limit, documented in the manifest doc).
    assert_eq!(manifest.nodes[0].params.len(), 6);
    let nboot = &manifest.nodes[0].params["nboot"];
    assert!(matches!(nboot.r#type, ParamType::Int));
    assert_eq!(nboot.default, Some(json!(100)));
    assert_eq!(nboot.min, Some(10.0));
    assert_eq!(nboot.max, Some(500.0));
    for (name, want) in [
        ("cluster_column", ParamType::String),
        ("species", ParamType::String),
        ("workers", ParamType::Int),
        ("db_scope", ParamType::String),
        ("normalized", ParamType::String),
    ] {
        assert!(
            matches!(manifest.nodes[0].params[name].r#type, t if t == want),
            "{name}"
        );
    }
}

#[test]
fn cellchat_params_render_through_env_into_the_baked_driver() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "cellchat_inference");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({
            "cluster_column": "celltype",
            "species": "mouse",
            "nboot": 200,
            "workers": 8,
            "db_scope": "secreted",
            "normalized": "lognorm",
        }),
    )
    .unwrap();
    assert_eq!(compiled.env.get("CCHAT_CLUSTER_COLUMN").unwrap(), "celltype");
    assert_eq!(compiled.env.get("CCHAT_SPECIES").unwrap(), "mouse");
    assert_eq!(compiled.env.get("CCHAT_NBOOT").unwrap(), "200");
    assert_eq!(compiled.env.get("CCHAT_WORKERS").unwrap(), "8");
    assert_eq!(compiled.env.get("CCHAT_DB_SCOPE").unwrap(), "secreted");
    assert_eq!(compiled.env.get("CCHAT_NORMALIZED").unwrap(), "lognorm");
}
