//! Golden parity tests for the gcta plugin family: each `[[nodes]]` entry
//! must compile to the same `Container` spec the legacy Rust wrapper
//! (`nodes_io::gcta_container`) produced. The plugin directory lives
//! outside this repository (`/mnt/projects/node-plugins/gcta` by default,
//! overridable via `NODE_PLUGINS_ROOT`); the test is skipped when absent so
//! CI without the plugin checkout stays green.
//!
//! Contract notes specific to this family:
//!
//! - `chr` (cojo_select/sblup/fastbat) and `lambda` (sblup) are required
//!   params with no default, so the contract tests submit them (chr 22,
//!   matching the legacy wrapper's own unit tests); every other field
//!   resolves from its manifest default, which is the legacy default.
//! - The v0 panel DSL is family-wide, so every node compiles with both
//!   panels. The legacy wrapper attached 1 (cojo_select/sblup/acat) or 2
//!   (fastbat); the plugin contract is the documented superset (the same
//!   precedent as ldsc_munge). fastbat is byte-identical to legacy.
//! - Legacy built `command = ["sh", "-c"]` with an inline script; the
//!   plugin compiles `["sh"]` and the runtime materializes the script at
//!   argv[1]. The legacy trailing `-c` only ever served as `$0`.

use std::path::PathBuf;

use container_plugin::compile::spec_compile::compile_container_spec;
use container_plugin::manifest::PluginManifest;
use serde_json::json;

const GCTA_IMAGE: &str = "ghcr.io/auto-nomics/autonomics/gcta@sha256:4cbf8c91376f7b314eebf1dfa02ad44028575991cd3d4c4e81b5324942bec20b";
const PLINK_PANEL: &str = "wjixiang/catalog-plink-ref-1000g-eur-binary";
const GENE_LIST_PANEL: &str = "wjixiang/catalog-gcta-gene-list-hg19";

fn plugin_root() -> Option<PathBuf> {
    let explicit = std::env::var_os("NODE_PLUGINS_ROOT");
    let root = explicit
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/projects/node-plugins"));
    let manifest = root.join("gcta").join("manifest.toml");
    if manifest.is_file() {
        return Some(root);
    }
    if explicit.is_some() {
        // fix-review R04 (2026-10-05): an explicitly set NODE_PLUGINS_ROOT
        // means migration parity was requested. A missing family must fail
        // loudly — early-returns here used to count as *passed* tests, so a
        // green summary claimed coverage that never ran.
        panic!(
            "NODE_PLUGINS_ROOT is set but the gcta family is not deployed \
             under it ({}) — migration parity cannot run. Deploy the family \
             or unset NODE_PLUGINS_ROOT to skip these tests deliberately.",
            manifest.display()
        );
    }
    eprintln!("skipping: gcta plugin directory not present (NODE_PLUGINS_ROOT unset)");
    None
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let text = std::fs::read_to_string(root.join("gcta").join("manifest.toml")).unwrap();
    // Inline script_file the way the loader does.
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        if let Some(relative) = node.command.script_file.clone() {
            let source = std::fs::read_to_string(root.join("gcta").join(&relative)).unwrap();
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
fn gcta_cojo_select_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "gcta_cojo_select");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        // chr is required in the legacy spec (no default); 22 mirrors the
        // legacy wrapper's own unit tests.
        &json!({"chr": 22}),
    )
    .unwrap();

    assert_eq!(compiled.image, GCTA_IMAGE);
    assert_eq!(compiled.outputs.len(), 4);
    assert_eq!(compiled.outputs[0].path, "gcta_cojo.jma.cojo");
    assert_eq!(compiled.outputs[0].format.as_deref(), Some("gcta_cojo_jma"));
    assert_eq!(compiled.outputs[1].path, "gcta_cojo.ldr.cojo");
    assert_eq!(compiled.outputs[1].format.as_deref(), Some("gcta_cojo_ldr"));
    assert_eq!(compiled.outputs[2].path, "gcta_cojo.cma.cojo");
    assert_eq!(compiled.outputs[2].format.as_deref(), Some("gcta_cojo_cma"));
    assert_eq!(compiled.outputs[3].path, "gcta_cojo.log");
    assert_eq!(compiled.outputs[3].format.as_deref(), Some("gcta_log"));
    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert!(matches!(
        compiled.pull_policy,
        container_runtime::PullPolicy::Missing
    ));
    assert_eq!(compiled.timeout_secs, 3600);
    // The legacy default_prefix() was family-shared; the manifest pins the
    // same value per node instead of the DSL's /artifacts/{kind} default.
    assert_eq!(
        compiled.artifact_prefix,
        Some("/artifacts/gcta_container".to_string())
    );
    assert_eq!(compiled.workdir, None);
    assert!(compiled.panels.is_empty());
    // Family-wide panels (documented superset of the legacy single panel).
    let panel_ids: Vec<&str> = compiled
        .panel_bundles
        .iter()
        .map(|p| p.panel_id.as_str())
        .collect();
    assert_eq!(panel_ids, vec![PLINK_PANEL, GENE_LIST_PANEL]);
    assert_eq!(
        compiled.panel_bundles[0].mount_path, "/panels/plink_ref",
        "legacy mount path preserved"
    );
    assert_eq!(
        compiled.panel_bundles[1].mount_path, "/panels/gene_list",
        "legacy mount path preserved"
    );
    // Legacy built ["sh", "-c"]; the plugin contract is ["sh"] (see the
    // module comment).
    assert_eq!(compiled.command, vec!["sh".to_string()]);

    // Env defaults resolve to the legacy constants.
    assert_eq!(compiled.env.get("GCTA_CHR").unwrap(), "22");
    assert_eq!(compiled.env.get("GCTA_MAF").unwrap(), "0.01");
    // Legacy formatted cojo_p with {:e}; serde_json renders 5e-8 with the
    // same spelling.
    assert_eq!(compiled.env.get("GCTA_COJO_P").unwrap(), "5e-8");
    assert_eq!(compiled.env.get("GCTA_COJO_WIND_KB").unwrap(), "10000");
    assert_eq!(compiled.env.get("GCTA_COJO_COLLINEAR").unwrap(), "0.9");
    assert_eq!(compiled.env.get("GCTA_DIFF_FREQ").unwrap(), "0.2");
    assert_eq!(compiled.env.get("GCTA_THREAD_NUM").unwrap(), "1");

    // Semantic script markers (the plugin drives flags through env instead
    // of Rust string building, so variable names differ from the legacy
    // inline script).
    let script = compiled.script.as_deref().unwrap();
    assert!(script.contains("--bfile \"/panels/plink_ref/1000G.EUR.QC.${GCTA_CHR}\""));
    assert!(script.contains("--cojo-file \"$AUTONOMICS_INPUT0\""));
    assert!(script.contains("--cojo-slct"));
    assert!(script.contains("--cojo-p \"$GCTA_COJO_P\""));
    assert!(script.contains("--cojo-wind \"$GCTA_COJO_WIND_KB\""));
    assert!(script.contains("--cojo-collinear \"$GCTA_COJO_COLLINEAR\""));
    assert!(script.contains("--diff-freq \"$GCTA_DIFF_FREQ\""));
    assert!(script.contains("--thread-num \"$GCTA_THREAD_NUM\""));
    assert!(script.contains("--out \"$AUTONOMICS_WORKDIR/gcta_cojo\""));
    // Log lands on output port 3; JMA and LDR are asserted non-empty.
    assert!(script.contains("> \"$AUTONOMICS_OUTPUT3\" 2>&1"));
    assert!(script.contains("test -s \"$AUTONOMICS_OUTPUT0\""));
    assert!(script.contains("test -s \"$AUTONOMICS_OUTPUT1\""));
}

#[test]
fn gcta_cojo_select_plugin_renders_submitted_values_into_env() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "gcta_cojo_select");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({
            "chr": 5,
            "maf": 0.05,
            "cojo_p": 1e-6,
            "cojo_wind_kb": 5000,
            "cojo_collinear": 0.8,
            "diff_freq": 0.1,
            "thread_num": 4
        }),
    )
    .unwrap();

    assert_eq!(compiled.env.get("GCTA_CHR").unwrap(), "5");
    assert_eq!(compiled.env.get("GCTA_MAF").unwrap(), "0.05");
    // serde_json renders 1e-6 as "1e-6"; the legacy {:e} spelling matched.
    assert_eq!(compiled.env.get("GCTA_COJO_P").unwrap(), "1e-6");
    assert_eq!(compiled.env.get("GCTA_COJO_WIND_KB").unwrap(), "5000");
    assert_eq!(compiled.env.get("GCTA_COJO_COLLINEAR").unwrap(), "0.8");
    assert_eq!(compiled.env.get("GCTA_DIFF_FREQ").unwrap(), "0.1");
    assert_eq!(compiled.env.get("GCTA_THREAD_NUM").unwrap(), "4");
}

#[test]
fn gcta_sblup_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "gcta_sblup");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        // chr and lambda are both required in the legacy spec.
        &json!({"chr": 22, "lambda": 1.33e6}),
    )
    .unwrap();

    assert_eq!(compiled.image, GCTA_IMAGE);
    assert_eq!(compiled.outputs.len(), 2);
    assert_eq!(compiled.outputs[0].path, "gcta_sblup.sblup.cojo");
    assert_eq!(compiled.outputs[0].format.as_deref(), Some("gcta_sblup"));
    assert_eq!(compiled.outputs[1].path, "gcta_sblup.log");
    assert_eq!(compiled.outputs[1].format.as_deref(), Some("gcta_log"));
    assert_eq!(compiled.timeout_secs, 3600);
    assert_eq!(
        compiled.artifact_prefix,
        Some("/artifacts/gcta_container".to_string())
    );
    // Family-wide panels (documented superset of the legacy single panel).
    assert_eq!(compiled.panel_bundles.len(), 2);

    // The SBLUP window default is 1000, not cojo_select's 10000.
    assert_eq!(compiled.env.get("GCTA_COJO_WIND_KB").unwrap(), "1000");
    assert_eq!(compiled.env.get("GCTA_MAF").unwrap(), "0.01");
    assert_eq!(compiled.env.get("GCTA_DIFF_FREQ").unwrap(), "0.2");
    assert_eq!(compiled.env.get("GCTA_THREAD_NUM").unwrap(), "1");
    // Pitfall 8: serde_json renders f64 1.33e6 as "1330000.0" where the
    // legacy format!({:e}) produced "1.33e6". Same f64 after parsing; GCTA
    // accepts both spellings.
    assert_eq!(compiled.env.get("GCTA_LAMBDA").unwrap(), "1330000.0");

    let script = compiled.script.as_deref().unwrap();
    assert!(script.contains("--bfile \"/panels/plink_ref/1000G.EUR.QC.${GCTA_CHR}\""));
    assert!(script.contains("--cojo-file \"$AUTONOMICS_INPUT0\""));
    assert!(script.contains("--cojo-sblup \"$GCTA_LAMBDA\""));
    assert!(script.contains("--cojo-wind \"$GCTA_COJO_WIND_KB\""));
    assert!(script.contains("--out \"$AUTONOMICS_WORKDIR/gcta_sblup\""));
    assert!(script.contains("> \"$AUTONOMICS_OUTPUT1\" 2>&1"));
    assert!(script.contains("test -s \"$AUTONOMICS_OUTPUT0\""));
}

#[test]
fn gcta_fastbat_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "gcta_fastbat");

    let compiled =
        compile_container_spec(node, &manifest.image, &manifest.panels, &json!({"chr": 22}))
            .unwrap();

    assert_eq!(compiled.image, GCTA_IMAGE);
    assert_eq!(compiled.outputs.len(), 2);
    assert_eq!(compiled.outputs[0].path, "gcta_fastbat.gene.fastbat");
    assert_eq!(compiled.outputs[0].format.as_deref(), Some("gcta_fastbat"));
    assert_eq!(compiled.outputs[1].path, "gcta_fastbat.log");
    assert_eq!(compiled.outputs[1].format.as_deref(), Some("gcta_log"));
    assert_eq!(compiled.timeout_secs, 3600);
    assert_eq!(
        compiled.artifact_prefix,
        Some("/artifacts/gcta_container".to_string())
    );
    // fastbat is the one variant whose legacy contract wired both panels;
    // the family-wide plugin binding is byte-identical here.
    let panel_ids: Vec<&str> = compiled
        .panel_bundles
        .iter()
        .map(|p| p.panel_id.as_str())
        .collect();
    assert_eq!(panel_ids, vec![PLINK_PANEL, GENE_LIST_PANEL]);

    assert_eq!(compiled.env.get("GCTA_GENE_FLANK_KB").unwrap(), "50");
    assert_eq!(compiled.env.get("GCTA_LD_CUTOFF").unwrap(), "0.9");
    assert_eq!(compiled.env.get("GCTA_MAF").unwrap(), "0.01");

    let script = compiled.script.as_deref().unwrap();
    assert!(script.contains("--fastBAT \"$AUTONOMICS_INPUT0\""));
    assert!(script.contains("--fastBAT-gene-list /panels/gene_list/glist-hg19.txt"));
    assert!(script.contains("--fastBAT-wind \"$GCTA_GENE_FLANK_KB\""));
    assert!(script.contains("--fastBAT-ld-cutoff \"$GCTA_LD_CUTOFF\""));
    assert!(script.contains("--out \"$AUTONOMICS_WORKDIR/gcta_fastbat\""));
    assert!(script.contains("test -s \"$AUTONOMICS_OUTPUT0\""));
}

#[test]
fn gcta_acat_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "gcta_acat");

    // ACAT has no required params: the empty spec is exactly the legacy
    // default spec (max_maf 0.01, min_mac 20, gene_flank_kb 0).
    let compiled =
        compile_container_spec(node, &manifest.image, &manifest.panels, &json!({})).unwrap();

    assert_eq!(compiled.image, GCTA_IMAGE);
    assert_eq!(compiled.outputs.len(), 2);
    assert_eq!(compiled.outputs[0].path, "gcta_acat.acat");
    assert_eq!(compiled.outputs[0].format.as_deref(), Some("gcta_acat"));
    assert_eq!(compiled.outputs[1].path, "gcta_acat.log");
    assert_eq!(compiled.outputs[1].format.as_deref(), Some("gcta_log"));
    assert_eq!(compiled.timeout_secs, 3600);
    assert_eq!(
        compiled.artifact_prefix,
        Some("/artifacts/gcta_container".to_string())
    );
    // Family-wide panels (documented superset of the legacy gene-list-only
    // binding; ACAT never reads plink_ref).
    let panel_ids: Vec<&str> = compiled
        .panel_bundles
        .iter()
        .map(|p| p.panel_id.as_str())
        .collect();
    assert_eq!(panel_ids, vec![PLINK_PANEL, GENE_LIST_PANEL]);

    assert_eq!(compiled.env.get("GCTA_MAX_MAF").unwrap(), "0.01");
    assert_eq!(compiled.env.get("GCTA_MIN_MAC").unwrap(), "20");
    // Legacy default gene_flank_kb = 0 reaches --wind verbatim.
    assert_eq!(compiled.env.get("GCTA_GENE_FLANK_KB").unwrap(), "0");

    let script = compiled.script.as_deref().unwrap();
    assert!(script.contains("--acat"));
    assert!(script.contains("--snp-list \"$AUTONOMICS_INPUT0\""));
    assert!(script.contains("--gene-list /panels/gene_list/glist-hg19.txt"));
    assert!(script.contains("--max-maf \"$GCTA_MAX_MAF\""));
    assert!(script.contains("--min-mac \"$GCTA_MIN_MAC\""));
    assert!(script.contains("--wind \"$GCTA_GENE_FLANK_KB\""));
    assert!(script.contains("--out \"$AUTONOMICS_WORKDIR/gcta_acat_raw\""));
    assert!(script.contains("test -s \"$AUTONOMICS_WORKDIR/gcta_acat_raw\""));
    assert!(script.contains("mv \"$AUTONOMICS_WORKDIR/gcta_acat_raw\" \"$AUTONOMICS_OUTPUT0\""));
}

#[test]
fn gcta_plugin_schemas_keep_the_legacy_required_and_bound_fields() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);

    // cojo_select: chr required with the legacy 1..=22 bound; float bounds
    // mirror validate_common/validate_cojo_select.
    let schema = serde_json::to_value(container_plugin::compile::compile_schema(
        &node_by_kind(&manifest, "gcta_cojo_select").params,
    ))
    .unwrap();
    assert_eq!(schema["required"], json!(["chr"]));
    assert_eq!(schema["properties"]["chr"]["type"], "integer");
    assert_eq!(schema["properties"]["chr"]["minimum"], 1.0);
    assert_eq!(schema["properties"]["chr"]["maximum"], 22.0);
    assert_eq!(schema["properties"]["maf"]["exclusiveMinimum"], 0.0);
    assert_eq!(schema["properties"]["maf"]["maximum"], 0.5);
    assert_eq!(schema["properties"]["cojo_p"]["default"], json!(5e-8));
    assert_eq!(
        schema["properties"]["cojo_collinear"]["exclusiveMaximum"],
        1.0
    );
    assert_eq!(schema["properties"]["thread_num"]["maximum"], 1024.0);

    // sblup: chr and lambda are the two required params (lambda > 0).
    let schema = serde_json::to_value(container_plugin::compile::compile_schema(
        &node_by_kind(&manifest, "gcta_sblup").params,
    ))
    .unwrap();
    assert_eq!(schema["required"], json!(["chr", "lambda"]));
    assert_eq!(schema["properties"]["lambda"]["exclusiveMinimum"], 0.0);
    assert_eq!(schema["properties"]["cojo_wind_kb"]["default"], json!(1000));

    // acat: no required params (the empty spec is valid), legacy defaults.
    let schema = serde_json::to_value(container_plugin::compile::compile_schema(
        &node_by_kind(&manifest, "gcta_acat").params,
    ))
    .unwrap();
    let required = schema["required"].as_array().unwrap();
    assert!(required.is_empty());
    assert_eq!(schema["properties"]["max_maf"]["default"], json!(0.01));
    assert_eq!(schema["properties"]["min_mac"]["default"], json!(20));
    assert_eq!(schema["properties"]["gene_flank_kb"]["default"], json!(0));
    assert_eq!(schema["properties"]["gene_flank_kb"]["minimum"], 0.0);
}
