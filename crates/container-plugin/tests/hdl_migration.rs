//! Golden parity tests for the hdl plugin family: each `[[nodes]]` entry
//! must compile to the same `Container` spec the legacy Rust wrapper
//! (`hdl_l_container.rs` / `hdl_l_scan_container.rs`) produced. The plugin
//! directory lives outside this repository (`/mnt/projects/node-plugins/hdl`
//! by default, overridable via `NODE_PLUGINS_ROOT`); the test is skipped
//! when absent so CI without the plugin checkout stays green.

use std::path::PathBuf;

use container_plugin::compile::spec_compile::compile_container_spec;
use container_plugin::manifest::PluginManifest;
use serde_json::json;

fn plugin_root() -> Option<PathBuf> {
    let root = std::env::var_os("NODE_PLUGINS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/projects/node-plugins"));
    let manifest = root.join("hdl").join("manifest.toml");
    manifest.is_file().then_some(root)
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let text = std::fs::read_to_string(root.join("hdl").join("manifest.toml")).unwrap();
    // Inline script_file the way the loader does.
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        if let Some(relative) = node.command.script_file.clone() {
            let source = std::fs::read_to_string(root.join("hdl").join(&relative)).unwrap();
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
fn hdl_l_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "hdl_l");

    // chr/piece/trait names were required by the legacy wrapper too; the
    // values mirror the legacy wrapper unit-test baseline (chr 1, piece 3).
    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({
            "chr": 1,
            "piece": 3,
            "trait1_name": "Basal metabolic rate",
            "trait2_name": "Standing height"
        }),
    )
    .unwrap();

    assert_eq!(
        compiled.image,
        "ghcr.io/auto-nomics/autonomics/hdl@sha256:cbce3f3e4037b8c53c59275240f4449f2041de39aa95a4b5d9e588b9a75b18ea"
    );
    assert_eq!(compiled.outputs.len(), 3);
    assert_eq!(compiled.outputs[0].path, "hdl_l.tsv");
    assert_eq!(
        compiled.outputs[0].format.as_deref(),
        Some("hdl_l_result_tsv")
    );
    assert_eq!(compiled.outputs[1].path, "hdl_l.RDS");
    assert_eq!(compiled.outputs[1].format.as_deref(), Some("r_rds"));
    assert_eq!(compiled.outputs[2].path, "hdl_l.log");
    assert_eq!(compiled.outputs[2].format.as_deref(), Some("hdl_l_log"));
    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert!(matches!(
        compiled.pull_policy,
        container_runtime::PullPolicy::Missing
    ));
    assert_eq!(compiled.timeout_secs, 1800);
    assert_eq!(compiled.artifact_prefix, "/artifacts/hdl_l");
    assert_eq!(compiled.workdir, None);
    assert!(compiled.panels.is_empty());
    assert_eq!(compiled.panel_bundles.len(), 1);
    assert_eq!(
        compiled.panel_bundles[0].panel_id,
        "wjixiang/catalog-hdl-ref-ukb-eur"
    );
    assert_eq!(compiled.panel_bundles[0].mount_path, "/panels/hdl_ref");
    assert_eq!(compiled.command, vec!["Rscript".to_string()]);

    // Defaults ride through env exactly as declared. serde_json renders
    // f64 0.0 / 335272.0 with the trailing ".0" where the legacy format!
    // printed "0" / "335272"; both parse to the same number in R
    // (documented renderer pitfall 8). The lim default renders in ryu's
    // shortest scientific form where the legacy Display spelled it out;
    // the parsed values are equal.
    assert_eq!(compiled.env.get("HDL_L_CHR").unwrap(), "1");
    assert_eq!(compiled.env.get("HDL_L_PIECE").unwrap(), "3");
    assert_eq!(
        compiled.env.get("HDL_L_TRAIT1_NAME").unwrap(),
        "Basal metabolic rate"
    );
    assert_eq!(
        compiled.env.get("HDL_L_TRAIT2_NAME").unwrap(),
        "Standing height"
    );
    assert_eq!(compiled.env.get("HDL_L_N0").unwrap(), "0.0");
    assert_eq!(compiled.env.get("HDL_L_NREF").unwrap(), "335272.0");
    assert_eq!(compiled.env.get("HDL_L_EIGEN_CUT").unwrap(), "0.99");
    assert_eq!(compiled.env.get("HDL_L_ALPHA").unwrap(), "0.05");
    assert_eq!(
        compiled.env.get("HDL_L_LIM").unwrap(),
        "1.522997974471263e-8"
    );
    assert_eq!(compiled.env.get("HDL_L_INTERCEPT_OUTPUT").unwrap(), "false");
    assert_eq!(compiled.env.get("HDL_L_FILL_MISSING_N").unwrap(), "");

    // Semantic script markers, not byte equality: the legacy wrapper
    // inlined the spec values as literals, the plugin ships one static
    // script that reads the same values from the HDL_L_* env vars. The
    // official call shape and panel contract are load-bearing.
    let script = compiled.script.as_deref().unwrap();
    assert!(script.contains("HDL::HDL.L("));
    assert!(script.contains("Trait1name = trait1_name"));
    assert!(script.contains("Trait2name = trait2_name"));
    assert!(script.contains("ld_path <- \"/panels/hdl_ref/LD/\""));
    assert!(script.contains("bim_path <- \"/panels/hdl_ref/bim/\""));
    assert!(script.contains("HDLL_LOC_snps.RData"));
    assert!(script.contains("data.table::fread"));
    assert!(script.contains("output.file = Sys.getenv(\"AUTONOMICS_OUTPUT2\")"));
    assert!(script.contains("write.table("));
    assert!(script.contains("saveRDS(result"));
    assert!(script.contains("AUTONOMICS_INPUT0"));
    assert!(script.contains("AUTONOMICS_INPUT1"));
}

#[test]
fn hdl_l_plugin_renders_submitted_values_into_env() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "hdl_l");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({
            "chr": 7,
            "piece": 12,
            "trait1_name": "Trait A",
            "trait2_name": "Trait B",
            "n0": 1000.0,
            "nref": 300000.0,
            "eigen_cut": 0.95,
            "alpha": 0.01,
            "lim": 1e-8,
            "intercept_output": true,
            "fill_missing_n": "median"
        }),
    )
    .unwrap();

    assert_eq!(compiled.env.get("HDL_L_CHR").unwrap(), "7");
    assert_eq!(compiled.env.get("HDL_L_PIECE").unwrap(), "12");
    assert_eq!(compiled.env.get("HDL_L_TRAIT1_NAME").unwrap(), "Trait A");
    assert_eq!(compiled.env.get("HDL_L_TRAIT2_NAME").unwrap(), "Trait B");
    assert_eq!(compiled.env.get("HDL_L_N0").unwrap(), "1000.0");
    assert_eq!(compiled.env.get("HDL_L_NREF").unwrap(), "300000.0");
    assert_eq!(compiled.env.get("HDL_L_EIGEN_CUT").unwrap(), "0.95");
    assert_eq!(compiled.env.get("HDL_L_ALPHA").unwrap(), "0.01");
    // serde_json renders f64 1e-8 in ryu's shortest form.
    assert_eq!(compiled.env.get("HDL_L_LIM").unwrap(), "1e-8");
    assert_eq!(compiled.env.get("HDL_L_INTERCEPT_OUTPUT").unwrap(), "true");
    assert_eq!(compiled.env.get("HDL_L_FILL_MISSING_N").unwrap(), "median");
}

#[test]
fn hdl_l_plugin_schema_marks_required_params_and_bounds() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "hdl_l");

    let schema =
        serde_json::to_value(container_plugin::compile::compile_schema(&node.params)).unwrap();
    // BTreeMap order: exactly the four legacy non-defaulted fields.
    let required = schema["required"].as_array().unwrap();
    assert_eq!(
        required,
        &vec![
            json!("chr"),
            json!("piece"),
            json!("trait1_name"),
            json!("trait2_name")
        ]
    );
    assert_eq!(schema["properties"]["chr"]["type"], "integer");
    assert_eq!(schema["properties"]["chr"]["minimum"], 1.0);
    assert_eq!(schema["properties"]["chr"]["maximum"], 22.0);
    assert_eq!(schema["properties"]["piece"]["exclusiveMinimum"], 0.0);
    assert_eq!(schema["properties"]["nref"]["default"], 335272.0);
    assert_eq!(schema["properties"]["eigen_cut"]["maximum"], 1.0);
    assert_eq!(schema["properties"]["alpha"]["exclusiveMinimum"], 0.0);
    assert_eq!(schema["properties"]["lim"]["exclusiveMinimum"], 0.0);
    // fill_missing_n stays optional: the enum lives in the script.
    assert_eq!(schema["properties"]["fill_missing_n"]["type"], "string");
}

#[test]
fn hdl_l_scan_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "hdl_l_scan");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({
            "chr": 1,
            "trait1_name": "trait1",
            "trait2_name": "trait2"
        }),
    )
    .unwrap();

    assert_eq!(
        compiled.image,
        "ghcr.io/auto-nomics/autonomics/hdl@sha256:cbce3f3e4037b8c53c59275240f4449f2041de39aa95a4b5d9e588b9a75b18ea"
    );
    assert_eq!(compiled.outputs.len(), 3);
    assert_eq!(compiled.outputs[0].path, "hdl_l_scan.tsv");
    assert_eq!(
        compiled.outputs[0].format.as_deref(),
        Some("hdl_l_scan_result_tsv")
    );
    assert_eq!(compiled.outputs[1].path, "hdl_l_scan.RDS");
    assert_eq!(compiled.outputs[1].format.as_deref(), Some("r_rds"));
    assert_eq!(compiled.outputs[2].path, "hdl_l_scan.log");
    assert_eq!(
        compiled.outputs[2].format.as_deref(),
        Some("hdl_l_scan_log")
    );
    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert!(matches!(
        compiled.pull_policy,
        container_runtime::PullPolicy::Missing
    ));
    // The scan is the whole-chromosome run: the legacy timeout was one day.
    assert_eq!(compiled.timeout_secs, 86400);
    assert_eq!(compiled.artifact_prefix, "/artifacts/hdl_l_scan");
    assert_eq!(compiled.panel_bundles.len(), 1);
    assert_eq!(
        compiled.panel_bundles[0].panel_id,
        "wjixiang/catalog-hdl-ref-ukb-eur"
    );
    assert_eq!(compiled.panel_bundles[0].mount_path, "/panels/hdl_ref");
    assert_eq!(compiled.command, vec!["Rscript".to_string()]);

    // pieces omitted: env renders empty and the scan runs every block.
    assert_eq!(compiled.env.get("HDL_L_SCAN_CHR").unwrap(), "1");
    assert_eq!(compiled.env.get("HDL_L_SCAN_PIECES").unwrap(), "");
    assert_eq!(
        compiled.env.get("HDL_L_SCAN_TRAIT1_NAME").unwrap(),
        "trait1"
    );
    assert_eq!(compiled.env.get("HDL_L_SCAN_NREF").unwrap(), "335272.0");
    assert_eq!(
        compiled.env.get("HDL_L_SCAN_INTERCEPT_OUTPUT").unwrap(),
        "false"
    );
    assert_eq!(compiled.env.get("HDL_L_SCAN_FILL_MISSING_N").unwrap(), "");

    // Semantic script markers: the per-chr/piece loop stays script-side
    // (identical loop shape to the legacy generated script), block
    // selection comes from NEWLOC, and failed blocks are skipped.
    let script = compiled.script.as_deref().unwrap();
    assert!(script.contains("load(marker_path)"));
    assert!(script.contains("blocks <- NEWLOC[NEWLOC$CHR == chr, , drop = FALSE]"));
    assert!(script.contains("requested_pieces"));
    assert!(script.contains("for (row_index in seq_len(nrow(blocks)))"));
    assert!(script.contains("HDL::HDL.L("));
    assert!(script.contains("tryCatch("));
    assert!(script.contains("sep = \"\\t\""));
    assert!(script.contains("AUTONOMICS_INPUT0"));
    assert!(script.contains("AUTONOMICS_INPUT1"));
    assert!(script.contains("saveRDS(results"));
}

#[test]
fn hdl_l_scan_plugin_renders_submitted_pieces_into_env() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "hdl_l_scan");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({
            "chr": 4,
            "pieces": ["8", "9"],
            "trait1_name": "trait1",
            "trait2_name": "trait2"
        }),
    )
    .unwrap();

    assert_eq!(compiled.env.get("HDL_L_SCAN_CHR").unwrap(), "4");
    // The env renderer space-joins the string_array; the script splits it
    // back apart and re-validates positivity and duplicates.
    assert_eq!(compiled.env.get("HDL_L_SCAN_PIECES").unwrap(), "8 9");
}

#[test]
fn hdl_l_scan_plugin_schema_leaves_pieces_optional() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "hdl_l_scan");

    let schema =
        serde_json::to_value(container_plugin::compile::compile_schema(&node.params)).unwrap();
    let required = schema["required"].as_array().unwrap();
    assert_eq!(
        required,
        &vec![json!("chr"), json!("trait1_name"), json!("trait2_name")]
    );
    assert_eq!(schema["properties"]["pieces"]["type"], "array");
    assert_eq!(schema["properties"]["pieces"]["items"]["type"], "string");
    assert_eq!(schema["properties"]["pieces"]["minItems"], 1.0);
}
