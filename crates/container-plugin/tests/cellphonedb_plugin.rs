//! Contract lock for the cellphonedb plugin family (tier-1 official wrap):
//! pip-pinned official CellPhoneDB 5.0.1 + curated database v5.0.0 baked
//! into a digest-pinned image; the driver does I/O translation only. The
//! plugin directory lives outside this repository
//! (`/mnt/projects/node-plugins` by default, overridable via
//! `NODE_PLUGINS_ROOT`); the test is skipped when absent so CI without the
//! plugin checkout stays green.

use std::path::PathBuf;

use container_plugin::compile::spec_compile::compile_container_spec;
use container_plugin::manifest::PluginManifest;
use container_plugin::node_definition::ParamType;
use serde_json::json;

/// Digest of ghcr.io/zj-2002/cellphonedb:5.0.1-db5.0.0 as pushed — the
/// registry manifest digest (NOT the local-build digest: podman converts
/// the manifest format during push). Pins algorithm AND database together.
const IMAGE: &str =
    "ghcr.io/zj-2002/cellphonedb@sha256:997126a941d1db4d9f98231a97901611915276e140ad2bf183fbfae848c08347";

fn plugin_root() -> Option<PathBuf> {
    let explicit = std::env::var_os("NODE_PLUGINS_ROOT");
    let root = explicit
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/projects/node-plugins"));
    let manifest = root.join("cellphonedb").join("manifest.toml");
    if manifest.is_file() {
        return Some(root);
    }
    if explicit.is_some() {
        // Same hygiene rule as the migration tests (fix-review R04,
        // 2026-10-05): an explicitly set NODE_PLUGINS_ROOT means the
        // contract lock was requested; a missing family must fail loudly
        // instead of counting as passed.
        panic!(
            "NODE_PLUGINS_ROOT is set but the cellphonedb family is not \
             deployed under it ({}) — the contract lock cannot run. Deploy \
             the family or unset NODE_PLUGINS_ROOT to skip deliberately.",
            manifest.display()
        );
    }
    eprintln!("skipping: cellphonedb plugin directory not present (NODE_PLUGINS_ROOT unset)");
    None
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let text = std::fs::read_to_string(root.join("cellphonedb").join("manifest.toml")).unwrap();
    // Inline script_file the way the loader does (this family is
    // baked-driver style — argv only — but keep the loader parity).
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        if let Some(relative) = node.command.script_file.clone() {
            let source = std::fs::read_to_string(root.join("cellphonedb").join(&relative)).unwrap();
            node.command.script = Some(source);
            node.command.script_file = None;
        }
    }
    manifest
}

#[test]
fn cellphonedb_statistical_analysis_compiles_to_the_official_wrap_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    assert_eq!(manifest.plugin_name, "cellphonedb");
    let node = manifest
        .nodes
        .iter()
        .find(|n| n.kind == "cellphonedb_statistical_analysis")
        .unwrap_or_else(|| panic!("cellphonedb_statistical_analysis missing from manifest"));

    let compiled =
        compile_container_spec(node, &manifest.image, &manifest.panels, &json!({})).unwrap();

    // The tier-1 identity: the digest pins the official package AND the
    // baked curated database together.
    assert_eq!(compiled.image, IMAGE);

    // Baked-runner style: the image's own driver, no injected script.
    assert_eq!(
        compiled.command,
        vec![
            "python".to_string(),
            "/opt/autonomics/driver.py".to_string()
        ]
    );
    assert_eq!(compiled.script, None);

    // Single required input (raw-count H5AD; PortSpec has no optional
    // inputs, so the single-port design is the deliberate v1 shape);
    // outputs are the official tables renamed from their timestamped
    // names to a stable contract, plus the provenance record. No panel
    // bundles, no mounted files. (The compiled spec carries no inputs
    // field — inputs are asserted at the manifest level.)
    assert_eq!(node.ports.inputs.len(), 1);
    assert_eq!(node.ports.inputs[0].label.as_deref(), Some("counts_h5ad"));
    assert!(compiled.panels.is_empty());
    assert!(compiled.files.is_empty());
    assert_eq!(compiled.panel_bundles.len(), 0);
    assert_eq!(compiled.outputs.len(), 7);
    let contract: Vec<(&str, &str)> = compiled
        .outputs
        .iter()
        .map(|o| (o.path.as_str(), o.format.as_deref().unwrap_or("")))
        .collect();
    assert_eq!(
        contract,
        vec![
            ("cpdb_pvalues.txt", "cpdb_table"),
            ("cpdb_means.txt", "cpdb_table"),
            ("cpdb_significant_means.txt", "cpdb_table"),
            ("cpdb_deconvoluted.txt", "cpdb_table"),
            ("cpdb_deconvoluted_percents.txt", "cpdb_table"),
            ("cpdb_interaction_scores.txt", "cpdb_table"),
            ("cpdb_provenance.json", "cpdb_provenance_json"),
        ]
    );

    // Network-isolated by design: the database is baked in, so the node
    // never needs egress (verified offline with --network none).
    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert!(matches!(
        compiled.pull_policy,
        container_runtime::PullPolicy::Missing
    ));
    assert_eq!(compiled.timeout_secs, 14400);
    assert_eq!(
        compiled.artifact_prefix,
        "/artifacts/cellphonedb_statistical_analysis"
    );
    assert_eq!(compiled.workdir, None);
    assert_eq!(compiled.cpus, Some(4.0));
    assert_eq!(compiled.memory.as_deref(), Some("16Gi"));
    assert_eq!(compiled.pids_limit, Some(512));
    assert_eq!(compiled.shm_size.as_deref(), Some("2Gi"));

    // All knobs flow through env into the baked driver (CPDB_*); defaults
    // mirror the official method defaults. The Int default renders "1000"
    // and the Number default "0.1" (serde_json spelling).
    assert_eq!(compiled.env.len(), 4);
    assert_eq!(compiled.env.get("CPDB_CLUSTER_COLUMN").unwrap(), "cluster");
    assert_eq!(compiled.env.get("CPDB_COUNTS_DATA").unwrap(), "hgnc_symbol");
    assert_eq!(compiled.env.get("CPDB_ITERATIONS").unwrap(), "1000");
    assert_eq!(compiled.env.get("CPDB_THRESHOLD").unwrap(), "0.1");

    // Param schema: typed bounds — iterations is a floored int (>= 100),
    // threshold is a fraction in [0, 1], no flag/bool params (the v1
    // contract hardcodes score_interactions=true in the driver, so no
    // conditional output ports).
    assert_eq!(manifest.nodes[0].params.len(), 4);
    let iterations = &manifest.nodes[0].params["iterations"];
    assert!(matches!(iterations.r#type, ParamType::Int));
    assert_eq!(iterations.default, Some(json!(1000)));
    assert_eq!(iterations.min, Some(100.0));
    let threshold = &manifest.nodes[0].params["threshold"];
    assert!(matches!(threshold.r#type, ParamType::Number));
    assert_eq!(threshold.default, Some(json!(0.1)));
    assert_eq!(threshold.min, Some(0.0));
    assert_eq!(threshold.max, Some(1.0));
    let cluster_column = &manifest.nodes[0].params["cluster_column"];
    assert!(matches!(cluster_column.r#type, ParamType::String));
    assert_eq!(cluster_column.default, Some(json!("cluster")));
    let counts_data = &manifest.nodes[0].params["counts_data"];
    assert!(matches!(counts_data.r#type, ParamType::String));
    assert_eq!(counts_data.default, Some(json!("hgnc_symbol")));
}

#[test]
fn cellphonedb_params_render_through_env_into_the_baked_driver() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = manifest
        .nodes
        .iter()
        .find(|n| n.kind == "cellphonedb_statistical_analysis")
        .unwrap();

    // Overrides render through the same env channel; ints stay integer-
    // spelled, numbers keep their float spelling.
    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({
            "cluster_column": "celltype",
            "counts_data": "ensembl",
            "iterations": 500,
            "threshold": 0.25,
        }),
    )
    .unwrap();
    assert_eq!(compiled.env.get("CPDB_CLUSTER_COLUMN").unwrap(), "celltype");
    assert_eq!(compiled.env.get("CPDB_COUNTS_DATA").unwrap(), "ensembl");
    assert_eq!(compiled.env.get("CPDB_ITERATIONS").unwrap(), "500");
    assert_eq!(compiled.env.get("CPDB_THRESHOLD").unwrap(), "0.25");
}
