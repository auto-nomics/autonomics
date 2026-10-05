//! Golden parity tests for the magma plugin family: the `magma_annotate`
//! `[[nodes]]` entry must compile to the same `ContainerCommandSpec` the
//! legacy Rust wrapper (`nodes_io::magma_annotate_container`) produced.
//! The plugin directory lives outside this repository
//! (`/mnt/projects/node-plugins/magma` by default, overridable via
//! `NODE_PLUGINS_ROOT`); the test is skipped when absent so CI without
//! the plugin checkout stays green.

use std::path::PathBuf;

use container_plugin::compile::spec_compile::compile_container_spec;
use container_plugin::manifest::PluginManifest;
use serde_json::json;

fn plugin_root() -> Option<PathBuf> {
    let explicit = std::env::var_os("NODE_PLUGINS_ROOT");
    let root = explicit
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/projects/node-plugins"));
    let manifest = root.join("magma").join("manifest.toml");
    if manifest.is_file() {
        return Some(root);
    }
    if explicit.is_some() {
        // fix-review R04 (2026-10-05): an explicitly set NODE_PLUGINS_ROOT
        // means migration parity was requested. A missing family must fail
        // loudly — early-returns here used to count as *passed* tests, so a
        // green summary claimed coverage that never ran.
        panic!(
            "NODE_PLUGINS_ROOT is set but the magma family is not deployed \
             under it ({}) — migration parity cannot run. Deploy the family \
             or unset NODE_PLUGINS_ROOT to skip these tests deliberately.",
            manifest.display()
        );
    }
    eprintln!("skipping: magma plugin directory not present (NODE_PLUGINS_ROOT unset)");
    None
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let text = std::fs::read_to_string(root.join("magma").join("manifest.toml")).unwrap();
    // Inline script_file the way the loader does.
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        if let Some(relative) = node.command.script_file.clone() {
            let source = std::fs::read_to_string(root.join("magma").join(&relative)).unwrap();
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

/// The legacy wrapper's script, verbatim. It was fully static — no
/// Rust string building, no optional flags, no `decompress_gzip_inputs`
/// gzip stanza — so the plugin script is byte-identical to it. There are
/// no variable-name deltas to record (the ldsc `OUT_PREFIX`-style nuance
/// does not arise here).
const LEGACY_SCRIPT: &str = "set -eu\nmagma \\\n  --annotate \\\n  --snp-loc \"$AUTONOMICS_INPUT0\" \\\n  --gene-loc /panels/gene_loc/NCBI37.3.gene.loc \\\n  --out \"$AUTONOMICS_WORKDIR/magma_annotate\" \\\n  > \"$AUTONOMICS_OUTPUT0\" 2>&1";

#[test]
fn magma_annotate_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "magma_annotate");

    let compiled =
        compile_container_spec(node, &manifest.image, &manifest.panels, &json!({})).unwrap();

    assert_eq!(
        compiled.image,
        "ghcr.io/auto-nomics/autonomics/magma@sha256:2ca8540251ee9201f3b7b6ac2596daa2c95bd77eb314305dac61beb9b4d85342"
    );
    assert_eq!(compiled.outputs.len(), 2);
    assert_eq!(compiled.outputs[0].path, "magma_annotate.log");
    assert_eq!(compiled.outputs[0].format.as_deref(), Some("magma_log"));
    assert_eq!(compiled.outputs[1].path, "magma_annotate.genes.annot");
    assert_eq!(
        compiled.outputs[1].format.as_deref(),
        Some("magma_genes_annot")
    );
    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert!(matches!(
        compiled.pull_policy,
        container_runtime::PullPolicy::Missing
    ));
    // Deliberate delta: the legacy default was "/artifacts/magma_annotate_
    // container"; the plugin kind drops the `_container` suffix and the
    // prefix follows the kind (this is also the DSL-derived
    // `/artifacts/{kind}` default, asserted explicitly for honesty).
    assert_eq!(compiled.artifact_prefix, "/artifacts/magma_annotate");
    assert_eq!(compiled.timeout_secs, 300);
    assert_eq!(compiled.workdir, None);
    assert!(compiled.panels.is_empty());
    assert_eq!(compiled.panel_bundles.len(), 1);
    assert_eq!(
        compiled.panel_bundles[0].panel_id,
        "wjixiang/catalog-magma-gene-loc-ncbi37-3"
    );
    assert_eq!(compiled.panel_bundles[0].mount_path, "/panels/gene_loc");
    assert_eq!(compiled.command, vec!["sh".to_string()]);
    // The legacy wrapper set no env and had no user params; the plugin
    // script is static, so nothing travels through env.
    assert!(compiled.env.is_empty());

    // Semantic script markers: the load-bearing official CLI tokens.
    let script = compiled.script.as_deref().unwrap();
    assert!(script.contains("--annotate"));
    assert!(script.contains("--snp-loc \"$AUTONOMICS_INPUT0\""));
    assert!(script.contains("--gene-loc /panels/gene_loc/NCBI37.3.gene.loc"));
    assert!(script.contains("--out \"$AUTONOMICS_WORKDIR/magma_annotate\""));
    assert!(script.contains("> \"$AUTONOMICS_OUTPUT0\" 2>&1"));
    // No gzip `case` stanza on purpose: the legacy wrapper never called
    // decompress_gzip_inputs (MAGMA reads the SNP-location file verbatim).
    assert!(!script.contains("gzip"));
    // And because the legacy script was static, the parity is byte-exact
    // modulo the script file's trailing newline.
    assert_eq!(script.trim_end_matches('\n'), LEGACY_SCRIPT);
}

#[test]
fn magma_annotate_plugin_submitted_values_render_and_reject_unknown_params() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "magma_annotate");

    // The legacy spec exposed artifact_prefix/timeout_secs as per-instance
    // overridable fields with defaults; the plugin DSL bakes them as fixed
    // node-level manifest values, so the node has zero params: the only
    // accepted submitted value is the empty object, and nothing renders
    // into env.
    let compiled =
        compile_container_spec(node, &manifest.image, &manifest.panels, &json!({})).unwrap();
    assert!(compiled.env.is_empty());
    assert!(compiled.files.is_empty());

    // Submitting the legacy-overridable field now fails the same
    // additionalProperties:false gate the compiled schema enforces.
    let error = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({"artifact_prefix": "/artifacts/other"}),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unknown param `artifact_prefix`")
    );

    let error = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({"timeout_secs": 900}),
    )
    .unwrap_err();
    assert!(error.to_string().contains("unknown param `timeout_secs`"));
}
