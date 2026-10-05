//! Golden parity tests for the lava plugin family: each `[[nodes]]` entry
//! must compile to the same `Container` spec the legacy Rust wrapper
//! produced (`lava_container` and `lava_scan_container` in nodes-io). The
//! plugin directory lives outside this repository
//! (`/mnt/projects/node-plugins/lava` by default, overridable via
//! `NODE_PLUGINS_ROOT`); the test is skipped when absent so CI without
//! the plugin checkout stays green.
//!
//! Deliberate contract deltas vs the legacy wrappers (see the plugin README):
//! the artifact prefixes drop the `_container` suffix with the kind
//! (`/artifacts/lava`, `/artifacts/lava_scan`), the panel is the static
//! UKB EUR default (no `panel_id` param), and the port contract is the
//! dominant legacy shape (five inputs: input.info, loci, sample.overlap,
//! two sumstats; `sample_overlap = false` and other phenotype counts are
//! the recorded dynamic-ports gap).

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
    let manifest = root.join("lava").join("manifest.toml");
    if manifest.is_file() {
        return Some(root);
    }
    if explicit.is_some() {
        // fix-review R04 (2026-10-05): an explicitly set NODE_PLUGINS_ROOT
        // means migration parity was requested. A missing family must fail
        // loudly — early-returns here used to count as *passed* tests, so a
        // green summary claimed coverage that never ran.
        panic!(
            "NODE_PLUGINS_ROOT is set but the lava family is not deployed \
             under it ({}) — migration parity cannot run. Deploy the family \
             or unset NODE_PLUGINS_ROOT to skip these tests deliberately.",
            manifest.display()
        );
    }
    eprintln!("skipping: lava plugin directory not present (NODE_PLUGINS_ROOT unset)");
    None
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let text = std::fs::read_to_string(root.join("lava").join("manifest.toml")).unwrap();
    // Inline script_file the way the loader does.
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        if let Some(relative) = node.command.script_file.clone() {
            let source = std::fs::read_to_string(root.join("lava").join(&relative)).unwrap();
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
fn lava_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "lava");

    // The legacy wrapper's own default binding is the UKB EUR panel, and its
    // dominant shape is sample_overlap = true with two phenotypes; the
    // static contract bakes both, so `analysis` + `phenotypes` are the only
    // required submissions (the two params the legacy spec had without
    // defaults).
    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({"analysis": "bivar", "phenotypes": ["bmi", "depression"]}),
    )
    .unwrap();

    assert_eq!(
        compiled.image,
        "ghcr.io/auto-nomics/autonomics/lava@sha256:b7dd1d3a3493cc32af2dc286f2aace77036b1fd806bee4f14a4ff67509268be7"
    );
    assert_eq!(compiled.outputs.len(), 3);
    assert_eq!(compiled.outputs[0].path, "lava.tsv");
    assert_eq!(
        compiled.outputs[0].format.as_deref(),
        Some("lava_result_tsv")
    );
    assert_eq!(compiled.outputs[1].path, "lava.RDS");
    assert_eq!(compiled.outputs[1].format.as_deref(), Some("r_rds"));
    assert_eq!(compiled.outputs[2].path, "lava.log");
    assert_eq!(compiled.outputs[2].format.as_deref(), Some("lava_log"));
    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert!(matches!(
        compiled.pull_policy,
        container_runtime::PullPolicy::Missing
    ));
    assert_eq!(compiled.timeout_secs, 1800);
    // Kind suffix dropped with the wrapper name (ldsc precedent).
    assert_eq!(compiled.artifact_prefix, "/artifacts/lava");
    assert_eq!(compiled.workdir, None);
    assert!(compiled.panels.is_empty());
    // Exactly the legacy panel_bundles entry for the default panel:
    // catalog id plus the legacy mount path.
    assert_eq!(compiled.panel_bundles.len(), 1);
    assert_eq!(
        compiled.panel_bundles[0].panel_id,
        "wjixiang/catalog-lava-ref-ukb-eur"
    );
    assert_eq!(compiled.panel_bundles[0].mount_path, "/panels/lava_ref");
    assert_eq!(compiled.command, vec!["Rscript".to_string()]);
    assert_eq!(compiled.env.get("LAVA_ANALYSIS").unwrap(), "bivar");
    assert_eq!(compiled.env.get("LAVA_PHENOS").unwrap(), "bmi depression");
    assert_eq!(compiled.env.get("LAVA_TARGET").unwrap(), "");
    assert_eq!(compiled.env.get("LAVA_LOCUS_INDEX").unwrap(), "1");
    assert_eq!(compiled.env.get("LAVA_LOCUS_ID").unwrap(), "");
    // variances is a bool defaulting to false (value semantics, F02).
    assert_eq!(compiled.env.get("LAVA_VARIANCES").unwrap(), "false");
    assert_eq!(compiled.env.get("LAVA_ADAP_THRESH").unwrap(), "");
    assert_eq!(compiled.env.get("LAVA_P_VALUES").unwrap(), "true");
    assert_eq!(compiled.env.get("LAVA_CIS").unwrap(), "true");
    // only_full_model defaults to false and renders the literal so the
    // script's as.logical() never sees NA (F02); the surrounding empties
    // are optional strings/numbers on the null channel.
    assert_eq!(compiled.env.get("LAVA_ONLY_FULL_MODEL").unwrap(), "false");
    assert_eq!(compiled.env.get("LAVA_MAX_R2").unwrap(), "0.95");

    let script = compiled.script.as_deref().unwrap();
    // Semantic markers of the legacy script: the official API path, the
    // panel prefix the wrapper rendered for the UKB EUR panel, the
    // always-connected overlap port, and the input-count guard.
    assert!(script.contains("LAVA::process.input"));
    assert!(script.contains("LAVA::read.loci"));
    assert!(script.contains("LAVA::process.locus"));
    assert!(script.contains("ref.prefix = \"/panels/lava_ref/lava-ukb-v1.1\""));
    assert!(script.contains("sample.overlap.file = Sys.getenv(\"AUTONOMICS_INPUT2\")"));
    assert!(script.contains("as.integer(Sys.getenv(\"AUTONOMICS_INPUT_COUNT\"))"));
    // All four official analysis entry points stay in the dispatch; the
    // legacy wrapper picked one at Rust build time, the plugin script
    // switches on LAVA_ANALYSIS.
    assert!(script.contains("LAVA::run.univ("));
    assert!(script.contains("LAVA::run.bivar("));
    assert!(script.contains("LAVA::run.pcor("));
    assert!(script.contains("LAVA::run.multireg("));
    // Legacy validate() moved inside the container with its messages.
    assert!(script.contains("target must reference one of phenotypes"));
    assert!(script.contains("pcor target must contain exactly two distinct phenotypes"));
    assert!(script.contains("selected locus is outside the loci table"));
    assert!(script.contains("max_r2 must be finite and greater than zero"));
    // Outputs land on the same three ports (legacy variable name OUT0/1/2).
    assert!(script.contains("Sys.getenv(\"AUTONOMICS_OUTPUT0\")"));
    assert!(script.contains("Sys.getenv(\"AUTONOMICS_OUTPUT1\")"));
    assert!(script.contains("sink(Sys.getenv(\"AUTONOMICS_OUTPUT2\"), split = TRUE)"));
}

#[test]
fn lava_plugin_renders_submitted_values_into_env() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "lava");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({
            "analysis": "pcor",
            "phenotypes": ["bmi", "depression"],
            "target": ["depression", "bmi"],
            "locus_id": "230",
            "adap_thresh": ["0.0001", "1e-06"],
            "max_r2": 0.9,
        }),
    )
    .unwrap();

    // String arrays render space-joined; the script re-splits with
    // strsplit(..., fixed = TRUE). Legacy rendered
    // c("bmi", "depression") straight into the R source.
    assert_eq!(compiled.env.get("LAVA_PHENOS").unwrap(), "bmi depression");
    assert_eq!(compiled.env.get("LAVA_TARGET").unwrap(), "depression bmi");
    assert_eq!(
        compiled.env.get("LAVA_ADAP_THRESH").unwrap(),
        "0.0001 1e-06"
    );
    assert_eq!(compiled.env.get("LAVA_LOCUS_ID").unwrap(), "230");
    assert_eq!(compiled.env.get("LAVA_LOCUS_INDEX").unwrap(), "1");
    // serde_json renders f64 0.9 as "0.9".
    assert_eq!(compiled.env.get("LAVA_MAX_R2").unwrap(), "0.9");
}

#[test]
fn lava_plugin_schema_marks_the_two_required_params_and_array_bounds() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "lava");

    let schema =
        serde_json::to_value(container_plugin::compile::compile_schema(&node.params)).unwrap();
    // The legacy spec's two non-defaulted fields.
    let required = schema["required"].as_array().unwrap();
    assert_eq!(required, &vec![json!("analysis"), json!("phenotypes")]);
    // phenotypes is pinned to the two static sumstats ports.
    assert_eq!(schema["properties"]["phenotypes"]["minItems"], 2.0);
    assert_eq!(schema["properties"]["phenotypes"]["maxItems"], 2.0);
    assert_eq!(schema["properties"]["locus_index"]["minimum"], 1.0);
    assert_eq!(schema["properties"]["max_r2"]["exclusiveMinimum"], 0.0);
}

#[test]
fn lava_scan_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "lava_scan");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({"phenotypes": ["depression", "neuro"]}),
    )
    .unwrap();

    assert_eq!(
        compiled.image,
        "ghcr.io/auto-nomics/autonomics/lava@sha256:b7dd1d3a3493cc32af2dc286f2aace77036b1fd806bee4f14a4ff67509268be7"
    );
    assert_eq!(compiled.outputs.len(), 4);
    assert_eq!(compiled.outputs[0].path, "lava_scan.univ.tsv");
    assert_eq!(
        compiled.outputs[0].format.as_deref(),
        Some("lava_univ_result_tsv")
    );
    assert_eq!(compiled.outputs[1].path, "lava_scan.bivar.tsv");
    assert_eq!(
        compiled.outputs[1].format.as_deref(),
        Some("lava_bivar_result_tsv")
    );
    assert_eq!(compiled.outputs[2].path, "lava_scan.RDS");
    assert_eq!(compiled.outputs[2].format.as_deref(), Some("r_rds"));
    assert_eq!(compiled.outputs[3].path, "lava_scan.log");
    assert_eq!(compiled.outputs[3].format.as_deref(), Some("lava_scan_log"));
    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert_eq!(compiled.timeout_secs, 86_400);
    assert_eq!(compiled.artifact_prefix, "/artifacts/lava_scan");
    assert_eq!(compiled.panel_bundles.len(), 1);
    assert_eq!(
        compiled.panel_bundles[0].panel_id,
        "wjixiang/catalog-lava-ref-ukb-eur"
    );
    assert_eq!(compiled.panel_bundles[0].mount_path, "/panels/lava_ref");
    assert_eq!(compiled.command, vec!["Rscript".to_string()]);
    assert_eq!(
        compiled.env.get("LAVA_SCAN_PHENOS").unwrap(),
        "depression neuro"
    );
    assert_eq!(compiled.env.get("LAVA_SCAN_TARGET").unwrap(), "");
    assert_eq!(compiled.env.get("LAVA_SCAN_LOCUS_IDS").unwrap(), "");
    assert_eq!(compiled.env.get("LAVA_SCAN_CHR").unwrap(), "");
    assert_eq!(
        compiled.env.get("LAVA_SCAN_UNIV_THRESHOLD").unwrap(),
        "0.05"
    );
    assert_eq!(compiled.env.get("LAVA_SCAN_ADAP_THRESH").unwrap(), "");
    assert_eq!(compiled.env.get("LAVA_SCAN_P_VALUES").unwrap(), "true");
    assert_eq!(compiled.env.get("LAVA_SCAN_CIS").unwrap(), "true");

    let script = compiled.script.as_deref().unwrap();
    // The official multiple-locus workflow stays script-side:
    // one process.input, then the per-locus loop.
    assert_eq!(script.matches("LAVA::process.input").count(), 1);
    assert!(script.contains("for (row_index in seq_len(nrow(loci)))"));
    assert!(script.contains("LAVA::process.locus"));
    assert!(script.contains("LAVA::run.univ.bivar"));
    assert!(script.contains("param.lim = 1.25"));
    assert!(script.contains("ref.prefix = \"/panels/lava_ref/lava-ukb-v1.1\""));
    assert!(script.contains("sample.overlap.file = Sys.getenv(\"AUTONOMICS_INPUT2\")"));
    assert!(script.contains("as.integer(Sys.getenv(\"AUTONOMICS_INPUT_COUNT\"))"));
    // The four scan outputs land on ports 0-3 (legacy variable names).
    assert!(script.contains("write.table(univ, Sys.getenv(\"AUTONOMICS_OUTPUT0\")"));
    assert!(script.contains("write.table(bivar, Sys.getenv(\"AUTONOMICS_OUTPUT1\")"));
    assert!(script.contains("Sys.getenv(\"AUTONOMICS_OUTPUT2\")"));
    assert!(script.contains("sink(Sys.getenv(\"AUTONOMICS_OUTPUT3\"), split = TRUE)"));
    // Locus-failure logging matches the official batch workflow.
    assert!(script.contains("Error processing locus "));
    assert!(script.contains("Error analyzing locus "));
}

#[test]
fn lava_scan_plugin_renders_submitted_values_into_env() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "lava_scan");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({
            "phenotypes": ["depression", "neuro"],
            "target": ["depression"],
            "locus_ids": ["100", "230"],
            "chr": 7,
            "univ_threshold": 0.1,
        }),
    )
    .unwrap();

    assert_eq!(
        compiled.env.get("LAVA_SCAN_PHENOS").unwrap(),
        "depression neuro"
    );
    assert_eq!(compiled.env.get("LAVA_SCAN_TARGET").unwrap(), "depression");
    assert_eq!(compiled.env.get("LAVA_SCAN_LOCUS_IDS").unwrap(), "100 230");
    assert_eq!(compiled.env.get("LAVA_SCAN_CHR").unwrap(), "7");
    assert_eq!(compiled.env.get("LAVA_SCAN_UNIV_THRESHOLD").unwrap(), "0.1");
}

#[test]
fn lava_scan_plugin_schema_marks_phenotypes_required_with_bounds() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "lava_scan");

    let schema =
        serde_json::to_value(container_plugin::compile::compile_schema(&node.params)).unwrap();
    let required = schema["required"].as_array().unwrap();
    assert_eq!(required, &vec![json!("phenotypes")]);
    assert_eq!(schema["properties"]["phenotypes"]["minItems"], 2.0);
    assert_eq!(schema["properties"]["phenotypes"]["maxItems"], 2.0);
    assert_eq!(schema["properties"]["chr"]["minimum"], 1.0);
    assert_eq!(schema["properties"]["chr"]["maximum"], 23.0);
    assert_eq!(
        schema["properties"]["univ_threshold"]["exclusiveMinimum"],
        0.0
    );
    assert_eq!(schema["properties"]["univ_threshold"]["maximum"], 1.0);
}
