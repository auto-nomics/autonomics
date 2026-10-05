//! Golden parity tests for the mrpresso plugin: the `[[nodes]]` entry must
//! compile to the same `Container` spec the legacy Rust wrapper
//! (`nodes-io/src/mrpresso_container.rs`) produced. The plugin directory
//! lives outside this repository (`/mnt/projects/node-plugins/mrpresso` by
//! default, overridable via `NODE_PLUGINS_ROOT`); the test is skipped when
//! absent so CI without the plugin checkout stays green.

use std::path::PathBuf;

use container_plugin::compile::spec_compile::compile_container_spec;
use container_plugin::manifest::PluginManifest;
use serde_json::json;

fn plugin_root() -> Option<PathBuf> {
    let root = std::env::var_os("NODE_PLUGINS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/projects/node-plugins"));
    let manifest = root.join("mrpresso").join("manifest.toml");
    manifest.is_file().then_some(root)
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let text = std::fs::read_to_string(root.join("mrpresso").join("manifest.toml")).unwrap();
    // Inline script_file the way the loader does.
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        if let Some(relative) = node.command.script_file.clone() {
            let source = std::fs::read_to_string(root.join("mrpresso").join(&relative)).unwrap();
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

/// The four column-name params have no defaults and are not optional, so
/// the compile under test must submit them (the legacy wrapper's Spec had
/// them as plain required fields).
fn required_params() -> serde_json::Value {
    json!({
        "beta_outcome": "Y_effect",
        "sd_outcome": "Y_se",
        "beta_exposure": ["E1_effect"],
        "sd_exposure": ["E1_se"]
    })
}

#[test]
fn mrpresso_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "mrpresso");

    let compiled =
        compile_container_spec(node, &manifest.image, &manifest.panels, &required_params())
            .unwrap();

    assert_eq!(
        compiled.image,
        "ghcr.io/auto-nomics/autonomics/mrpresso@sha256:a3c46770506e07141dd89d3b805a3ec3d04b56367738b81917675c860ad7e2a4"
    );
    assert_eq!(compiled.outputs.len(), 2);
    assert_eq!(compiled.outputs[0].path, "mrpresso.RDS");
    assert_eq!(compiled.outputs[0].format.as_deref(), Some("r_rds"));
    assert_eq!(compiled.outputs[1].path, "mrpresso.log");
    assert_eq!(compiled.outputs[1].format.as_deref(), Some("mrpresso_log"));
    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert!(matches!(
        compiled.pull_policy,
        container_runtime::PullPolicy::Missing
    ));
    // Legacy timeout default (DEFAULT_TIMEOUT_SECS). The legacy spec
    // accepted a per-instance override; the plugin DSL pins it per kind.
    assert_eq!(compiled.timeout_secs, 900);
    // Deliberate delta: the legacy default was "/artifacts/mrpresso_container";
    // the plugin follows the kind rename (`_container` suffix dropped), the
    // same rule the ldsc family migration applied to its kinds.
    assert_eq!(compiled.artifact_prefix, "/artifacts/mrpresso");
    assert_eq!(compiled.workdir, None);
    // The legacy wrapper bound no panels.
    assert!(compiled.panels.is_empty());
    assert!(compiled.panel_bundles.is_empty());
    assert_eq!(compiled.command, vec!["Rscript".to_string()]);

    // Defaults render through the env channel: booleans as true/false
    // (R's as.logical() accepts both spellings where the legacy script
    // embedded TRUE/FALSE), numbers with their serde_json spelling.
    assert_eq!(
        compiled.env.get("MRPRESSO_BETA_OUTCOME").unwrap(),
        "Y_effect"
    );
    assert_eq!(compiled.env.get("MRPRESSO_SD_OUTCOME").unwrap(), "Y_se");
    assert_eq!(
        compiled.env.get("MRPRESSO_BETA_EXPOSURE").unwrap(),
        "E1_effect"
    );
    assert_eq!(compiled.env.get("MRPRESSO_SD_EXPOSURE").unwrap(), "E1_se");
    // Both diagnostic bools default to false and render the literal so
    // the script's as.logical() parse chain never sees NA (F02).
    assert_eq!(compiled.env.get("MRPRESSO_OUTLIER_TEST").unwrap(), "false");
    assert_eq!(
        compiled.env.get("MRPRESSO_DISTORTION_TEST").unwrap(),
        "false"
    );
    assert_eq!(
        compiled.env.get("MRPRESSO_SIGNIF_THRESHOLD").unwrap(),
        "0.05"
    );
    assert_eq!(
        compiled.env.get("MRPRESSO_NB_DISTRIBUTION").unwrap(),
        "1000"
    );
    assert_eq!(compiled.env.get("MRPRESSO_SEED").unwrap(), "123");

    // Script parity is semantic, not byte-exact: the plugin reads every
    // param from env instead of Rust string interpolation, but the
    // mr_presso invocation, prologue, and epilogue must match the legacy
    // embedded R code token for token.
    let script = compiled.script.as_deref().unwrap();
    // Prologue: input/output plumbing and the delimiter-agnostic reader.
    assert!(script.contains("input <- Sys.getenv(\"AUTONOMICS_INPUT0\")"));
    assert!(script.contains("result_path <- Sys.getenv(\"AUTONOMICS_OUTPUT0\")"));
    assert!(script.contains("log_path <- Sys.getenv(\"AUTONOMICS_OUTPUT1\")"));
    assert!(
        script.contains("data <- read.delim(input, check.names = FALSE, stringsAsFactors = FALSE)")
    );
    // Legacy validate() checks the DSL cannot express live as stop() guards
    // with the legacy error messages.
    assert!(script.contains("stop(\"beta_outcome and sd_outcome cannot be empty\")"));
    assert!(script.contains(
        "stop(\"beta_exposure and sd_exposure must be non-empty and have equal lengths\")"
    ));
    // Seed: legacy embedded the integer; the plugin parses the env literal,
    // which R coerces identically inside set.seed.
    assert!(script.contains("set.seed(as.numeric(Sys.getenv(\"MRPRESSO_SEED\")))"));
    // String arrays: the env channel space-joins; a single-space fixed
    // strsplit is the exact inverse of that join.
    assert!(
        script.contains("strsplit(Sys.getenv(\"MRPRESSO_BETA_EXPOSURE\"), \" \", fixed = TRUE)")
    );
    assert!(script.contains("strsplit(Sys.getenv(\"MRPRESSO_SD_EXPOSURE\"), \" \", fixed = TRUE)"));
    // The official call: same argument names, same order, same values.
    assert!(script.contains("result <- MRPRESSO::mr_presso("));
    assert!(script.contains("BetaOutcome = beta_outcome"));
    assert!(script.contains("BetaExposure = beta_exposure"));
    assert!(script.contains("SdOutcome = sd_outcome"));
    assert!(script.contains("SdExposure = sd_exposure"));
    assert!(script.contains("OUTLIERtest = as.logical(Sys.getenv(\"MRPRESSO_OUTLIER_TEST\"))"));
    assert!(
        script.contains("DISTORTIONtest = as.logical(Sys.getenv(\"MRPRESSO_DISTORTION_TEST\"))")
    );
    assert!(script.contains("data = data"));
    assert!(
        script.contains("NbDistribution = as.numeric(Sys.getenv(\"MRPRESSO_NB_DISTRIBUTION\"))")
    );
    assert!(
        script.contains("SignifThreshold = as.numeric(Sys.getenv(\"MRPRESSO_SIGNIF_THRESHOLD\"))")
    );
    // Epilogue: identical sink/log and saveRDS stanzas.
    assert!(script.contains("sink(log_path, split = TRUE)"));
    assert!(script.contains("print(result)"));
    assert!(script.contains("sink()\n"));
    assert!(script.contains("saveRDS(result, result_path)"));
}

#[test]
fn mrpresso_plugin_renders_submitted_values_into_env() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "mrpresso");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({
            "beta_outcome": "Y_effect",
            "sd_outcome": "Y_se",
            "beta_exposure": ["E1_effect", "E2_effect"],
            "sd_exposure": ["E1_se", "E2_se"],
            "outlier_test": true,
            "distortion_test": false,
            "signif_threshold": 0.01,
            "nb_distribution": 5000,
            "seed": 999
        }),
    )
    .unwrap();

    // Arrays render space-joined on the env channel; the script re-splits
    // on a single space, preserving the legacy c("a", "b") semantics.
    assert_eq!(
        compiled.env.get("MRPRESSO_BETA_EXPOSURE").unwrap(),
        "E1_effect E2_effect"
    );
    assert_eq!(
        compiled.env.get("MRPRESSO_SD_EXPOSURE").unwrap(),
        "E1_se E2_se"
    );
    assert_eq!(compiled.env.get("MRPRESSO_OUTLIER_TEST").unwrap(), "true");
    assert_eq!(
        compiled.env.get("MRPRESSO_DISTORTION_TEST").unwrap(),
        "false"
    );
    assert_eq!(
        compiled.env.get("MRPRESSO_SIGNIF_THRESHOLD").unwrap(),
        "0.01"
    );
    assert_eq!(
        compiled.env.get("MRPRESSO_NB_DISTRIBUTION").unwrap(),
        "5000"
    );
    assert_eq!(compiled.env.get("MRPRESSO_SEED").unwrap(), "999");
}

#[test]
fn mrpresso_plugin_schema_marks_column_params_required() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "mrpresso");

    let schema =
        serde_json::to_value(container_plugin::compile::compile_schema(&node.params)).unwrap();
    // BTreeMap order: the four legacy required fields stay required; every
    // defaulted field drops out of `required`.
    let required = schema["required"].as_array().unwrap();
    assert_eq!(
        required,
        &vec![
            json!("beta_exposure"),
            json!("beta_outcome"),
            json!("sd_exposure"),
            json!("sd_outcome")
        ]
    );
    let beta = &schema["properties"]["beta_exposure"];
    assert_eq!(beta["type"], "array");
    assert_eq!(beta["items"]["type"], "string");
    // min_len 1 encodes the legacy "non-empty array" validate() check.
    assert_eq!(beta["minItems"], 1.0);
    let signif = &schema["properties"]["signif_threshold"];
    assert_eq!(signif["type"], "number");
    assert_eq!(signif["default"], 0.05);
    // Bounds per the legacy validate() code (inclusive [0, 1], even though
    // its error message said "(0, 1]").
    assert_eq!(signif["minimum"], 0.0);
    assert_eq!(signif["maximum"], 1.0);
    let nb = &schema["properties"]["nb_distribution"];
    assert_eq!(nb["type"], "integer");
    assert_eq!(nb["default"], 1000);
    assert_eq!(nb["minimum"], 1.0);
    assert_eq!(schema["properties"]["outlier_test"]["type"], "boolean");
    assert_eq!(schema["properties"]["outlier_test"]["default"], false);
    assert_eq!(schema["properties"]["seed"]["type"], "integer");
    assert_eq!(schema["properties"]["seed"]["default"], 123);
}
