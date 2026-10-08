//! Golden parity tests for the plink2 plugin family: the `plink2_clump`
//! `[[nodes]]` entry must compile to the same `ContainerCommandSpec` the
//! legacy Rust wrapper (`nodes_io::plink2_clump_container`) produced.
//! The plugin directory lives outside this repository
//! (`/mnt/projects/node-plugins/plink2` by default, overridable via
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
    let manifest = root.join("plink2").join("manifest.toml");
    if manifest.is_file() {
        return Some(root);
    }
    if explicit.is_some() {
        // fix-review R04 (2026-10-05): an explicitly set NODE_PLUGINS_ROOT
        // means migration parity was requested. A missing family must fail
        // loudly — early-returns here used to count as *passed* tests, so a
        // green summary claimed coverage that never ran.
        panic!(
            "NODE_PLUGINS_ROOT is set but the plink2 family is not deployed \
             under it ({}) — migration parity cannot run. Deploy the family \
             or unset NODE_PLUGINS_ROOT to skip these tests deliberately.",
            manifest.display()
        );
    }
    eprintln!("skipping: plink2 plugin directory not present (NODE_PLUGINS_ROOT unset)");
    None
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let text = std::fs::read_to_string(root.join("plink2").join("manifest.toml")).unwrap();
    // Inline script_file the way the loader does.
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        if let Some(relative) = node.command.script_file.clone() {
            let source = std::fs::read_to_string(root.join("plink2").join(&relative)).unwrap();
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
fn plink2_clump_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "plink2_clump");

    let compiled =
        compile_container_spec(node, &manifest.image, &manifest.panels, &json!({})).unwrap();

    // Byte-exact contract: image, outputs, panels, resources, timeout.
    assert_eq!(
        compiled.image,
        "ghcr.io/auto-nomics/autonomics/plink2@sha256:998315bf1c34c1c7ef276e93ca6c043dd1982c027c7264325505a9a0adc6d1d2"
    );
    assert_eq!(compiled.outputs.len(), 3);
    assert_eq!(compiled.outputs[0].path, "plink2_clump.log");
    assert_eq!(
        compiled.outputs[0].format.as_deref(),
        Some("plink2_clump_log")
    );
    assert_eq!(compiled.outputs[1].path, "plink2_clump.clumps");
    assert_eq!(compiled.outputs[1].format.as_deref(), Some("plink2_clumps"));
    assert_eq!(compiled.outputs[2].path, "plink2_clump.chromosomes.tsv");
    assert_eq!(
        compiled.outputs[2].format.as_deref(),
        Some("plink2_chromosomes")
    );
    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert!(matches!(
        compiled.pull_policy,
        container_runtime::PullPolicy::Missing
    ));
    assert_eq!(compiled.timeout_secs, 1800);
    // Deliberate delta: the legacy default was "/artifacts/plink2_clump_
    // container"; the plugin kind drops the `_container` suffix and the
    // prefix follows the kind (the wave-wide `/artifacts/{kind}` convention,
    // same as ldsc/magma/coloc/mvmr/mrpresso).
    assert_eq!(
        compiled.artifact_prefix,
        Some("/artifacts/plink2_clump".to_string())
    );
    assert_eq!(compiled.workdir, None);
    assert!(compiled.panels.is_empty());
    assert_eq!(compiled.panel_bundles.len(), 1);
    assert_eq!(
        compiled.panel_bundles[0].panel_id,
        "wjixiang/catalog-plink-ref-1000g-eur-binary"
    );
    assert_eq!(compiled.panel_bundles[0].mount_path, "/panels/plink_ref");
    // Deliberate delta: the legacy command was vec!["sh", "-c"]; the manifest
    // carries the interpreter alone and the executor inserts the materialized
    // script at index 1 (the trailing "-c" was a no-op positional under
    // `sh <script>`).
    assert_eq!(compiled.command, vec!["sh".to_string()]);

    // Env defaults render exactly the tokens the legacy `format!` script
    // baked in (`{:.0e}` for the p thresholds, `{}` for r2/kb).
    assert_eq!(compiled.env.get("PLINK2_CLUMP_P1").unwrap(), "5e-8");
    assert_eq!(compiled.env.get("PLINK2_CLUMP_P2").unwrap(), "1e-6");
    assert_eq!(compiled.env.get("PLINK2_CLUMP_R2").unwrap(), "0.001");
    assert_eq!(compiled.env.get("PLINK2_CLUMP_KB").unwrap(), "10000");
    // Optional `chr` renders empty: the script falls back to autosomes 1..=22.
    assert_eq!(compiled.env.get("PLINK2_CLUMP_CHR").unwrap(), "");
    assert_eq!(
        compiled.env.get("PLINK2_CLUMP_INPUT_FORMAT").unwrap(),
        "twosamplemr"
    );

    // Semantic script markers (not byte equality): the plugin drives flags
    // through env and its own `if`/`case` stanzas instead of Rust
    // string-building, so layout differs from the legacy generated script.
    // Variable names that matter are kept identical (out_prefix, chr).
    let script = compiled.script.as_deref().unwrap();
    assert!(script.contains("--bfile /panels/plink_ref/1000G.EUR.QC.${chr}"));
    assert!(script.contains("--clump cols=+chrom,+pos \"${AUTONOMICS_INPUT0}\""));
    assert!(script.contains("--clump-id-field SNP --clump-p-field P"));
    assert!(script.contains("--clump-p1 \"${PLINK2_CLUMP_P1}\""));
    assert!(script.contains("--clump-p2 \"${PLINK2_CLUMP_P2}\""));
    assert!(script.contains("--clump-r2 \"${PLINK2_CLUMP_R2}\""));
    assert!(script.contains("--clump-kb \"${PLINK2_CLUMP_KB}\""));
    assert!(script.contains("seq $CHR_SEQ"));
    assert!(script.contains("mkdir -p /work/per_chr"));
    assert!(script.contains("sed \"s/^/${chr}\\t/\""));
    assert!(script.contains("printf 'chr\\t%s\\n' \"${chr}\""));
    assert!(script.contains(">> \"${AUTONOMICS_OUTPUT0}\" 2>&1"));
    assert!(script.contains(">> \"${AUTONOMICS_OUTPUT1}\""));
    assert!(script.contains(">> \"${AUTONOMICS_OUTPUT2}\""));
    // Per-chromosome failure must surface non-zero, as in the legacy script.
    assert!(script.contains("plink2 clump failed on chromosome ${chr}"));
    // The optional single-chromosome branch is static script logic keyed on
    // the possibly-empty env value.
    assert!(script.contains("if [ -n \"$PLINK2_CLUMP_CHR\" ]; then"));
    assert!(script.contains("--chr $PLINK2_CLUMP_CHR"));
    // No gzip `case` stanza on purpose: the legacy wrapper never called
    // decompress_gzip_inputs (the sumstats table is read verbatim).
    assert!(!script.contains("gzip"));
}

#[test]
fn plink2_clump_plugin_renders_submitted_values_into_env() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "plink2_clump");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({
            "clump_p1": 1e-7,
            "clump_p2": 2e-6,
            "clump_r2": 0.05,
            "clump_kb": 25000,
            "chr": 6,
            "input_format": "ieu_open_gwas",
        }),
    )
    .unwrap();

    // Every submitted value travels through env with the same spelling the
    // legacy `format!` renders would have produced.
    assert_eq!(compiled.env.get("PLINK2_CLUMP_P1").unwrap(), "1e-7");
    assert_eq!(compiled.env.get("PLINK2_CLUMP_P2").unwrap(), "2e-6");
    assert_eq!(compiled.env.get("PLINK2_CLUMP_R2").unwrap(), "0.05");
    assert_eq!(compiled.env.get("PLINK2_CLUMP_KB").unwrap(), "25000");
    assert_eq!(compiled.env.get("PLINK2_CLUMP_CHR").unwrap(), "6");
    assert_eq!(
        compiled.env.get("PLINK2_CLUMP_INPUT_FORMAT").unwrap(),
        "ieu_open_gwas"
    );

    // The script itself is static: the conditional flag and the column
    // selector mapping live in the script, not in the rendered text.
    let script = compiled.script.as_deref().unwrap();
    assert!(script.contains("--clump-id-field variant --clump-p-field pval"));
    assert!(script.contains("--chr $PLINK2_CLUMP_CHR"));
}

#[test]
fn plink2_clump_plugin_rejects_unknown_params_and_bad_input_format_is_script_owned() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "plink2_clump");

    // The legacy spec exposed artifact_prefix/timeout_secs as per-instance
    // overridable fields with defaults; the plugin DSL bakes them as fixed
    // node-level manifest values, so submitting them fails the same
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

    // The DSL has no enum param type: `input_format` is a string param, and
    // an unsupported value would be rejected by the script's `case` catch-all
    // (mirrored here only as documentation of the fail-closed posture).
    let script = load_manifest(&root)
        .nodes
        .into_iter()
        .find(|n| n.kind == "plink2_clump")
        .unwrap()
        .command
        .script
        .unwrap();
    assert!(script.contains("unsupported input_format"));
}

#[test]
fn plink2_clump_plugin_schema_marks_optionals_not_required() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "plink2_clump");

    let schema =
        serde_json::to_value(container_plugin::compile::compile_schema(&node.params)).unwrap();
    // Every legacy spec field had a serde default (or was Option), so the
    // compiled schema leaves `required` empty — same ergonomics as before.
    let required = schema["required"].as_array().unwrap();
    assert!(required.is_empty());
    assert_eq!(schema["properties"]["clump_p1"]["type"], "number");
    assert_eq!(schema["properties"]["clump_p1"]["minimum"], 0.0);
    assert_eq!(schema["properties"]["clump_p1"]["maximum"], 1.0);
    assert_eq!(schema["properties"]["clump_p2"]["type"], "number");
    assert_eq!(schema["properties"]["clump_r2"]["type"], "number");
    assert_eq!(schema["properties"]["clump_kb"]["type"], "integer");
    assert_eq!(schema["properties"]["clump_kb"]["exclusiveMinimum"], 0.0);
    assert_eq!(schema["properties"]["chr"]["type"], "integer");
    assert_eq!(schema["properties"]["chr"]["minimum"], 1.0);
    assert_eq!(schema["properties"]["chr"]["maximum"], 22.0);
    assert_eq!(schema["properties"]["input_format"]["type"], "string");
    assert_eq!(
        schema["properties"]["input_format"]["default"],
        json!("twosamplemr")
    );
}
