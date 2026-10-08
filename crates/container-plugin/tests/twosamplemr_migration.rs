//! Golden parity tests for the twosamplemr plugin family: each `[[nodes]]`
//! entry (`twosamplemr`, `twosamplemr_harmonise`) must compile to the same
//! `Container` spec the legacy Rust wrappers (`nodes-io/src/
//! twosamplemr_container.rs` and `twosamplemr_harmonise_container.rs`)
//! produced. The plugin directory lives outside this repository
//! (`/mnt/projects/node-plugins/twosamplemr` by default, overridable via
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
    let manifest = root.join("twosamplemr").join("manifest.toml");
    if manifest.is_file() {
        return Some(root);
    }
    if explicit.is_some() {
        // fix-review R04 (2026-10-05): an explicitly set NODE_PLUGINS_ROOT
        // means migration parity was requested. A missing family must fail
        // loudly — early-returns here used to count as *passed* tests, so a
        // green summary claimed coverage that never ran.
        panic!(
            "NODE_PLUGINS_ROOT is set but the twosamplemr family is not deployed \
             under it ({}) — migration parity cannot run. Deploy the family \
             or unset NODE_PLUGINS_ROOT to skip these tests deliberately.",
            manifest.display()
        );
    }
    eprintln!("skipping: twosamplemr plugin directory not present (NODE_PLUGINS_ROOT unset)");
    None
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let text = std::fs::read_to_string(root.join("twosamplemr").join("manifest.toml")).unwrap();
    // Inline script_file the way the loader does.
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        if let Some(relative) = node.command.script_file.clone() {
            let source = std::fs::read_to_string(root.join("twosamplemr").join(&relative)).unwrap();
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

/// The minimal valid harmonise spec, mirroring the legacy wrapper's unit
/// test fixture. The four labels are required params (no serde default)
/// on BOTH legacy specs, so neither contract test can compile from an
/// empty value set.
fn harmonise_values() -> serde_json::Value {
    json!({
        "id_exposure": "ieu-a-2",
        "exposure": "Body mass index",
        "id_outcome": "ieu-a-7",
        "outcome": "Coronary heart disease",
    })
}

/// Same four labels for the main node: `TwoSampleMrContainerSpec` carries
/// them without serde defaults, exactly like the harmonise spec.
fn main_values() -> serde_json::Value {
    harmonise_values()
}

#[test]
fn twosamplemr_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "twosamplemr");

    let compiled =
        compile_container_spec(node, &manifest.image, &manifest.panels, &main_values()).unwrap();

    // Byte-exact contract: image, outputs, panel bundle, resources,
    // timeout, prefix.
    assert_eq!(
        compiled.image,
        "ghcr.io/auto-nomics/autonomics/twosamplemr@sha256:c270de9978906ee48cbba2ac484ba3df9e86dddc69efd908c6f908963114002a"
    );
    assert_eq!(compiled.outputs.len(), 4);
    assert_eq!(compiled.outputs[0].path, "twosamplemr_results.tsv");
    assert_eq!(compiled.outputs[0].format.as_deref(), Some("tsv"));
    assert_eq!(compiled.outputs[1].path, "twosamplemr_harmonised.tsv");
    assert_eq!(compiled.outputs[1].format.as_deref(), Some("tsv"));
    assert_eq!(compiled.outputs[2].path, "twosamplemr.RDS");
    assert_eq!(compiled.outputs[2].format.as_deref(), Some("r_rds"));
    assert_eq!(compiled.outputs[3].path, "twosamplemr.log");
    assert_eq!(
        compiled.outputs[3].format.as_deref(),
        Some("twosamplemr_log")
    );
    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert!(matches!(
        compiled.pull_policy,
        container_runtime::PullPolicy::Missing
    ));
    assert_eq!(compiled.timeout_secs, 1800);
    // Deliberate delta: the legacy default was "/artifacts/
    // twosamplemr_container"; the plugin kind drops the `_container`
    // suffix and the prefix follows the kind (the wave-wide
    // `/artifacts/{kind}` convention, same as ldsc/plink2/coloc).
    assert_eq!(
        compiled.artifact_prefix,
        Some("/artifacts/twosamplemr".to_string())
    );
    assert_eq!(compiled.workdir, None);
    assert!(compiled.panels.is_empty());
    // The plink2 clumping stage mounts the catalog panel the legacy
    // wrapper pinned as PLINK2_REF_BINARY_PANEL.
    assert_eq!(compiled.panel_bundles.len(), 1);
    assert_eq!(
        compiled.panel_bundles[0].panel_id,
        "wjixiang/catalog-plink-ref-1000g-eur-binary"
    );
    assert_eq!(compiled.panel_bundles[0].mount_path, "/panels/plink_ref");
    // Deliberate delta: the legacy command was vec!["sh", "-c"]; the
    // manifest carries the interpreter alone and the executor inserts the
    // materialized script at index 1 (the trailing "-c" was a no-op
    // positional under `sh <script>`, same as the plink2 migration).
    assert_eq!(compiled.command, vec!["sh".to_string()]);
    assert!(compiled.files.is_empty());
    // The legacy wrapper shipped an empty env map and baked values into
    // the R source; env is the plugin's canonical param channel.
    assert_eq!(
        compiled.env.get("TWOSAMPLEMR_METHOD_LIST").unwrap(),
        "mr_egger_regression mr_weighted_median mr_ivw mr_simple_mode mr_weighted_mode"
    );
    assert_eq!(
        compiled.env.get("TWOSAMPLEMR_HARMONISE_ACTION").unwrap(),
        "2"
    );
    assert_eq!(compiled.env.get("TWOSAMPLEMR_CLUMP").unwrap(), "true");
    // serde_json/ryu spellings of the legacy defaults (the legacy script
    // rendered the p thresholds via `{:.0e}` as `5e-08`/`1e-06`; equal
    // after float parsing).
    assert_eq!(compiled.env.get("TWOSAMPLEMR_CLUMP_P1").unwrap(), "5e-8");
    assert_eq!(compiled.env.get("TWOSAMPLEMR_CLUMP_P2").unwrap(), "1e-6");
    assert_eq!(compiled.env.get("TWOSAMPLEMR_CLUMP_R2").unwrap(), "0.001");
    assert_eq!(compiled.env.get("TWOSAMPLEMR_CLUMP_KB").unwrap(), "10000");
    // Optional `chr` renders empty: the script falls back to autosomes
    // 1..=22.
    assert_eq!(compiled.env.get("TWOSAMPLEMR_CHR").unwrap(), "");

    // Semantic script markers, not byte equality: the plugin drives the
    // hybrid shell + R script through env instead of Rust format!
    // string-building, so layout and variable names differ from the
    // legacy generated script.
    let script = compiled.script.as_deref().unwrap();
    // PLINK2 clumping stage keeps the official tokens.
    assert!(script.contains("--bfile /panels/plink_ref/1000G.EUR.QC.${chr}"));
    assert!(script.contains("--clump /work/clump_input.tsv"));
    assert!(script.contains("--clump-id-field SNP --clump-p-field P"));
    assert!(script.contains("--clump-p1 \"${TWOSAMPLEMR_CLUMP_P1}\""));
    assert!(script.contains("--clump-p2 \"${TWOSAMPLEMR_CLUMP_P2}\""));
    assert!(script.contains("--clump-r2 \"${TWOSAMPLEMR_CLUMP_R2}\""));
    assert!(script.contains("--clump-kb \"${TWOSAMPLEMR_CLUMP_KB}\""));
    assert!(script.contains("--threads 1"));
    assert!(script.contains("seq $CHR_SEQ"));
    assert!(script.contains("mkdir -p /work/per_chr"));
    assert!(script.contains("if [ -n \"$TWOSAMPLEMR_CHR\" ]; then"));
    assert!(script.contains("--chr $TWOSAMPLEMR_CHR"));
    assert!(script.contains(">>\"${AUTONOMICS_OUTPUT3}\" 2>&1"));
    assert!(script.contains("cat \"$out.clumps\" >> /work/all.clumps"));
    // The optional single-chromosome and the clump on/off gates are
    // static script logic keyed on the env values.
    assert!(script.contains("if [ \"$TWOSAMPLEMR_CLUMP\" = \"true\" ]; then"));
    // The p-value derivation and prep stanza are byte-identical to the
    // legacy stage.
    assert!(
        script
            .contains("Rscript --vanilla -e 'data <- read.delim(Sys.getenv(\"AUTONOMICS_INPUT0\")")
    );
    assert!(script.contains("2 * pnorm(-abs(data$beta_exposure / data$se_exposure))"));
    // The R stage keeps the official TwoSampleMR calls; labels travel
    // through env where the legacy source baked Rust-quoted literals.
    assert!(script.contains("TwoSampleMR::harmonise_data"));
    assert!(script.contains("TwoSampleMR::mr"));
    assert!(script.contains(
        "harmonised <- TwoSampleMR::harmonise_data(exposure_dat, outcome_dat, action = as.numeric(Sys.getenv(\"TWOSAMPLEMR_HARMONISE_ACTION\")))"
    ));
    assert!(script.contains("estimates <- TwoSampleMR::mr(harmonised, method_list = method_list)"));
    assert!(script.contains("read.delim(\"/work/all.clumps\""));
    assert!(
        script.contains(
            "exposure_dat <- exposure_dat[exposure_dat$SNP %in% selected, , drop = FALSE]"
        )
    );
    // The SUPPORTED_METHODS membership gate moves into the R stage (the
    // v0 param DSL has no enum type).
    assert!(script.contains("unsupported TwoSampleMR method"));
    // The cross-field clump_p2 >= clump_p1 rule moves into the shell
    // stage.
    assert!(script.contains("clump_p2 must be greater than or equal to clump_p1"));
    // Log epilogue matches the legacy plumbing (truncate, plink2 appends,
    // R sink() appends).
    assert!(script.contains(": > \"${AUTONOMICS_OUTPUT3}\""));
    assert!(script.contains("sink(log_path, split = TRUE, append = TRUE)"));
    assert!(script.contains("saveRDS(result, result_path)"));
    // The legacy wrapper's own unit test asserted the heredoc epilogue.
    assert!(script.ends_with("RSCRIPT\n"));
    // No gzip stanza on purpose: the legacy wrapper never decompressed
    // inputs (read.delim reads the table verbatim).
    assert!(!script.contains("gzip"));
    // The renderer must leave the script untouched: no template markers,
    // every quote region stays balanced.
    assert!(!script.contains("{{"));
}

#[test]
fn twosamplemr_plugin_renders_submitted_values_into_env() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "twosamplemr");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({
            "id_exposure": "ieu-a-2",
            "exposure": "Body mass index",
            "id_outcome": "ieu-a-7",
            "outcome": "Coronary heart disease",
            "method_list": ["mr_ivw", "mr_wald_ratio"],
            "harmonise_action": 3,
            "clump": false,
            "clump_p1": 1e-7,
            "clump_p2": 2e-6,
            "clump_r2": 0.05,
            "clump_kb": 25000,
            "chr": 6
        }),
    )
    .unwrap();

    // Labels travel verbatim through env (the legacy wrapper Rust-quoted
    // them into the R source).
    assert_eq!(
        compiled.env.get("TWOSAMPLEMR_ID_EXPOSURE").unwrap(),
        "ieu-a-2"
    );
    assert_eq!(
        compiled.env.get("TWOSAMPLEMR_EXPOSURE").unwrap(),
        "Body mass index"
    );
    // The env renderer space-joins string arrays; the R stage splits on
    // the single space (official method names never contain spaces).
    assert_eq!(
        compiled.env.get("TWOSAMPLEMR_METHOD_LIST").unwrap(),
        "mr_ivw mr_wald_ratio"
    );
    assert_eq!(
        compiled.env.get("TWOSAMPLEMR_HARMONISE_ACTION").unwrap(),
        "3"
    );
    assert_eq!(compiled.env.get("TWOSAMPLEMR_CLUMP").unwrap(), "false");
    assert_eq!(compiled.env.get("TWOSAMPLEMR_CLUMP_P1").unwrap(), "1e-7");
    assert_eq!(compiled.env.get("TWOSAMPLEMR_CLUMP_P2").unwrap(), "2e-6");
    assert_eq!(compiled.env.get("TWOSAMPLEMR_CLUMP_R2").unwrap(), "0.05");
    assert_eq!(compiled.env.get("TWOSAMPLEMR_CLUMP_KB").unwrap(), "25000");
    assert_eq!(compiled.env.get("TWOSAMPLEMR_CHR").unwrap(), "6");

    // The script itself is static: the gates live in the script, not in
    // the rendered text.
    let script = compiled.script.as_deref().unwrap();
    assert!(script.contains("if [ \"$TWOSAMPLEMR_CLUMP\" = \"true\" ]; then"));
    assert!(script.contains("--chr $TWOSAMPLEMR_CHR"));
}

#[test]
fn twosamplemr_harmonise_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "twosamplemr_harmonise");

    let compiled =
        compile_container_spec(node, &manifest.image, &manifest.panels, &harmonise_values())
            .unwrap();

    // Byte-exact contract.
    assert_eq!(
        compiled.image,
        "ghcr.io/auto-nomics/autonomics/twosamplemr@sha256:c270de9978906ee48cbba2ac484ba3df9e86dddc69efd908c6f908963114002a"
    );
    assert_eq!(compiled.outputs.len(), 2);
    assert_eq!(compiled.outputs[0].path, "harmonised.tsv");
    assert_eq!(compiled.outputs[0].format.as_deref(), Some("tsv"));
    assert_eq!(compiled.outputs[1].path, "harmonise.log");
    assert_eq!(
        compiled.outputs[1].format.as_deref(),
        Some("twosamplemr_log")
    );
    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert!(matches!(
        compiled.pull_policy,
        container_runtime::PullPolicy::Missing
    ));
    assert_eq!(compiled.timeout_secs, 600);
    // Same `_container`-suffix drop as the main kind.
    assert_eq!(
        compiled.artifact_prefix,
        Some("/artifacts/twosamplemr_harmonise".to_string())
    );
    assert_eq!(compiled.workdir, None);
    assert!(compiled.panels.is_empty());
    // Deliberate delta (not parity): the legacy harmonise wrapper
    // attached NO panels (panel_bundles: Vec::new()), but the v0 panel
    // DSL is family-scoped, so the shared [[panels]] entry propagates to
    // every node — the ldsc_munge propagation precedent, inverted. The
    // extra read-only mount is unused by the script, but DAGs using this
    // kind must now bind the plink_ref bundle.
    assert_eq!(compiled.panel_bundles.len(), 1);
    assert_eq!(
        compiled.panel_bundles[0].panel_id,
        "wjixiang/catalog-plink-ref-1000g-eur-binary"
    );
    assert_eq!(compiled.panel_bundles[0].mount_path, "/panels/plink_ref");
    // The legacy command vec!["Rscript"] is reproduced byte-for-byte: the
    // interpreter executes the materialized script file directly, exactly
    // like the legacy script insertion.
    assert_eq!(compiled.command, vec!["Rscript".to_string()]);
    // Defaults: action 2, units unset (render empty).
    assert_eq!(
        compiled.env.get("TWOSAMPLEMR_HARMONISE_ACTION").unwrap(),
        "2"
    );
    assert_eq!(compiled.env.get("TWOSAMPLEMR_UNITS_EXPOSURE").unwrap(), "");
    assert_eq!(compiled.env.get("TWOSAMPLEMR_UNITS_OUTCOME").unwrap(), "");
    assert_eq!(
        compiled.env.get("TWOSAMPLEMR_ID_EXPOSURE").unwrap(),
        "ieu-a-2"
    );

    // Semantic script markers (not byte equality): the plugin drives the
    // R source through env instead of Rust __PLACEHOLDER__ substitution.
    let script = compiled.script.as_deref().unwrap();
    assert!(script.contains("exp_path <- Sys.getenv(\"AUTONOMICS_INPUT0\")"));
    assert!(script.contains("out_path <- Sys.getenv(\"AUTONOMICS_INPUT1\")"));
    assert!(script.contains("harm_path <- Sys.getenv(\"AUTONOMICS_OUTPUT0\")"));
    assert!(script.contains("log_path <- Sys.getenv(\"AUTONOMICS_OUTPUT1\")"));
    assert!(script.contains("sink(log_path, split = TRUE)"));
    assert!(
        script.contains(
            "cat(\"TwoSampleMR:\", as.character(packageVersion(\"TwoSampleMR\")), \"\\n\")"
        )
    );
    assert!(script.contains("exp <- TwoSampleMR::read_exposure_data(exp_path)"));
    assert!(script.contains("out <- TwoSampleMR::read_outcome_data(out_path)"));
    assert!(script.contains(
        "harm <- TwoSampleMR::harmonise_data(exp, out, action = as.numeric(Sys.getenv(\"TWOSAMPLEMR_HARMONISE_ACTION\")))"
    ));
    // The 0-row guard and the snake_case rewrite keep the legacy messages
    // and shapes; the downstream wrapper reads data$snp (lowercase) while
    // harmonise_data emits SNP, so the rewrite must cover both cases.
    assert!(script.contains(
        "if (nrow(harm) == 0) stop(\"harmonise_data produced 0 rows; check SNP overlap between exposure and outcome\")"
    ));
    assert!(script.contains("names(harm)[names(harm) == \"SNP\"] <- \"snp\""));
    assert!(script.contains("gsub(\"\\\\.\", \"_\""));
    assert!(
        script.contains(
            "write.table(harm, harm_path, sep = \"\\t\", quote = FALSE, row.names = FALSE)"
        )
    );
    // The label-emptiness rules the legacy validate() enforced before the
    // container started are mirrored inside the script.
    assert!(script.contains("id_exposure cannot be empty"));
    assert!(script.contains("outcome cannot be empty"));
    // Optional units labels are gated by nzchar(), the R equivalent of
    // the legacy Option<String> branches.
    assert!(script.contains("nzchar(Sys.getenv(\"TWOSAMPLEMR_UNITS_EXPOSURE\"))"));
    assert!(script.contains("exp$units.exposure <- Sys.getenv(\"TWOSAMPLEMR_UNITS_EXPOSURE\")"));
    assert!(script.contains("out$units.outcome <- Sys.getenv(\"TWOSAMPLEMR_UNITS_OUTCOME\")"));
    // No gzip stanza: the legacy wrapper never decompressed inputs.
    assert!(!script.contains("gzip"));
    assert!(!script.contains("{{"));
}

#[test]
fn twosamplemr_harmonise_plugin_renders_submitted_values_into_env() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "twosamplemr_harmonise");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({
            "id_exposure": "bbj-a-100",
            "exposure": "Height",
            "id_outcome": "ieu-a-7",
            "outcome": "Coronary heart disease",
            "harmonise_action": 1,
            "units_exposure": "SD",
            "units_outcome": "log odds"
        }),
    )
    .unwrap();

    assert_eq!(
        compiled.env.get("TWOSAMPLEMR_ID_EXPOSURE").unwrap(),
        "bbj-a-100"
    );
    assert_eq!(compiled.env.get("TWOSAMPLEMR_EXPOSURE").unwrap(), "Height");
    assert_eq!(
        compiled.env.get("TWOSAMPLEMR_HARMONISE_ACTION").unwrap(),
        "1"
    );
    assert_eq!(
        compiled.env.get("TWOSAMPLEMR_UNITS_EXPOSURE").unwrap(),
        "SD"
    );
    assert_eq!(
        compiled.env.get("TWOSAMPLEMR_UNITS_OUTCOME").unwrap(),
        "log odds"
    );

    // The units stamping gates are static: the submitted labels travel
    // through env, the nzchar() branches never change shape.
    let script = compiled.script.as_deref().unwrap();
    assert!(script.contains("exp$units.exposure <- Sys.getenv(\"TWOSAMPLEMR_UNITS_EXPOSURE\")"));
    assert!(script.contains("out$units.outcome <- Sys.getenv(\"TWOSAMPLEMR_UNITS_OUTCOME\")"));
}

#[test]
fn twosamplemr_plugin_schema_and_bounds_reject_the_legacy_invalid_specs() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "twosamplemr");
    let harmonise = node_by_kind(&manifest, "twosamplemr_harmonise");

    // Main node: the four labels were required (no serde default) on
    // TwoSampleMrContainerSpec; every other field had a default or was
    // Option.
    let schema =
        serde_json::to_value(container_plugin::compile::compile_schema(&node.params)).unwrap();
    let required = schema["required"].as_array().unwrap();
    assert_eq!(
        required
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["exposure", "id_exposure", "id_outcome", "outcome"]
    );
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["properties"]["clump_p1"]["type"], "number");
    assert_eq!(schema["properties"]["clump_p1"]["exclusiveMinimum"], 0.0);
    assert_eq!(schema["properties"]["clump_p1"]["maximum"], 1.0);
    assert_eq!(schema["properties"]["clump_p2"]["default"], json!(1e-6));
    assert_eq!(schema["properties"]["clump_kb"]["type"], "integer");
    assert_eq!(schema["properties"]["clump_kb"]["exclusiveMinimum"], 0.0);
    assert_eq!(schema["properties"]["chr"]["type"], "integer");
    assert_eq!(schema["properties"]["chr"]["minimum"], 1.0);
    assert_eq!(schema["properties"]["chr"]["maximum"], 22.0);
    assert_eq!(schema["properties"]["harmonise_action"]["minimum"], 1.0);
    assert_eq!(schema["properties"]["harmonise_action"]["maximum"], 3.0);
    assert_eq!(schema["properties"]["clump"]["type"], "boolean");
    assert_eq!(schema["properties"]["clump"]["default"], json!(true));
    // method_list keeps the legacy default order and the non-empty rule.
    assert_eq!(schema["properties"]["method_list"]["type"], "array");
    assert_eq!(
        schema["properties"]["method_list"]["items"]["type"],
        "string"
    );
    assert_eq!(schema["properties"]["method_list"]["minItems"], 1.0);
    assert_eq!(
        schema["properties"]["method_list"]["default"],
        json!([
            "mr_egger_regression",
            "mr_weighted_median",
            "mr_ivw",
            "mr_simple_mode",
            "mr_weighted_mode"
        ])
    );

    // Harmonise node: the four labels were required (no serde default)
    // in TwoSampleMrHarmoniseContainerSpec.
    let harmonise_schema =
        serde_json::to_value(container_plugin::compile::compile_schema(&harmonise.params)).unwrap();
    let required = harmonise_schema["required"].as_array().unwrap();
    assert_eq!(
        required
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["exposure", "id_exposure", "id_outcome", "outcome"]
    );
    assert_eq!(harmonise_schema["additionalProperties"], false);
    assert_eq!(
        harmonise_schema["properties"]["units_exposure"]["type"],
        "string"
    );

    // Mirrors the legacy wrapper test `rejects_unsupported_method_and_
    // bad_thresholds` bounds half: clump thresholds outside (0, 1] are
    // rejected at compile time.
    let mut values = main_values();
    values["clump_p1"] = json!(1.5);
    let error = compile_container_spec(node, &manifest.image, &manifest.panels, &values)
        .unwrap_err()
        .to_string();
    assert!(error.contains("clump_p1"), "{error}");
    assert!(error.contains("maximum"), "{error}");

    // harmonise_action must be 1, 2, or 3 (legacy validate()).
    values = main_values();
    values["harmonise_action"] = json!(4);
    let error = compile_container_spec(node, &manifest.image, &manifest.panels, &values)
        .unwrap_err()
        .to_string();
    assert!(error.contains("harmonise_action"), "{error}");
    assert!(error.contains("maximum"), "{error}");

    // method_list cannot be empty (legacy validate()); min_len = 1 now
    // rejects it at compile time. Membership still fails inside the
    // container (no enum param type).
    values = main_values();
    values["method_list"] = json!([]);
    let error = compile_container_spec(node, &manifest.image, &manifest.panels, &values)
        .unwrap_err()
        .to_string();
    assert!(error.contains("method_list"), "{error}");
    assert!(error.contains("minItems"), "{error}");

    // chr must lie in 1..=22 when specified (legacy validate()).
    values = main_values();
    values["chr"] = json!(23);
    let error = compile_container_spec(node, &manifest.image, &manifest.panels, &values)
        .unwrap_err()
        .to_string();
    assert!(error.contains("chr"), "{error}");
    assert!(error.contains("maximum"), "{error}");

    // The legacy spec exposed artifact_prefix/timeout_secs as
    // per-instance overridable fields with defaults; the plugin DSL bakes
    // them as fixed node-level manifest values, so submitting them fails
    // the same additionalProperties:false gate the compiled schema
    // enforces.
    let error = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({"artifact_prefix": "/artifacts/other"}),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("unknown param `artifact_prefix`"), "{error}");

    // Mirrors the legacy serde enforcement of the required harmonise
    // labels: an absent label is a MissingParam.
    let error = compile_container_spec(
        harmonise,
        &manifest.image,
        &manifest.panels,
        &json!({"id_exposure": "ieu-a-2"}),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("exposure"), "{error}");
    assert!(error.contains("missing required param"), "{error}");
}
