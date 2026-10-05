//! Golden parity tests for the coloc plugin family: the `coloc_abf`
//! `[[nodes]]` entry must compile to the same `Container` spec the legacy
//! Rust wrapper (`nodes-io/src/coloc_abf_container.rs`) produced. The
//! plugin directory lives outside this repository
//! (`/mnt/projects/node-plugins/coloc` by default, overridable via
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
    let manifest = root.join("coloc").join("manifest.toml");
    if manifest.is_file() {
        return Some(root);
    }
    if explicit.is_some() {
        // fix-review R04 (2026-10-05): an explicitly set NODE_PLUGINS_ROOT
        // means migration parity was requested. A missing family must fail
        // loudly — early-returns here used to count as *passed* tests, so a
        // green summary claimed coverage that never ran.
        panic!(
            "NODE_PLUGINS_ROOT is set but the coloc family is not deployed \
             under it ({}) — migration parity cannot run. Deploy the family \
             or unset NODE_PLUGINS_ROOT to skip these tests deliberately.",
            manifest.display()
        );
    }
    eprintln!("skipping: coloc plugin directory not present (NODE_PLUGINS_ROOT unset)");
    None
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let text = std::fs::read_to_string(root.join("coloc").join("manifest.toml")).unwrap();
    // Inline script_file the way the loader does.
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        if let Some(relative) = node.command.script_file.clone() {
            let source = std::fs::read_to_string(root.join("coloc").join(&relative)).unwrap();
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

/// The minimal valid submitted spec: both datasets on the beta/varbeta
/// path. Mirrors the legacy wrapper unit-test fixture.
fn base_values() -> serde_json::Value {
    json!({
        "dataset1_type": "quant",
        "dataset1_snp": "snp",
        "dataset1_beta": "beta1",
        "dataset1_varbeta": "varbeta1",
        "dataset2_type": "quant",
        "dataset2_snp": "snp",
        "dataset2_beta": "beta2",
        "dataset2_varbeta": "varbeta2",
    })
}

#[test]
fn coloc_abf_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "coloc_abf");

    let compiled =
        compile_container_spec(node, &manifest.image, &manifest.panels, &base_values()).unwrap();

    assert_eq!(
        compiled.image,
        "ghcr.io/auto-nomics/autonomics/coloc@sha256:a2afe7aa83ae6f1ecb9573317aa022af4db0870771378c7fe90d295d70057322"
    );
    assert_eq!(compiled.outputs.len(), 2);
    assert_eq!(compiled.outputs[0].path, "coloc_abf.RDS");
    assert_eq!(compiled.outputs[0].format.as_deref(), Some("r_rds"));
    assert_eq!(compiled.outputs[1].path, "coloc_abf.log");
    assert_eq!(compiled.outputs[1].format.as_deref(), Some("coloc_abf_log"));
    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert!(matches!(
        compiled.pull_policy,
        container_runtime::PullPolicy::Missing
    ));
    // Legacy DEFAULT_TIMEOUT_SECS / DEFAULT_ARTIFACT_PREFIX. The prefix
    // drops the `_container` suffix along with the kind, per the
    // migration convention.
    assert_eq!(compiled.timeout_secs, 600);
    assert_eq!(compiled.artifact_prefix, "/artifacts/coloc_abf");
    assert_eq!(compiled.workdir, None);
    // coloc.abf needs no reference panel: the legacy wrapper attached
    // neither direct panels nor catalog bundles, and neither does the
    // manifest (the family omits [[panels]] entirely).
    assert!(compiled.panels.is_empty());
    assert!(compiled.panel_bundles.is_empty());
    assert!(compiled.files.is_empty());
    // Legacy command was vec!["Rscript".into()]; the interpreter field
    // reproduces it byte-for-byte.
    assert_eq!(compiled.command, vec!["Rscript".to_string()]);

    // Env is the plugin's canonical param channel (the legacy wrapper
    // baked values into the R source and shipped an empty env map).
    // Priors keep the coloc.abf defaults 1e-4/1e-4/1e-5; ryu renders
    // both 1e-4 and 1e-5 in decimal notation (the scientific branch
    // only starts at kk <= -5, i.e. values below 1e-5).
    assert_eq!(compiled.env.get("COLOC_P1").unwrap(), "0.0001");
    assert_eq!(compiled.env.get("COLOC_P2").unwrap(), "0.0001");
    assert_eq!(compiled.env.get("COLOC_P12").unwrap(), "0.00001");
    assert_eq!(compiled.env.get("COLOC_DATASET1_BETA").unwrap(), "beta1");
    assert_eq!(
        compiled.env.get("COLOC_DATASET1_VARBETA").unwrap(),
        "varbeta1"
    );
    assert_eq!(compiled.env.get("COLOC_DATASET2_BETA").unwrap(), "beta2");
    // Optional-and-absent params render as empty strings.
    assert_eq!(compiled.env.get("COLOC_DATASET1_PVALUES").unwrap(), "");
    assert_eq!(compiled.env.get("COLOC_DATASET1_MAF").unwrap(), "");
    assert_eq!(compiled.env.get("COLOC_DATASET1_N").unwrap(), "");
    assert_eq!(compiled.env.get("COLOC_DATASET1_S").unwrap(), "");
    assert_eq!(compiled.env.get("COLOC_DATASET1_SD_Y").unwrap(), "");
    assert_eq!(compiled.env.get("COLOC_DATASET2_S").unwrap(), "");

    let script = compiled.script.as_deref().unwrap();
    // Semantic markers, not byte equality: the plugin drives the R script
    // through env instead of Rust __PLACEHOLDER__ substitution.
    //
    // Input/output plumbing and the reader line are byte-identical to
    // the legacy template.
    assert!(script.contains("input <- Sys.getenv(\"AUTONOMICS_INPUT0\")"));
    assert!(script.contains("result_path <- Sys.getenv(\"AUTONOMICS_OUTPUT0\")"));
    assert!(script.contains("log_path <- Sys.getenv(\"AUTONOMICS_OUTPUT1\")"));
    assert!(
        script.contains("data <- read.delim(input, check.names = FALSE, stringsAsFactors = FALSE)")
    );
    // The coloc.abf() call keeps its named-argument structure; the
    // legacy literal priors p1 = 0.0001 become env reads (same values
    // after as.numeric()).
    assert!(script.contains("result <- coloc::coloc.abf("));
    assert!(script.contains("dataset1 = dataset_1"));
    assert!(script.contains("dataset2 = dataset_2"));
    assert!(script.contains("p1 = p1"));
    assert!(script.contains("p2 = p2"));
    assert!(script.contains("p12 = p12"));
    assert!(script.contains("p1 <- as.numeric(Sys.getenv(\"COLOC_P1\"))"));
    assert!(script.contains("p12 <- as.numeric(Sys.getenv(\"COLOC_P12\"))"));
    // Dataset construction keeps the legacy variable vocabulary
    // (snp_1/type_1/... and the list element names snp/type/beta/
    // varbeta/pvalues/MAF/N/s/sdY). Legacy read columns with
    // data$"<literal>"; the plugin resolves the same column through
    // data[[Sys.getenv(...)]] — identical R semantics.
    assert!(script.contains("dataset_1 <- list(snp = snp_1, type = type_1)"));
    assert!(script.contains("dataset_2 <- list(snp = snp_2, type = type_2)"));
    assert!(script.contains("dataset_1$beta <- beta_1"));
    assert!(script.contains("dataset_1$varbeta <- varbeta_1"));
    assert!(script.contains("dataset_1$pvalues <- pvalues_1"));
    assert!(script.contains("dataset_1$MAF <- MAF_1"));
    assert!(script.contains("dataset_1$N <- N_1"));
    assert!(script.contains("dataset_1$s <- s_1"));
    assert!(script.contains("dataset_1$sdY <- sdY_1"));
    assert!(script.contains("dataset_2$beta <- beta_2"));
    assert!(script.contains("dataset_2$varbeta <- varbeta_2"));
    assert!(script.contains("dataset_2$pvalues <- pvalues_2"));
    assert!(script.contains("dataset_2$MAF <- MAF_2"));
    assert!(script.contains("dataset_2$N <- N_2"));
    assert!(script.contains("dataset_2$s <- s_2"));
    assert!(script.contains("dataset_2$sdY <- sdY_2"));
    // Scalar per-trait fields parse through as.numeric().
    assert!(script.contains("N_1 <- as.numeric(Sys.getenv(\"COLOC_DATASET1_N\"))"));
    assert!(script.contains("sdY_2 <- as.numeric(Sys.getenv(\"COLOC_DATASET2_SD_Y\"))"));
    // Optional fields are gated by nzchar(), the R equivalent of the
    // legacy Option<T> branches.
    assert!(script.contains("nzchar(Sys.getenv(\"COLOC_DATASET1_BETA\"))"));
    assert!(script.contains("nzchar(Sys.getenv(\"COLOC_DATASET2_PVALUES\"))"));
    // The cross-field rules the legacy Rust validate() enforced before
    // the container started are mirrored verbatim inside the script.
    assert!(script.contains("dataset1 must supply exactly one of (beta+varbeta) or pvalues"));
    assert!(script.contains("dataset1.maf is required when using dataset1.pvalues"));
    assert!(script.contains("dataset2.s is required for cc + pvalues"));
    assert!(script.contains("dataset1.type must be quant or cc"));
    // Output epilogues: printed log and the RDS artifact.
    assert!(script.contains("sink(log_path, split = TRUE)"));
    assert!(script.contains("cat(\"## coloc.abf (official R package 5.2.3)\\n\")"));
    assert!(script.contains("ord <- order(result$results$SNP.PP.H4, decreasing = TRUE)"));
    assert!(script.contains("top <- head(result$results[ord, c(\"snp\", \"SNP.PP.H4\")], 10)"));
    assert!(script.contains("saveRDS(result, result_path)"));
    // The renderer must leave the R source untouched: no template
    // markers, and every quote region stays balanced.
    assert!(!script.contains("{{"));
}

#[test]
fn coloc_abf_plugin_renders_submitted_values_into_env() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "coloc_abf");

    // Cross-section of the param surface: required strings, optional
    // strings set and unset, optional numbers (integer- and
    // fractional-valued), and a prior override. Dataset 2 takes the
    // pvalues path (pvalues + maf + n + s) to exercise the cc branch.
    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({
            "dataset1_type": "quant",
            "dataset1_snp": "rsid",
            "dataset1_beta": "b1",
            "dataset1_varbeta": "v1",
            "dataset1_sd_y": 1.0,
            "dataset2_type": "cc",
            "dataset2_snp": "rsid",
            "dataset2_pvalues": "p2",
            "dataset2_maf": "maf2",
            "dataset2_n": 5000,
            "dataset2_s": 0.3,
            "p12": 0.000002
        }),
    )
    .unwrap();

    assert_eq!(compiled.env.get("COLOC_DATASET1_BETA").unwrap(), "b1");
    assert_eq!(compiled.env.get("COLOC_DATASET1_SD_Y").unwrap(), "1.0");
    // Absent optionals render empty; the script gates them with nzchar().
    assert_eq!(compiled.env.get("COLOC_DATASET1_MAF").unwrap(), "");
    assert_eq!(compiled.env.get("COLOC_DATASET2_BETA").unwrap(), "");
    // An integer JSON number stays integer-spelled ("5000"); the
    // fractional value keeps its decimals.
    assert_eq!(compiled.env.get("COLOC_DATASET2_N").unwrap(), "5000");
    assert_eq!(compiled.env.get("COLOC_DATASET2_S").unwrap(), "0.3");
    // Untouched params fall back to the coloc.abf defaults.
    assert_eq!(compiled.env.get("COLOC_P1").unwrap(), "0.0001");
    assert_eq!(compiled.env.get("COLOC_P2").unwrap(), "0.0001");
    // Overridden prior: serde_json renders f64 2e-6 through ryu, whose
    // scientific branch starts at kk <= -5 — equal to the legacy
    // format!("{}", 2e-6) value after as.numeric() parsing.
    assert_eq!(compiled.env.get("COLOC_P12").unwrap(), "2e-6");
}

#[test]
fn coloc_abf_plugin_schema_marks_optionals_not_required() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "coloc_abf");

    let schema =
        serde_json::to_value(container_plugin::compile::compile_schema(&node.params)).unwrap();
    // Only the four always-present legacy fields are required: the two
    // trait types and the two SNP columns. Everything else was
    // Option<T> or defaulted in ColocAbfContainerSpec.
    let required = schema["required"].as_array().unwrap();
    assert_eq!(
        required
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "dataset1_snp",
            "dataset1_type",
            "dataset2_snp",
            "dataset2_type"
        ]
    );
    assert_eq!(schema["additionalProperties"], false);
    // The legacy ColocTraitType enum flattens to a documented string.
    assert_eq!(schema["properties"]["dataset1_type"]["type"], "string");
    // Option<f64> scalars flatten to optional numbers.
    assert_eq!(schema["properties"]["dataset1_n"]["type"], "number");
    // Priors keep the legacy (0.0..=1.0) validate() bounds and defaults.
    assert_eq!(schema["properties"]["p1"]["type"], "number");
    assert_eq!(schema["properties"]["p1"]["minimum"], 0.0);
    assert_eq!(schema["properties"]["p1"]["maximum"], 1.0);
    assert_eq!(schema["properties"]["p1"]["default"], json!(1e-4));
    assert_eq!(schema["properties"]["p12"]["default"], json!(1e-5));
}

#[test]
fn coloc_abf_plugin_rejects_out_of_range_priors_and_missing_required_params() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "coloc_abf");

    // Mirrors the legacy wrapper test `rejects_prior_out_of_range`:
    // validate() rejected p12 = 1.5; the DSL bound (max = 1.0) now
    // rejects it at compile time.
    let mut values = base_values();
    values["p12"] = json!(1.5);
    let error = compile_container_spec(node, &manifest.image, &manifest.panels, &values)
        .unwrap_err()
        .to_string();
    assert!(error.contains("p12"), "{error}");
    assert!(error.contains("maximum"), "{error}");

    // Mirrors the legacy serde enforcement of the required
    // ColocDatasetSpec fields: an absent SNP column is a MissingParam.
    let mut missing = base_values();
    missing.as_object_mut().unwrap().remove("dataset1_snp");
    let error = compile_container_spec(node, &manifest.image, &manifest.panels, &missing)
        .unwrap_err()
        .to_string();
    assert!(error.contains("dataset1_snp"), "{error}");
    assert!(error.contains("missing required param"), "{error}");
}
