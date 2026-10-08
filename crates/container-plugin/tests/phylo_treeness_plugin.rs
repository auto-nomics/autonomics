//! Contract tests for the external phylo-treeness plugin. The checkout is
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
    root.join("phylo-treeness")
        .join("manifest.toml")
        .is_file()
        .then_some(root)
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let directory = root.join("phylo-treeness");
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
fn phylo_scogs_treeness_compiles_to_documented_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: phylo-treeness plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    assert_eq!(manifest.plugin_name, "phylo-treeness");
    assert!(manifest.panels.is_empty());
    assert_eq!(manifest.nodes.len(), 1);

    // The executable reference must be a real digest, not the all-zero
    // placeholder rejected by the loader.
    let reference = manifest.image.reference.to_string();
    assert!(reference.starts_with("ghcr.io/auto-nomics/autonomics/phylo-treeness@sha256:"));
    let digest = reference
        .rsplit('@')
        .next()
        .unwrap()
        .trim_start_matches("sha256:");
    assert_eq!(digest.len(), 64);
    assert!(digest.bytes().any(|b| b != b'0'));

    let node = &manifest.nodes[0];
    assert_eq!(node.kind, "phylo_scogs_treeness");
    assert_eq!(node.ports.inputs.len(), 1);
    assert_eq!(node.ports.inputs[0].label.as_deref(), Some("scogs_genes"));
    assert_eq!(
        node.ports.inputs[0].accepted_formats,
        vec!["tar".to_string()]
    );
    assert_eq!(node.ports.outputs.len(), 2);
    assert_eq!(node.ports.outputs[0].path, "treeness.tsv");
    assert_eq!(
        node.ports.outputs[0].format.as_deref(),
        Some("phylo_scogs_treeness_tsv")
    );
    assert_eq!(node.ports.outputs[1].path, "treeness_summary.json");
    assert_eq!(
        node.ports.outputs[1].format.as_deref(),
        Some("phylo_scogs_treeness_json")
    );

    let compiled = compile_container_spec(node, &manifest.image, &manifest.panels, &json!({}))
        .expect("default parameters must compile");
    assert_eq!(compiled.command, vec!["bash".to_string()]);
    assert!(
        compiled
            .script
            .as_deref()
            .unwrap()
            .contains("phykit treeness")
    );
    assert!(compiled.script.as_deref().unwrap().contains("mafft --auto"));
    assert!(compiled.script.as_deref().unwrap().contains("clipkit"));
    assert!(compiled.script.as_deref().unwrap().contains("iqtree -s"));
    for key in [
        "AUTONOMICS_THREADS",
        "AUTONOMICS_MODEL",
        "AUTONOMICS_MIN_TAXA",
        "AUTONOMICS_SEED",
    ] {
        assert!(
            compiled.env.contains_key(key),
            "missing env template: {key}"
        );
    }
    assert_eq!(compiled.env["AUTONOMICS_THREADS"], "2");
    assert_eq!(compiled.env["AUTONOMICS_MODEL"], "TEST");
    assert_eq!(compiled.env["AUTONOMICS_MIN_TAXA"], "3");
    assert_eq!(compiled.env["AUTONOMICS_SEED"], "12345");
    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert_eq!(compiled.cpus, Some(2.0));
    assert_eq!(compiled.memory.as_deref(), Some("8Gi"));
    assert_eq!(compiled.pids_limit, Some(512));
    assert_eq!(compiled.shm_size.as_deref(), Some("1Gi"));
    assert_eq!(compiled.timeout_secs, 14400);
    assert_eq!(
        compiled.artifact_prefix,
        Some("/artifacts/phylo_scogs_treeness".to_string())
    );

    // Explicit parameter values render into the env template.
    let explicit = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({"threads": 4, "model": "LG+G4", "min_taxa": 4, "seed": 7}),
    )
    .expect("explicit parameters must compile");
    assert_eq!(explicit.env["AUTONOMICS_THREADS"], "4");
    assert_eq!(explicit.env["AUTONOMICS_MODEL"], "LG+G4");
    assert_eq!(explicit.env["AUTONOMICS_MIN_TAXA"], "4");
    assert_eq!(explicit.env["AUTONOMICS_SEED"], "7");
}

#[test]
fn phylo_scogs_treeness_rejects_invalid_parameters() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: phylo-treeness plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = &manifest.nodes[0];

    let cases: Vec<(&str, serde_json::Value)> = vec![
        ("unknown parameter", json!({"bogus": 1})),
        ("threads below minimum", json!({"threads": 0})),
        ("threads above maximum", json!({"threads": 17})),
        ("threads wrong type", json!({"threads": "two"})),
        ("min_taxa below minimum", json!({"min_taxa": 1})),
        ("seed negative", json!({"seed": -1})),
    ];
    // A blank `model` string is not expressible in the manifest DSL; the
    // execution script rejects it at runtime (authoring guide §4).
    for (label, params) in cases {
        let outcome = compile_container_spec(node, &manifest.image, &manifest.panels, &params);
        assert!(outcome.is_err(), "expected {label} to be rejected");
    }
}
