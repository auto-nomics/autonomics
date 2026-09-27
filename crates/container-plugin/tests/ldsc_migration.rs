//! Golden parity tests for the ldsc plugin family: each `[[nodes]]` entry
//! must compile to the same `Container` spec the legacy Rust wrapper
//! produced. The plugin directory lives outside this repository
//! (`/mnt/projects/node-plugins/ldsc` by default, overridable via
//! `NODE_PLUGINS_ROOT`); the test is skipped when absent so CI without
//! the plugin checkout stays green.

use std::path::PathBuf;

use container_plugin::compile::spec_compile::compile_container_spec;
use container_plugin::manifest::PluginManifest;
use serde_json::json;

fn plugin_root() -> Option<PathBuf> {
    let root = std::env::var_os("NODE_PLUGINS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/projects/node-plugins"));
    let manifest = root.join("ldsc").join("manifest.toml");
    manifest.is_file().then_some(root)
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let text = std::fs::read_to_string(root.join("ldsc").join("manifest.toml")).unwrap();
    // Inline script_file the way the loader does.
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        if let Some(relative) = node.command.script_file.clone() {
            let source = std::fs::read_to_string(root.join("ldsc").join(&relative)).unwrap();
            node.command.script = Some(source);
            node.command.script_file = None;
        }
    }
    manifest
}

fn node_by_kind<'a>(manifest: &'a PluginManifest, kind: &str) -> &'a container_plugin::node_definition::NodeDefinition {
    manifest.nodes.iter()
        .find(|n| n.kind == kind)
        .unwrap_or_else(|| panic!("{kind} missing from manifest"))
}

#[test]
fn ldsc_h2_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "ldsc_h2");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({}),
    )
    .unwrap();

    assert_eq!(
        compiled.image,
        "ghcr.io/auto-nomics/autonomics/ldsc@sha256:2dad70a9583f93db1dcc9a560b7d5b309af4a5151dfaf615f80d059a0925d78c"
    );
    assert_eq!(compiled.outputs.len(), 1);
    assert_eq!(compiled.outputs[0].path, "ldsc_h2.log");
    assert_eq!(compiled.outputs[0].format.as_deref(), Some("ldsc_log"));
    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert!(matches!(
        compiled.pull_policy,
        container_runtime::PullPolicy::Missing
    ));
    assert_eq!(compiled.timeout_secs, 900);
    assert_eq!(compiled.artifact_prefix, "/artifacts/ldsc_h2");
    assert_eq!(compiled.workdir, None);
    assert!(compiled.panels.is_empty());
    let panel_ids: Vec<&str> = compiled
        .panel_bundles
        .iter()
        .map(|p| p.panel_id.as_str())
        .collect();
    assert_eq!(
        panel_ids,
        vec![
            "wjixiang/catalog-ldsc-ref-ld-1000g-eur-basic",
            "wjixiang/catalog-ldsc-w-ld-1000g-eur-hm3-no-mhc"
        ]
    );
    assert_eq!(compiled.command, vec!["sh".to_string()]);
    assert_eq!(compiled.env.get("LDSC_N_BLOCKS").unwrap(), "200");
    assert_eq!(compiled.env.get("LDSC_INTERCEPT_H2").unwrap(), "");
    assert_eq!(compiled.env.get("LDSC_CHISQ_MAX").unwrap(), "");
}

#[test]
fn ldsc_h2_plugin_renders_submitted_values_into_env() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "ldsc_h2");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({"n_blocks": 100, "intercept_h2": 1.2, "chisq_max": 30.0}),
    )
    .unwrap();

    assert_eq!(compiled.env.get("LDSC_N_BLOCKS").unwrap(), "100");
    // serde_json renders f64 30.0 as "30.0".
    assert_eq!(compiled.env.get("LDSC_CHISQ_MAX").unwrap(), "30.0");
}

#[test]
fn ldsc_h2_plugin_schema_marks_optionals_not_required() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "ldsc_h2");

    let schema =
        serde_json::to_value(container_plugin::compile::compile_schema(&node.params)).unwrap();
    let required = schema["required"].as_array().unwrap();
    assert!(required.is_empty());
    assert_eq!(schema["properties"]["n_blocks"]["type"], "integer");
    assert_eq!(schema["properties"]["n_blocks"]["minimum"], 2.0);
}

#[test]
fn ldsc_munge_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "ldsc_munge");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({}),
    )
    .unwrap();

    assert_eq!(
        compiled.image,
        "ghcr.io/auto-nomics/autonomics/ldsc@sha256:2dad70a9583f93db1dcc9a560b7d5b309af4a5151dfaf615f80d059a0925d78c"
    );
    // Munge itself does not require the LD-score panels, but the legacy
    // wrapper attaches them via the family-wide panel bundle spec. The
    // plugin does the same (family panels propagate to every node); the
    // legacy contract is preserved for binary parity.
    assert_eq!(compiled.panel_bundles.len(), 2);
    assert_eq!(compiled.outputs.len(), 2);
    assert_eq!(compiled.outputs[0].path, "munged.sumstats.gz");
    assert_eq!(compiled.outputs[0].format.as_deref(), Some("sumstats_gz"));
    assert_eq!(compiled.outputs[1].path, "munge_sumstats.log");
    assert_eq!(compiled.outputs[1].format.as_deref(), Some("ldsc_log"));

    let script = compiled.script.as_deref().unwrap();
        // munge.sh does its own gzip handling in a sh case statement, so the
    // legacy "prepare_input" helper is not present. The plugin's
    // equivalent markers are the .gz branch and the gzip -dc command.
    assert!(script.contains("*.gz"));
    assert!(script.contains("gzip -dc"));
    assert!(script.contains("munge_sumstats"));
    // Scripts use OUT_PREFIX (the plugin's variable name); legacy used
    // out_prefix. Both are shell variable literals and parse identically;
    // this is the documented script-variable-name parity nuance.
    assert!(script.contains("mv \"$OUT_PREFIX.sumstats.gz\" \"$AUTONOMICS_OUTPUT0\""));
    assert!(script.contains("cat \"$OUT_PREFIX.log\" >> \"$AUTONOMICS_OUTPUT1\""));

    assert_eq!(compiled.env.get("LDSC_MUNGE_INFO_MIN").unwrap(), "0.9");
    assert_eq!(compiled.env.get("LDSC_MUNGE_MAF_MIN").unwrap(), "0.01");
    assert_eq!(compiled.env.get("LDSC_MUNGE_DANER").unwrap(), "");
    assert_eq!(compiled.env.get("LDSC_MUNGE_SNP").unwrap(), "");
    assert_eq!(compiled.env.get("LDSC_MUNGE_N_COL").unwrap(), "");
}

#[test]
fn ldsc_rg_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "ldsc_rg");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({}),
    )
    .unwrap();

    assert_eq!(compiled.panel_bundles.len(), 2);
    let panel_ids: Vec<&str> = compiled
        .panel_bundles
        .iter()
        .map(|p| p.panel_id.as_str())
        .collect();
    assert_eq!(
        panel_ids,
        vec![
            "wjixiang/catalog-ldsc-ref-ld-1000g-eur-basic",
            "wjixiang/catalog-ldsc-w-ld-1000g-eur-hm3-no-mhc"
        ]
    );
    assert_eq!(compiled.outputs.len(), 1);
    assert_eq!(compiled.outputs[0].path, "ldsc_rg.log");

    let script = compiled.script.as_deref().unwrap();
    assert!(script.contains("AUTONOMICS_INPUT0"));
    assert!(script.contains("AUTONOMICS_INPUT1"));

    assert!(script.contains("--rg \"$AUTONOMICS_INPUT0\",\"$AUTONOMICS_INPUT1\""));
    assert!(script.contains("--ref-ld-chr /panels/ref_ld/LDscore."));
    assert!(script.contains("--w-ld-chr /panels/w_ld/weights.hm3_noMHC."));

    assert_eq!(compiled.env.get("LDSC_RG_N_BLOCKS").unwrap(), "200");
    assert_eq!(compiled.env.get("LDSC_RG_CHISQ_MAX").unwrap(), "");
}