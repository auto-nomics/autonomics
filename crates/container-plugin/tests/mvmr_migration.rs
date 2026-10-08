//! Golden parity tests for the mvmr plugin family: the `[[nodes]]` entry
//! must compile to the same `Container` spec the legacy Rust wrapper
//! (`nodes-io/src/mvmr_container.rs`) produced. The plugin directory lives
//! outside this repository (`/mnt/projects/node-plugins/mvmr` by default,
//! overridable via `NODE_PLUGINS_ROOT`); the test is skipped when absent so
//! CI without the plugin checkout stays green.

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
    let manifest = root.join("mvmr").join("manifest.toml");
    if manifest.is_file() {
        return Some(root);
    }
    if explicit.is_some() {
        // fix-review R04 (2026-10-05): an explicitly set NODE_PLUGINS_ROOT
        // means migration parity was requested. A missing family must fail
        // loudly — early-returns here used to count as *passed* tests, so a
        // green summary claimed coverage that never ran.
        panic!(
            "NODE_PLUGINS_ROOT is set but the mvmr family is not deployed \
             under it ({}) — migration parity cannot run. Deploy the family \
             or unset NODE_PLUGINS_ROOT to skip these tests deliberately.",
            manifest.display()
        );
    }
    eprintln!("skipping: mvmr plugin directory not present (NODE_PLUGINS_ROOT unset)");
    None
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let text = std::fs::read_to_string(root.join("mvmr").join("manifest.toml")).unwrap();
    // Inline script_file the way the loader does.
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        if let Some(relative) = node.command.script_file.clone() {
            let source = std::fs::read_to_string(root.join("mvmr").join(&relative)).unwrap();
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

/// The four params the legacy `MvmrContainerSpec` demanded (no serde
/// default): the plugin marks them required, so every compile supplies them.
fn required_columns() -> serde_json::Value {
    json!({
        "beta_yg": "SBP_beta",
        "sebeta_yg": "SBP_se",
        "beta_xg": ["LDL_beta", "HDL_beta"],
        "sebeta_xg": ["LDL_se", "HDL_se"],
    })
}

#[test]
fn mvmr_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "mvmr");

    let compiled =
        compile_container_spec(node, &manifest.image, &manifest.panels, &required_columns())
            .unwrap();

    assert_eq!(
        compiled.image,
        "ghcr.io/auto-nomics/autonomics/mvmr@sha256:ce9ad3f46cf8b74a95c194fe791b6a529d9168676c5d936a05ba440569260eb3"
    );
    assert_eq!(compiled.outputs.len(), 2);
    assert_eq!(compiled.outputs[0].path, "mvmr.RDS");
    assert_eq!(compiled.outputs[0].format.as_deref(), Some("r_rds"));
    assert_eq!(compiled.outputs[1].path, "mvmr.log");
    assert_eq!(compiled.outputs[1].format.as_deref(), Some("mvmr_log"));
    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert!(matches!(
        compiled.pull_policy,
        container_runtime::PullPolicy::Missing
    ));
    // Legacy DEFAULT_TIMEOUT_SECS.
    assert_eq!(compiled.timeout_secs, 900);
    // Legacy DEFAULT_ARTIFACT_PREFIX was "/artifacts/mvmr_container"; the
    // plugin drops the `_container` suffix together with the kind rename
    // (same convention the ldsc family followed for its kinds).
    assert_eq!(
        compiled.artifact_prefix,
        Some("/artifacts/mvmr".to_string())
    );
    assert_eq!(compiled.workdir, None);
    // Panel-free family: the legacy wrapper declared no panels and no
    // panel bundles, and the manifest declares no [[panels]].
    assert!(compiled.panels.is_empty());
    assert!(compiled.panel_bundles.is_empty());
    assert_eq!(compiled.command, vec!["Rscript".to_string()]);

    // The legacy wrapper shipped an empty env (all params baked into the R
    // program by Rust codegen); the plugin's param channel is MVMR_* env.
    // String arrays render space-joined; bools render true/false; optional
    // params absent from the submitted spec render as empty strings.
    assert_eq!(compiled.env.get("MVMR_BETA_YG").unwrap(), "SBP_beta");
    assert_eq!(compiled.env.get("MVMR_SEBETA_YG").unwrap(), "SBP_se");
    assert_eq!(
        compiled.env.get("MVMR_BETA_XG").unwrap(),
        "LDL_beta HDL_beta"
    );
    assert_eq!(compiled.env.get("MVMR_SEBETA_XG").unwrap(), "LDL_se HDL_se");
    assert_eq!(compiled.env.get("MVMR_LABEL_COLUMN").unwrap(), "");
    assert_eq!(compiled.env.get("MVMR_STRENGTH").unwrap(), "true");
    assert_eq!(compiled.env.get("MVMR_STRHET").unwrap(), "true");
    assert_eq!(compiled.env.get("MVMR_PLEIOTROPY").unwrap(), "true");
    // qhet defaults to false and renders the literal — R's
    // as.logical("false") is FALSE while as.logical("") is NA (F02).
    assert_eq!(compiled.env.get("MVMR_QHET").unwrap(), "false");
    assert_eq!(compiled.env.get("MVMR_PCOR").unwrap(), "");

    // Script is asserted semantically, not byte-exactly: the plugin drives
    // the R program through env + Sys.getenv instead of Rust string-building.
    let script = compiled.script.as_deref().unwrap();
    // Input staging and the official format_mvmr construction. Legacy
    // baked the column names at compile time (`data[, c("LDL_beta",
    // "HDL_beta")]`, `data$SBP_beta`, `RSID = "SNP"`); the plugin rebuilds
    // the identical calls from env — `data[[name]]` is the `data$name`
    // codegen in indexing form.
    assert!(
        script.contains("data <- read.delim(input, check.names = FALSE, stringsAsFactors = FALSE)")
    );
    assert!(script.contains("MVMR::format_mvmr("));
    assert!(script.contains("BXGs = as.matrix(data[, beta_xg, drop = FALSE])"));
    assert!(script.contains("BYG = data[[beta_yg]]"));
    assert!(script.contains("seBXGs = as.matrix(data[, sebeta_xg, drop = FALSE])"));
    assert!(script.contains("seBYG = data[[sebeta_yg]]"));
    assert!(
        script
            .contains("RSID = if (nzchar(label_column)) data[[label_column]] else rownames(data)")
    );
    // pcor handling: legacy emitted either `pcor <- NULL; gencov <- 0` or a
    // baked `matrix(c(1, 0.25, ...), nrow = p, ncol = p, byrow = TRUE)` plus
    // phenocov_mvmr; the script selects at runtime from the serialized
    // MVMR_PCOR value and rebuilds the same byrow = TRUE matrix.
    assert!(
        script.contains("matrix(as.numeric(unlist(pcor_cells)), nrow = p, ncol = p, byrow = TRUE)")
    );
    assert!(script.contains(
        "gencov <- MVMR::phenocov_mvmr(pcor, as.matrix(data[, sebeta_xg, drop = FALSE]))"
    ));
    // Every upstream MVMR function the legacy wrapper could invoke.
    assert!(script.contains("ivw = MVMR::ivw_mvmr(mvmr_input, gencov = gencov)"));
    assert!(script.contains("MVMR::strength_mvmr(mvmr_input, gencov)"));
    assert!(script.contains("MVMR::strhet_mvmr(mvmr_input, gencov)"));
    assert!(script.contains("MVMR::pleiotropy_mvmr(mvmr_input, gencov)"));
    assert!(script.contains("MVMR::qhet_mvmr(mvmr_input, pcor, CI = FALSE)"));
    // The legacy compile-time call-or-NULL dispatch became script-side
    // if/else over the same booleans (assigning NULL drops the slot,
    // exactly like the legacy emitted `result$x <- NULL`).
    assert!(script.contains(
        "result$strength <- if (strength) MVMR::strength_mvmr(mvmr_input, gencov) else NULL"
    ));
    assert!(script.contains("result$covariance <- list(pcor = pcor, source = if (is.null(pcor)) \"zero\" else \"phenocov_mvmr\")"));
    // Log epilogue and RDS output, byte-faithful to the legacy program.
    assert!(script.contains("sink(log_path, split = TRUE)"));
    assert!(script.contains("print(result)"));
    assert!(script.contains("sink()"));
    assert!(script.contains("saveRDS(result, result_path)"));
    // Legacy validate() message set, moved into the script.
    assert!(script.contains("beta_yg and sebeta_yg cannot be empty"));
    assert!(script.contains("beta_xg and sebeta_xg must be non-empty and equal-length"));
    assert!(script.contains("qhet requires a pcor matrix"));
    assert!(script.contains("pcor must be symmetric"));
}

#[test]
fn mvmr_plugin_requires_the_legacy_required_params() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "mvmr");

    // The legacy spec deserialized beta_yg/sebeta_yg/beta_xg/sebeta_xg as
    // mandatory fields; the plugin enforces the same through the required
    // list (first missing param is named).
    let error =
        compile_container_spec(node, &manifest.image, &manifest.panels, &json!({})).unwrap_err();
    assert!(error.to_string().contains("beta_xg"), "{error}");
}

#[test]
fn mvmr_plugin_renders_submitted_values_into_env() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "mvmr");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({
            "beta_yg": "logBMI_beta",
            "sebeta_yg": "logBMI_se",
            "beta_xg": ["LDL_beta", "HDL_beta", "TG_beta"],
            "sebeta_xg": ["LDL_se", "HDL_se", "TG_se"],
            "label_column": "SNP",
            "strength": false,
            "qhet": true,
            // DSL gap: pcor is a serialized row-major matrix string until
            // the param vocabulary grows a number-matrix type.
            "pcor": "1,0.25,0.1;0.25,1,0.2;0.1,0.2,1",
        }),
    )
    .unwrap();

    assert_eq!(
        compiled.env.get("MVMR_BETA_XG").unwrap(),
        "LDL_beta HDL_beta TG_beta"
    );
    assert_eq!(compiled.env.get("MVMR_LABEL_COLUMN").unwrap(), "SNP");
    // Submitted false renders the literal (value semantics).
    assert_eq!(compiled.env.get("MVMR_STRENGTH").unwrap(), "false");
    // Unsubmitted booleans keep their defaults.
    assert_eq!(compiled.env.get("MVMR_STRHET").unwrap(), "true");
    assert_eq!(compiled.env.get("MVMR_QHET").unwrap(), "true");
    assert_eq!(
        compiled.env.get("MVMR_PCOR").unwrap(),
        "1,0.25,0.1;0.25,1,0.2;0.1,0.2,1"
    );
}

#[test]
fn mvmr_plugin_schema_matches_the_legacy_param_surface() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "mvmr");

    let schema =
        serde_json::to_value(container_plugin::compile::compile_schema(&node.params)).unwrap();
    // BTreeMap order: exactly the four legacy non-defaulted fields.
    let required = schema["required"].as_array().unwrap();
    assert_eq!(
        required,
        &vec![
            json!("beta_xg"),
            json!("beta_yg"),
            json!("sebeta_xg"),
            json!("sebeta_yg")
        ]
    );
    assert_eq!(schema["properties"]["beta_xg"]["type"], "array");
    assert_eq!(schema["properties"]["beta_xg"]["items"]["type"], "string");
    assert_eq!(schema["properties"]["beta_xg"]["minItems"], 1.0);
    assert_eq!(schema["properties"]["strength"]["type"], "boolean");
    assert_eq!(schema["properties"]["strength"]["default"], true);
    assert_eq!(schema["properties"]["qhet"]["default"], false);
    // gencov stays unadvertised, exactly like the legacy #[schemars(skip)].
    assert!(schema["properties"].get("gencov").is_none());
    // pcor changes shape: legacy advertised a nested number array; the DSL
    // has no number-matrix type, so it is a serialized string for now.
    assert_eq!(schema["properties"]["pcor"]["type"], "string");
}
