//! Contract lock for the nichenetr plugin family (tier-1 official wrap):
//! official nichenetr (saeyslab/nichenetr, GPL-3-only) pinned to tag
//! v2.2.0 commit f9906f3a inside a digest-pinned image, with the human
//! prior-knowledge triple (Zenodo record 7074291, MD5-verified at build)
//! baked alongside — the image digest pins algorithm AND priors. The
//! plugin directory lives outside this repository
//! (`/mnt/projects/node-plugins` by default, overridable via
//! `NODE_PLUGINS_ROOT`); the test is skipped when absent so CI without
//! the plugin checkout stays green.

use std::path::PathBuf;

use container_plugin::compile::spec_compile::compile_container_spec;
use container_plugin::manifest::PluginManifest;
use container_plugin::node_definition::ParamType;
use serde_json::json;

/// Registry digest of the pushed image (NOT the local-build digest —
/// podman converts the manifest format on push; the registry digest is
/// authoritative and pins algorithm + baked priors together).
const IMAGE: &str = "ghcr.io/zj-2002/nichenetr@sha256:f0ee4a511531f91839d2d9d9cb83535bd7f7f6b12905e26635684f8a081f0c19";

fn plugin_root() -> Option<PathBuf> {
    let explicit = std::env::var_os("NODE_PLUGINS_ROOT");
    let root = explicit
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/projects/node-plugins"));
    let manifest = root.join("nichenetr").join("manifest.toml");
    if manifest.is_file() {
        return Some(root);
    }
    if explicit.is_some() {
        // R04 hygiene: an explicitly set NODE_PLUGINS_ROOT means the
        // contract lock was requested; a missing family must fail loudly.
        panic!(
            "NODE_PLUGINS_ROOT is set but the nichenetr family is not \
             deployed under it ({}) — the contract lock cannot run. Deploy \
             the family or unset NODE_PLUGINS_ROOT to skip deliberately.",
            manifest.display()
        );
    }
    eprintln!("skipping: nichenetr plugin directory not present (NODE_PLUGINS_ROOT unset)");
    None
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let text = std::fs::read_to_string(root.join("nichenetr").join("manifest.toml")).unwrap();
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        if let Some(relative) = node.command.script_file.clone() {
            let source = std::fs::read_to_string(root.join("nichenetr").join(&relative)).unwrap();
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
fn nichenetr_ligand_activity_compiles_to_the_official_wrap_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    assert_eq!(manifest.plugin_name, "nichenetr");
    let node = node_by_kind(&manifest, "nichenetr_ligand_activity");

    // sender/receiver are required params — supply them; every other
    // knob exercises its frozen official default.
    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({"sender": "Macrophages", "receiver": "T_cells"}),
    )
    .unwrap();

    // Tier-1 identity: the digest pins the official package AND the
    // baked human prior triple (ligand_target_matrix, weighted_networks,
    // lr_network — Zenodo 7074291, MD5-verified at build).
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

    // Two required inputs: expression H5AD (port 0) and the
    // geneset-of-interest file (port 1) — the frozen manual pipeline
    // takes the geneset as data, not a derived quantity.
    assert_eq!(node.ports.inputs.len(), 2);
    assert_eq!(node.ports.inputs[0].label.as_deref(), Some("exprs_h5ad"));
    assert_eq!(node.ports.inputs[1].label.as_deref(), Some("geneset"));

    // Stable 5-file output contract.
    assert_eq!(compiled.outputs.len(), 5);
    let contract: Vec<(&str, &str)> = compiled
        .outputs
        .iter()
        .map(|o| (o.path.as_str(), o.format.as_deref().unwrap_or("")))
        .collect();
    assert_eq!(
        contract,
        vec![
            ("nichenetr_ligand_activities.csv", "nichenetr_table"),
            ("nichenetr_ligand_target_links.csv", "nichenetr_table"),
            ("nichenetr_ligand_receptor_links.csv", "nichenetr_table"),
            ("nichenetr_top_ligands.csv", "nichenetr_table"),
            ("nichenetr_provenance.json", "nichenetr_provenance_json"),
        ]
    );

    // Priors are baked: the node never needs egress.
    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert!(matches!(
        compiled.pull_policy,
        container_runtime::PullPolicy::Missing
    ));
    assert_eq!(compiled.timeout_secs, 14400);
    assert_eq!(
        compiled.artifact_prefix,
        "/artifacts/nichenetr_ligand_activity"
    );
    assert_eq!(compiled.workdir, None);
    assert_eq!(compiled.cpus, Some(4.0));
    assert_eq!(compiled.memory.as_deref(), Some("16Gi"));
    assert_eq!(compiled.pids_limit, Some(512));
    assert_eq!(compiled.shm_size.as_deref(), Some("1Gi"));

    // sender/receiver are REQUIRED params (no default): the official
    // prioritization is directional by construction.
    let sender = &manifest.nodes[0].params["sender"];
    assert!(matches!(sender.r#type, ParamType::String));
    assert!(sender.default.is_none());
    assert!(!sender.optional);
    let receiver = &manifest.nodes[0].params["receiver"];
    assert!(matches!(receiver.r#type, ParamType::String));
    assert!(receiver.default.is_none());
    assert!(!receiver.optional);
    assert_eq!(manifest.nodes[0].params.len(), 6);

    // Frozen official defaults render through NNET_* env; no bool/flag
    // params by design (base parser constraint + no natural knobs).
    // Required params absent from the submission: compile with just
    // sender/receiver supplied.
    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({"sender": "Fibroblasts", "receiver": "T_cells"}),
    )
    .unwrap();
    assert_eq!(compiled.env.get("NNET_SENDER").unwrap(), "Fibroblasts");
    assert_eq!(compiled.env.get("NNET_RECEIVER").unwrap(), "T_cells");
    assert_eq!(compiled.env.get("NNET_CLUSTER_COLUMN").unwrap(), "cluster");
    assert_eq!(compiled.env.get("NNET_TOP_N_LIGANDS").unwrap(), "30");
    assert_eq!(compiled.env.get("NNET_N_TARGETS").unwrap(), "200");
    assert_eq!(compiled.env.get("NNET_PCT").unwrap(), "0.1");

    // Frozen-convention bounds: top 30 ligands / n=200 targets are the
    // official protocol values, exposed with caps; pct floored at 0.01.
    let top_n = &manifest.nodes[0].params["top_n_ligands"];
    assert!(matches!(top_n.r#type, ParamType::Int));
    assert_eq!(top_n.default, Some(json!(30)));
    assert_eq!(top_n.min, Some(1.0));
    assert_eq!(top_n.max, Some(200.0));
    let n_targets = &manifest.nodes[0].params["n_targets"];
    assert!(matches!(n_targets.r#type, ParamType::Int));
    assert_eq!(n_targets.default, Some(json!(200)));
    assert_eq!(n_targets.min, Some(1.0));
    assert_eq!(n_targets.max, Some(1000.0));
    let pct = &manifest.nodes[0].params["pct"];
    assert!(matches!(pct.r#type, ParamType::Number));
    assert_eq!(pct.default, Some(json!(0.1)));
    assert_eq!(pct.min, Some(0.01));
    assert_eq!(pct.max, Some(1.0));
}
