//! Golden parity tests for the smr plugin family: each `[[nodes]]` entry
//! must compile to the same `Container` spec the legacy Rust wrapper
//! (`smr_heidi_container`) produced. The legacy `eqtl_source` enum has no
//! v0 DSL equivalent, so it became two kinds sharing one image and one
//! three-panel family (superset mounting, ldsc-munge precedent): the
//! plugin directory lives outside this repository
//! (`/mnt/projects/node-plugins/smr` by default, overridable via
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
    let manifest = root.join("smr").join("manifest.toml");
    if manifest.is_file() {
        return Some(root);
    }
    if explicit.is_some() {
        // fix-review R04 (2026-10-05): an explicitly set NODE_PLUGINS_ROOT
        // means migration parity was requested. A missing family must fail
        // loudly — early-returns here used to count as *passed* tests, so a
        // green summary claimed coverage that never ran.
        panic!(
            "NODE_PLUGINS_ROOT is set but the smr family is not deployed \
             under it ({}) — migration parity cannot run. Deploy the family \
             or unset NODE_PLUGINS_ROOT to skip these tests deliberately.",
            manifest.display()
        );
    }
    eprintln!("skipping: smr plugin directory not present (NODE_PLUGINS_ROOT unset)");
    None
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let text = std::fs::read_to_string(root.join("smr").join("manifest.toml")).unwrap();
    // Inline script_file the way the loader does.
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        if let Some(relative) = node.command.script_file.clone() {
            let source = std::fs::read_to_string(root.join("smr").join(&relative)).unwrap();
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

/// The legacy wrapper bound exactly two panels (the selected eQTL BESD
/// package at `/panels/smr_eqtl` plus the 1000G EUR binary LD reference);
/// the plugin mounts both eQTL packages on every node at distinct paths
/// and each script reads only its own. Manifest order is preserved.
const EXPECTED_PANEL_IDS: [&str; 3] = [
    "wjixiang/catalog-smr-eqtl-westra-hg19",
    "wjixiang/catalog-smr-eqtl-eqtlgen-hg19",
    "wjixiang/catalog-plink-ref-1000g-eur-binary",
];

const EXPECTED_PANEL_MOUNTS: [&str; 3] = [
    "/panels/smr_eqtl_westra",
    "/panels/smr_eqtl_eqtlgen",
    "/panels/smr_ld_ref",
];

#[test]
fn smr_heidi_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "smr_heidi");

    let compiled =
        compile_container_spec(node, &manifest.image, &manifest.panels, &json!({"chr": 22}))
            .unwrap();

    assert_eq!(
        compiled.image,
        "ghcr.io/auto-nomics/autonomics/smr@sha256:40c0db3c71eda506913c376ab939027fff8ce8eb55c262fd1e5da2fe4c351b6d"
    );
    assert_eq!(compiled.outputs.len(), 2);
    assert_eq!(compiled.outputs[0].path, "smr.smr");
    assert_eq!(compiled.outputs[0].format.as_deref(), Some("smr_heidi"));
    assert_eq!(compiled.outputs[1].path, "smr.log");
    assert_eq!(compiled.outputs[1].format.as_deref(), Some("smr_log"));
    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert!(matches!(
        compiled.pull_policy,
        container_runtime::PullPolicy::Missing
    ));
    assert_eq!(compiled.timeout_secs, 3600);
    // The legacy default prefix carried the `_container` suffix; plugin
    // kinds are suffix-less, so the prefix is kind-derived.
    assert_eq!(compiled.artifact_prefix, "/artifacts/smr_heidi");
    assert_eq!(compiled.workdir, None);
    assert!(compiled.panels.is_empty());
    let panel_ids: Vec<&str> = compiled
        .panel_bundles
        .iter()
        .map(|p| p.panel_id.as_str())
        .collect();
    assert_eq!(panel_ids, EXPECTED_PANEL_IDS);
    let mounts: Vec<&str> = compiled
        .panel_bundles
        .iter()
        .map(|p| p.mount_path.as_str())
        .collect();
    assert_eq!(mounts, EXPECTED_PANEL_MOUNTS);
    assert_eq!(compiled.command, vec!["sh".to_string()]);
    // Every param travels through env, nothing else is templated.
    assert_eq!(compiled.env.len(), 14);

    // Default env rendering; the legacy wrapper baked these values into
    // the script via Rust `format!` (`--peqtl-smr 5e-8`).
    assert_eq!(compiled.env.get("SMR_CHR").unwrap(), "22");
    assert_eq!(compiled.env.get("SMR_MAF").unwrap(), "0.01");
    assert_eq!(compiled.env.get("SMR_PEQTL_SMR").unwrap(), "5e-8");
    // Legacy spelled the default `1.5654e-3`; serde_json renders the same
    // f64 as "0.0015654" — equal after float parsing (pitfall 8).
    assert_eq!(compiled.env.get("SMR_PEQTL_HEIDI").unwrap(), "0.0015654");
    assert_eq!(compiled.env.get("SMR_HEIDI_MTD").unwrap(), "1");
    assert_eq!(compiled.env.get("SMR_HEIDI_MIN_M").unwrap(), "3");
    assert_eq!(compiled.env.get("SMR_HEIDI_MAX_M").unwrap(), "20");
    assert_eq!(compiled.env.get("SMR_LD_LOWER_LIMIT").unwrap(), "0.05");
    assert_eq!(compiled.env.get("SMR_LD_UPPER_LIMIT").unwrap(), "0.9");
    assert_eq!(compiled.env.get("SMR_CIS_WIND_KB").unwrap(), "2000");
    assert_eq!(compiled.env.get("SMR_MAX_NUM_LD").unwrap(), "500");
    assert_eq!(compiled.env.get("SMR_DIFF_FREQ").unwrap(), "0.2");
    assert_eq!(compiled.env.get("SMR_DIFF_FREQ_PROP").unwrap(), "0.05");
    assert_eq!(compiled.env.get("SMR_THREAD_NUM").unwrap(), "1");

    // Semantic script markers (the plugin drives flags through env; the
    // legacy wrapper interpolated them with Rust string building, e.g.
    // the literal `--bfile /panels/smr_ld_ref/1000G.EUR.QC.22`).
    let script = compiled.script.as_deref().unwrap();
    assert!(script.contains("--bfile \"/panels/smr_ld_ref/1000G.EUR.QC.$SMR_CHR\""));
    assert!(script.contains("--beqtl-summary /panels/smr_eqtl_westra/westra_eqtl_hg19"));
    assert!(!script.contains("smr_eqtl_eqtlgen"));
    assert!(!script.contains("eqtlgen_hg19"));
    assert!(script.contains("--gwas-summary \"$AUTONOMICS_INPUT0\""));
    assert!(script.contains("--peqtl-smr \"$SMR_PEQTL_SMR\""));
    assert!(script.contains("--peqtl-heidi \"$SMR_PEQTL_HEIDI\""));
    assert!(script.contains("--max_num_ld \"$SMR_MAX_NUM_LD\""));
    assert!(script.contains("--thread-num \"$SMR_THREAD_NUM\""));
    // Same out-prefix as the legacy `--out /work/smr`.
    assert!(script.contains("--out \"$AUTONOMICS_WORKDIR/smr\""));
    assert!(script.contains("> \"$AUTONOMICS_OUTPUT1\" 2>&1"));
    assert!(script.contains("test -s \"$AUTONOMICS_OUTPUT0\""));
    // Cross-field validate() rules the DSL cannot express live in the
    // script (coloc precedent).
    assert!(script.contains("-le \"$SMR_HEIDI_MAX_M\""));
    assert!(script.contains("sort -g"));
}

#[test]
fn smr_heidi_eqtlgen_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "smr_heidi_eqtlgen");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({"chr": 22, "thread_num": 2}),
    )
    .unwrap();

    assert_eq!(
        compiled.image,
        "ghcr.io/auto-nomics/autonomics/smr@sha256:40c0db3c71eda506913c376ab939027fff8ce8eb55c262fd1e5da2fe4c351b6d"
    );
    assert_eq!(compiled.outputs.len(), 2);
    assert_eq!(compiled.outputs[0].path, "smr.smr");
    assert_eq!(compiled.outputs[0].format.as_deref(), Some("smr_heidi"));
    assert_eq!(compiled.outputs[1].path, "smr.log");
    assert_eq!(compiled.outputs[1].format.as_deref(), Some("smr_log"));
    assert_eq!(compiled.timeout_secs, 3600);
    assert_eq!(compiled.artifact_prefix, "/artifacts/smr_heidi_eqtlgen");
    // Superset family panels: identical bindings on both kinds; only the
    // script decides which BESD package is read.
    let panel_ids: Vec<&str> = compiled
        .panel_bundles
        .iter()
        .map(|p| p.panel_id.as_str())
        .collect();
    assert_eq!(panel_ids, EXPECTED_PANEL_IDS);
    let mounts: Vec<&str> = compiled
        .panel_bundles
        .iter()
        .map(|p| p.mount_path.as_str())
        .collect();
    assert_eq!(mounts, EXPECTED_PANEL_MOUNTS);
    assert_eq!(compiled.env.get("SMR_THREAD_NUM").unwrap(), "2");

    let script = compiled.script.as_deref().unwrap();
    assert!(script.contains("--beqtl-summary /panels/smr_eqtl_eqtlgen/eqtlgen_hg19"));
    assert!(!script.contains("westra_eqtl_hg19"));
    assert!(!script.contains("smr_eqtl_westra"));
    assert!(script.contains("--bfile \"/panels/smr_ld_ref/1000G.EUR.QC.$SMR_CHR\""));
    assert!(script.contains("--gwas-summary \"$AUTONOMICS_INPUT0\""));
    assert!(script.contains("--out \"$AUTONOMICS_WORKDIR/smr\""));
    assert!(script.contains("> \"$AUTONOMICS_OUTPUT1\" 2>&1"));
    assert!(script.contains("test -s \"$AUTONOMICS_OUTPUT0\""));
}

#[test]
fn smr_heidi_plugin_renders_submitted_values_into_env() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "smr_heidi");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({
            "chr": 22,
            "maf": 0.05,
            "peqtl_smr": 1e-7,
            "peqtl_heidi": 0.01,
            "heidi_mtd": 0,
            "heidi_min_m": 5,
            "heidi_max_m": 50,
            "ld_lower_limit": 0.1,
            "ld_upper_limit": 0.8,
            "cis_wind_kb": 1000,
            "max_num_ld": 1000,
            "diff_freq": 0.3,
            "diff_freq_prop": 0.1,
            "thread_num": 4,
        }),
    )
    .unwrap();

    assert_eq!(compiled.env.get("SMR_CHR").unwrap(), "22");
    assert_eq!(compiled.env.get("SMR_MAF").unwrap(), "0.05");
    assert_eq!(compiled.env.get("SMR_PEQTL_SMR").unwrap(), "1e-7");
    assert_eq!(compiled.env.get("SMR_PEQTL_HEIDI").unwrap(), "0.01");
    assert_eq!(compiled.env.get("SMR_HEIDI_MTD").unwrap(), "0");
    assert_eq!(compiled.env.get("SMR_HEIDI_MIN_M").unwrap(), "5");
    assert_eq!(compiled.env.get("SMR_HEIDI_MAX_M").unwrap(), "50");
    assert_eq!(compiled.env.get("SMR_LD_LOWER_LIMIT").unwrap(), "0.1");
    assert_eq!(compiled.env.get("SMR_LD_UPPER_LIMIT").unwrap(), "0.8");
    assert_eq!(compiled.env.get("SMR_CIS_WIND_KB").unwrap(), "1000");
    assert_eq!(compiled.env.get("SMR_MAX_NUM_LD").unwrap(), "1000");
    assert_eq!(compiled.env.get("SMR_DIFF_FREQ").unwrap(), "0.3");
    assert_eq!(compiled.env.get("SMR_DIFF_FREQ_PROP").unwrap(), "0.1");
    assert_eq!(compiled.env.get("SMR_THREAD_NUM").unwrap(), "4");
}

#[test]
fn smr_heidi_plugin_schema_requires_only_chr() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    for kind in ["smr_heidi", "smr_heidi_eqtlgen"] {
        let manifest = load_manifest(&root);
        let node = node_by_kind(&manifest, kind);

        let schema =
            serde_json::to_value(container_plugin::compile::compile_schema(&node.params)).unwrap();
        let required = schema["required"].as_array().unwrap();
        assert_eq!(required, &[json!("chr")], "{kind}: only chr is required");
        assert_eq!(schema["properties"]["chr"]["type"], "integer");
        assert_eq!(schema["properties"]["chr"]["minimum"], 1.0);
        assert_eq!(schema["properties"]["chr"]["maximum"], 22.0);
        assert_eq!(schema["properties"]["maf"]["type"], "number");
        assert_eq!(schema["properties"]["maf"]["exclusiveMinimum"], 0.0);
        assert_eq!(schema["properties"]["maf"]["maximum"], 0.5);
        assert_eq!(schema["properties"]["heidi_mtd"]["type"], "integer");
        assert_eq!(schema["properties"]["heidi_mtd"]["maximum"], 1.0);
        assert_eq!(schema["properties"]["max_num_ld"]["minimum"], 500.0);
        assert_eq!(schema["properties"]["max_num_ld"]["maximum"], 10000.0);
    }
}

#[test]
fn smr_heidi_plugin_fails_closed_on_missing_required_chr() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "smr_heidi");

    // The legacy wrapper validated `chr` before building the spec; the
    // plugin enforces the same at compile time through the required param.
    let error =
        compile_container_spec(node, &manifest.image, &manifest.panels, &json!({})).unwrap_err();
    assert!(
        error.to_string().contains("chr"),
        "missing chr must be reported: {error}"
    );
}
