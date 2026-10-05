//! Contract tests for the external clusterprofiler plugin. The checkout is
//! optional in CI, but parsing or validation failures are never skipped.

use std::path::PathBuf;

use container_plugin::compile::spec_compile::compile_container_spec;
use container_plugin::manifest::PluginManifest;
use container_plugin::node_definition::validate;
use serde_json::json;

fn plugin_root() -> Option<PathBuf> {
    let root = std::env::var_os("NODE_PLUGINS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/projects/node-plugins"));
    root.join("clusterprofiler")
        .join("manifest.toml")
        .is_file()
        .then_some(root)
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let directory = root.join("clusterprofiler");
    let text = std::fs::read_to_string(directory.join("manifest.toml")).unwrap();
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        validate(node).unwrap();
        if let Some(relative) = node.command.script_file.clone() {
            let source = std::fs::read_to_string(directory.join(&relative)).unwrap();
            node.command.script = Some(source);
            node.command.script_file = None;
        }
    }
    manifest
}

#[test]
fn clusterprofiler_ora_compiles_to_documented_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: clusterprofiler plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    assert_eq!(manifest.plugin_name, "clusterprofiler");
    assert!(manifest.panels.is_empty());
    assert_eq!(manifest.nodes.len(), 2);

    let node = &manifest.nodes[0];
    assert_eq!(node.kind, "clusterprofiler_ora");
    assert_eq!(node.ports.inputs.len(), 2);
    assert_eq!(node.ports.inputs[0].label.as_deref(), Some("gene_table"));
    assert_eq!(
        node.ports.inputs[0].accepted_formats,
        vec!["tsv".to_string()]
    );
    assert_eq!(node.ports.inputs[1].label.as_deref(), Some("gene_sets"));
    assert_eq!(
        node.ports.inputs[1].accepted_formats,
        vec!["gmt".to_string()]
    );
    assert_eq!(node.ports.outputs[0].path, "clusterprofiler_ora.tsv");
    assert_eq!(
        node.ports.outputs[0].format.as_deref(),
        Some("clusterprofiler_ora_tsv")
    );
    assert_eq!(node.ports.outputs[1].path, "clusterprofiler_ora.json");
    assert_eq!(
        node.ports.outputs[1].format.as_deref(),
        Some("clusterprofiler_ora_json")
    );

    let compiled = compile_container_spec(node, &manifest.image, &manifest.panels, &json!({}))
        .expect("default parameters must compile");
    assert_eq!(compiled.command, vec!["Rscript".to_string()]);
    assert!(
        compiled
            .script
            .as_deref()
            .unwrap()
            .contains("clusterProfiler::enricher")
    );
    for key in [
        "AUTONOMICS_GENE_COL",
        "AUTONOMICS_P_ADJUST_METHOD",
        "AUTONOMICS_PVALUE_CUTOFF",
        "AUTONOMICS_QVALUE_CUTOFF",
        "AUTONOMICS_MIN_SIZE",
        "AUTONOMICS_MAX_SIZE",
    ] {
        assert!(
            compiled.env.contains_key(key),
            "missing env template: {key}"
        );
    }
    assert_eq!(compiled.env["AUTONOMICS_GENE_COL"], "gene");
    assert_eq!(compiled.env["AUTONOMICS_P_ADJUST_METHOD"], "BH");
    assert_eq!(compiled.env["AUTONOMICS_PVALUE_CUTOFF"], "0.05");
    assert_eq!(compiled.env["AUTONOMICS_QVALUE_CUTOFF"], "0.2");
    assert_eq!(compiled.env["AUTONOMICS_MIN_SIZE"], "10");
    assert_eq!(compiled.env["AUTONOMICS_MAX_SIZE"], "500");
    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert_eq!(compiled.cpus, Some(2.0));
    assert_eq!(compiled.memory.as_deref(), Some("8Gi"));
    assert_eq!(compiled.pids_limit, Some(512));
    assert_eq!(compiled.shm_size.as_deref(), Some("1Gi"));
    assert_eq!(compiled.timeout_secs, 3600);
    assert_eq!(compiled.artifact_prefix, "/artifacts/clusterprofiler_ora");
}

#[test]
fn clusterprofiler_gsea_compiles_to_documented_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: clusterprofiler plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = manifest
        .nodes
        .iter()
        .find(|node| node.kind == "clusterprofiler_gsea")
        .expect("GSEA node must be registered");

    assert_eq!(node.ports.inputs[0].label.as_deref(), Some("rank_table"));
    assert_eq!(
        node.ports.inputs[0].accepted_formats,
        vec!["tsv".to_string()]
    );
    assert_eq!(node.ports.inputs[1].label.as_deref(), Some("gene_sets"));
    assert_eq!(
        node.ports.inputs[1].accepted_formats,
        vec!["gmt".to_string()]
    );
    assert_eq!(node.ports.outputs[0].path, "clusterprofiler_gsea.tsv");
    assert_eq!(
        node.ports.outputs[0].format.as_deref(),
        Some("clusterprofiler_gsea_tsv")
    );
    assert_eq!(node.ports.outputs[1].path, "clusterprofiler_gsea.json");
    assert_eq!(
        node.ports.outputs[1].format.as_deref(),
        Some("clusterprofiler_gsea_json")
    );

    let compiled = compile_container_spec(node, &manifest.image, &manifest.panels, &json!({}))
        .expect("default parameters must compile");
    assert_eq!(compiled.command, vec!["Rscript".to_string()]);
    assert!(
        compiled
            .script
            .as_deref()
            .unwrap()
            .contains("clusterProfiler::GSEA")
    );
    for key in [
        "AUTONOMICS_GENE_COL",
        "AUTONOMICS_SCORE_COL",
        "AUTONOMICS_P_ADJUST_METHOD",
        "AUTONOMICS_PVALUE_CUTOFF",
        "AUTONOMICS_MIN_SIZE",
        "AUTONOMICS_MAX_SIZE",
        "AUTONOMICS_EPS",
        "AUTONOMICS_SEED",
    ] {
        assert!(
            compiled.env.contains_key(key),
            "missing env template: {key}"
        );
    }
    assert_eq!(compiled.env["AUTONOMICS_SCORE_COL"], "score");
    assert_eq!(compiled.env["AUTONOMICS_PVALUE_CUTOFF"], "0.05");
    assert_eq!(compiled.env["AUTONOMICS_EPS"], "1e-10");
    assert_eq!(compiled.env["AUTONOMICS_SEED"], "true");
    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert_eq!(compiled.cpus, Some(2.0));
    assert_eq!(compiled.memory.as_deref(), Some("8Gi"));
    assert_eq!(compiled.pids_limit, Some(512));
    assert_eq!(compiled.shm_size.as_deref(), Some("1Gi"));
    assert_eq!(compiled.timeout_secs, 3600);
    assert_eq!(compiled.artifact_prefix, "/artifacts/clusterprofiler_gsea");
}
