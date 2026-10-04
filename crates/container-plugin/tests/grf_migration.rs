//! Golden contract tests for the grf plugin.
//!
//! Unlike every earlier `*_migration.rs` there is no legacy
//! `container_command` wrapper to be byte-parity with: the 23 grf kinds were
//! native in-process nodes (`crates/node-bundles/nodes-grf` over
//! `bio_crates/grf` + `crates/grf-sys`, all removed by this migration).
//! The golden therefore pins the *new* contract exactly as compiled — image
//! digest, ports, outputs, env channel, resources, timeouts — against the
//! retired native specs (kind strings, port surface, parameter defaults and
//! the deliberate adaptations recorded in the plugin README). The plugin
//! directory lives outside this repository
//! (`/mnt/projects/node-plugins/grf` by default, overridable via
//! `NODE_PLUGINS_ROOT`); the test is skipped when absent so CI without the
//! plugin checkout stays green.
//!
//! grf is a baked-runner family (deseq2 precedent): the image carries
//! `/opt/autonomics/grf_runner.R`, so every node's command is interpreter +
//! fixed argv with `script: None`, and the env channel — `AUTONOMICS_GRF_OP`
//! plus one `AUTONOMICS_GRF_*` variable per parameter — is the whole param
//! contract.

use std::path::PathBuf;

use container_plugin::compile::spec_compile::compile_container_spec;
use container_plugin::manifest::PluginManifest;
use container_plugin::node_definition::NodeDefinition;
use serde_json::{Value, json};

const GRF_IMAGE: &str = "ghcr.io/auto-nomics/autonomics/grf@sha256:b75d551a77f72d6fe50e81c48948eb8551ffb230c7d254e40b9447c5ace4175e";

/// The retired native bundle's kind list (nodes-grf/src/lib.rs), unchanged.
const KINDS: &[&str] = &[
    // trainers (12)
    "grf_regression_forest",
    "grf_causal_forest",
    "grf_quantile_forest",
    "grf_probability_forest",
    "grf_survival_forest",
    "grf_multi_regression_forest",
    "grf_instrumental_forest",
    "grf_lm_forest",
    "grf_ll_regression_forest",
    "grf_boosted_regression_forest",
    "grf_multi_arm_causal_forest",
    "grf_causal_survival_forest",
    // prediction + causal analysis (5)
    "grf_predict_forest",
    "grf_average_treatment_effect",
    "grf_best_linear_projection",
    "grf_test_calibration",
    "grf_get_scores",
    // forest analysis (5)
    "grf_get_forest_weights",
    "grf_split_frequencies",
    "grf_variable_importance",
    "grf_get_tree",
    "grf_merge_forests",
    // data generation (1)
    "grf_generate_causal_data",
];

fn plugin_root() -> Option<PathBuf> {
    let root = std::env::var_os("NODE_PLUGINS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/projects/node-plugins"));
    let manifest = root.join("grf").join("manifest.toml");
    manifest.is_file().then_some(root)
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let text = std::fs::read_to_string(root.join("grf").join("manifest.toml")).unwrap();
    // Inline script_file the way the loader does (the grf family is
    // baked-runner: no script_file expected, but keep the loop honest).
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        if let Some(relative) = node.command.script_file.clone() {
            let source = std::fs::read_to_string(root.join("grf").join(&relative)).unwrap();
            node.command.script = Some(source);
            node.command.script_file = None;
        }
    }
    manifest
}

fn node_by_kind<'a>(manifest: &'a PluginManifest, kind: &str) -> &'a NodeDefinition {
    manifest
        .nodes
        .iter()
        .find(|n| n.kind == kind)
        .unwrap_or_else(|| panic!("{kind} missing from manifest"))
}

/// Minimal legal specs per kind — the plugin-era equivalent of the retired
/// `grf_nodes_registered.rs` fixture map (same required fields, minus the
/// params the file contract replaced with column names or dropped).
fn minimal_params(kind: &str) -> Value {
    match kind {
        "grf_regression_forest"
        | "grf_quantile_forest"
        | "grf_ll_regression_forest"
        | "grf_boosted_regression_forest" => json!({ "y_column_name": "y" }),
        "grf_causal_forest" => json!({ "y_column_name": "y", "w_column_name": "w" }),
        "grf_probability_forest" => {
            json!({ "y_column_name": "label", "num_classes": 2 })
        }
        "grf_survival_forest" => {
            json!({ "time_column_name": "time", "censor_column_name": "censor" })
        }
        "grf_multi_regression_forest" => json!({ "y_column_names": "y0" }),
        "grf_instrumental_forest" => {
            json!({ "y_column_name": "y", "w_column_name": "w", "z_column_name": "z" })
        }
        "grf_lm_forest" => json!({ "y_column_names": "y0", "w_column_names": "w0" }),
        "grf_multi_arm_causal_forest" => {
            json!({ "y_column_names": "y0", "w_column_name": "w" })
        }
        "grf_causal_survival_forest" => json!({
            "time_column_name": "time",
            "w_column_name": "w",
            "censor_column_name": "censor",
            "horizon": 2.0,
        }),
        "grf_split_frequencies" => json!({ "max_depth": 2 }),
        "grf_variable_importance" => json!({ "max_depth": 4 }),
        "grf_get_tree" => json!({ "index": 0 }),
        "grf_generate_causal_data" => json!({ "n": 50, "p": 4, "seed": 42 }),
        _ => json!({}),
    }
}

#[test]
fn every_kind_compiles_and_carries_its_op() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    assert_eq!(manifest.nodes.len(), KINDS.len());
    assert_eq!(manifest.plugin_name, "grf");
    for &kind in KINDS {
        let node = node_by_kind(&manifest, kind);
        let compiled = compile_container_spec(
            node,
            &manifest.image,
            &manifest.panels,
            &minimal_params(kind),
        )
        .unwrap_or_else(|e| panic!("{kind}: {e}"));

        // One family, one image, digest-pinned.
        assert_eq!(compiled.image, GRF_IMAGE, "{kind}");

        // The op env selects the R operation; the kind keeps the legacy
        // `grf_` prefix while the op is the bare R-level name.
        let op = compiled
            .env
            .get("AUTONOMICS_GRF_OP")
            .unwrap_or_else(|| panic!("{kind}: missing AUTONOMICS_GRF_OP"));
        assert_eq!(op, &kind.strip_prefix("grf_").unwrap().to_string());

        // Baked runner: fixed argv, no inline script or files.
        assert_eq!(
            compiled.command,
            vec![
                "Rscript".to_string(),
                "--vanilla".to_string(),
                "/opt/autonomics/grf_runner.R".to_string()
            ],
            "{kind}"
        );
        assert!(compiled.script.is_none(), "{kind}");
        assert!(compiled.files.is_empty(), "{kind}");

        // Legacy artifact prefix rule: /artifacts/<kind>.
        assert_eq!(
            compiled.artifact_prefix,
            format!("/artifacts/{kind}"),
            "{kind}"
        );

        // Hardened defaults everywhere.
        assert_eq!(compiled.network, "isolated", "{kind}");
        assert!(compiled.read_only_rootfs, "{kind}");
        assert!(matches!(
            compiled.pull_policy,
            container_runtime::PullPolicy::Missing
        ));
        assert!(compiled.panels.is_empty(), "{kind}");
        assert!(compiled.panel_bundles.is_empty(), "{kind}");
    }
}

#[test]
fn causal_forest_compiles_to_the_pinned_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "grf_causal_forest");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({ "y_column_name": "y", "w_column_name": "w" }),
    )
    .unwrap();

    // The legacy two-port trainer surface: forest + OOB (the Arrow
    // forest-exchange batch is now the forest.rds file).
    assert_eq!(compiled.outputs.len(), 2);
    assert_eq!(compiled.outputs[0].path, "forest.rds");
    assert_eq!(compiled.outputs[0].format.as_deref(), Some("r_rds"));
    assert_eq!(compiled.outputs[1].path, "oob_predictions.tsv");
    assert_eq!(compiled.outputs[1].format.as_deref(), Some("tsv"));
    assert_eq!(compiled.timeout_secs, 3600);

    // The env channel as an exact key set (BTreeMap order): OP + the 18
    // declared params (13 shared hyperparameters + the causal specifics).
    let env_names: Vec<&str> = compiled.env.keys().map(String::as_str).collect();
    assert_eq!(
        env_names,
        vec![
            "AUTONOMICS_GRF_ALPHA",
            "AUTONOMICS_GRF_CI_GROUP_SIZE",
            "AUTONOMICS_GRF_COMPUTE_OOB_PREDICTIONS",
            "AUTONOMICS_GRF_HONESTY",
            "AUTONOMICS_GRF_HONESTY_FRACTION",
            "AUTONOMICS_GRF_HONESTY_PRUNE_LEAVES",
            "AUTONOMICS_GRF_IMBALANCE_PENALTY",
            "AUTONOMICS_GRF_MIN_NODE_SIZE",
            "AUTONOMICS_GRF_MTRY",
            "AUTONOMICS_GRF_NUM_THREADS",
            "AUTONOMICS_GRF_NUM_TREES",
            "AUTONOMICS_GRF_OP",
            "AUTONOMICS_GRF_SAMPLE_FRACTION",
            "AUTONOMICS_GRF_SAMPLE_WEIGHTS_COLUMN",
            "AUTONOMICS_GRF_SEED",
            "AUTONOMICS_GRF_STABILIZE_SPLITS",
            "AUTONOMICS_GRF_W_COLUMN_NAME",
            "AUTONOMICS_GRF_W_HAT_COLUMN_NAME",
            "AUTONOMICS_GRF_X_COLUMN_NAMES",
            "AUTONOMICS_GRF_Y_COLUMN_NAME",
            "AUTONOMICS_GRF_Y_HAT_COLUMN_NAME",
        ]
    );

    // Defaults byte-equal the retired NodeTrainOptions (num_trees 2000,
    // ci_group_size 2, sample_fraction 0.5, min_node_size 5, honesty true,
    // honesty_fraction 0.5, honesty_prune_leaves true, alpha 0.05,
    // imbalance_penalty 0, num_threads 0, seed 42, compute_oob true) plus
    // the causal specifics (stabilize_splits true; empty column lists).
    let get = |key: &str| compiled.env.get(key).unwrap().clone();
    assert_eq!(get("AUTONOMICS_GRF_OP"), "causal_forest");
    assert_eq!(get("AUTONOMICS_GRF_NUM_TREES"), "2000");
    assert_eq!(get("AUTONOMICS_GRF_CI_GROUP_SIZE"), "2");
    assert_eq!(get("AUTONOMICS_GRF_SAMPLE_FRACTION"), "0.5");
    assert_eq!(get("AUTONOMICS_GRF_MTRY"), "0");
    assert_eq!(get("AUTONOMICS_GRF_MIN_NODE_SIZE"), "5");
    assert_eq!(get("AUTONOMICS_GRF_HONESTY"), "true");
    assert_eq!(get("AUTONOMICS_GRF_HONESTY_FRACTION"), "0.5");
    assert_eq!(get("AUTONOMICS_GRF_HONESTY_PRUNE_LEAVES"), "true");
    assert_eq!(get("AUTONOMICS_GRF_ALPHA"), "0.05");
    assert_eq!(get("AUTONOMICS_GRF_IMBALANCE_PENALTY"), "0.0");
    assert_eq!(get("AUTONOMICS_GRF_NUM_THREADS"), "0");
    assert_eq!(get("AUTONOMICS_GRF_SEED"), "42");
    assert_eq!(get("AUTONOMICS_GRF_COMPUTE_OOB_PREDICTIONS"), "true");
    assert_eq!(get("AUTONOMICS_GRF_STABILIZE_SPLITS"), "true");
    assert_eq!(get("AUTONOMICS_GRF_X_COLUMN_NAMES"), "");
    assert_eq!(get("AUTONOMICS_GRF_SAMPLE_WEIGHTS_COLUMN"), "");
    assert_eq!(get("AUTONOMICS_GRF_Y_HAT_COLUMN_NAME"), "");
    assert_eq!(get("AUTONOMICS_GRF_W_HAT_COLUMN_NAME"), "");

    // Submitted values render byte-exact through the template channel.
    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({
            "y_column_name": "outcome",
            "w_column_name": "treated",
            "x_column_names": "x0,x1,x2",
            "y_hat_column_name": "yhat",
            "num_trees": 500,
            "honesty": false,
        }),
    )
    .unwrap();
    let get = |key: &str| compiled.env.get(key).unwrap().clone();
    assert_eq!(get("AUTONOMICS_GRF_Y_COLUMN_NAME"), "outcome");
    assert_eq!(get("AUTONOMICS_GRF_W_COLUMN_NAME"), "treated");
    assert_eq!(get("AUTONOMICS_GRF_X_COLUMN_NAMES"), "x0,x1,x2");
    assert_eq!(get("AUTONOMICS_GRF_Y_HAT_COLUMN_NAME"), "yhat");
    assert_eq!(get("AUTONOMICS_GRF_NUM_TREES"), "500");
    assert_eq!(get("AUTONOMICS_GRF_HONESTY"), "");

    // Resources: deseq2-scale defaults for trainers.
    assert_eq!(compiled.cpus, Some(2.0));
    assert_eq!(compiled.memory.as_deref(), Some("4Gi"));
    assert_eq!(compiled.pids_limit, Some(256));
    assert_eq!(compiled.shm_size.as_deref(), Some("1Gi"));
}

#[test]
fn legacy_port_surface_is_preserved_per_kind() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let output_paths = |kind: &str| -> Vec<String> {
        node_by_kind(&manifest, kind)
            .ports
            .outputs
            .iter()
            .map(|o| o.path.clone())
            .collect()
    };
    let input_count = |kind: &str| node_by_kind(&manifest, kind).ports.inputs.len();

    // Two-port trainers: forest.rds + oob_predictions.tsv.
    for kind in [
        "grf_regression_forest",
        "grf_causal_forest",
        "grf_probability_forest",
        "grf_survival_forest",
        "grf_multi_regression_forest",
        "grf_instrumental_forest",
        "grf_lm_forest",
        "grf_multi_arm_causal_forest",
    ] {
        assert_eq!(
            output_paths(kind),
            vec!["forest.rds".to_string(), "oob_predictions.tsv".to_string()],
            "{kind}"
        );
        assert_eq!(input_count(kind), 1, "{kind}");
    }

    // Single-port trainers (legacy ports_one): forest only, no OOB output.
    for kind in [
        "grf_quantile_forest",
        "grf_ll_regression_forest",
        "grf_causal_survival_forest",
    ] {
        assert_eq!(output_paths(kind), vec!["forest.rds".to_string()], "{kind}");
        assert_eq!(input_count(kind), 1, "{kind}");
    }

    // Boosted carried a third output: the OOB error scalar.
    assert_eq!(
        output_paths("grf_boosted_regression_forest"),
        vec![
            "forest.rds".to_string(),
            "oob_predictions.tsv".to_string(),
            "oob_error.tsv".to_string(),
        ]
    );

    // Analysis nodes: forest in (plus the optional trailing data input the
    // runner reads only when the corresponding mode needs it).
    assert_eq!(input_count("grf_predict_forest"), 2);
    assert_eq!(
        output_paths("grf_predict_forest"),
        vec!["predictions.tsv".to_string()]
    );
    assert_eq!(input_count("grf_average_treatment_effect"), 1);
    assert_eq!(
        output_paths("grf_average_treatment_effect"),
        vec!["ate.tsv".to_string()]
    );
    assert_eq!(input_count("grf_best_linear_projection"), 2);
    assert_eq!(
        output_paths("grf_test_calibration"),
        vec!["calibration.tsv".to_string()]
    );
    assert_eq!(
        output_paths("grf_get_scores"),
        vec!["scores.tsv".to_string()]
    );
    assert_eq!(input_count("grf_get_forest_weights"), 2);
    assert_eq!(
        output_paths("grf_get_forest_weights"),
        vec!["forest_weights.tsv".to_string()]
    );
    assert_eq!(
        output_paths("grf_split_frequencies"),
        vec!["split_frequencies.tsv".to_string()]
    );
    assert_eq!(
        output_paths("grf_variable_importance"),
        vec!["variable_importance.tsv".to_string()]
    );
    assert_eq!(output_paths("grf_get_tree"), vec!["tree.rds".to_string()]);
    // The legacy merge node declared eight forest input ports.
    assert_eq!(input_count("grf_merge_forests"), 8);
    assert_eq!(
        output_paths("grf_merge_forests"),
        vec!["forest.rds".to_string()]
    );
    // generate_causal_data has no inputs at all.
    assert_eq!(input_count("grf_generate_causal_data"), 0);
    assert_eq!(
        output_paths("grf_generate_causal_data"),
        vec!["causal_data.tsv".to_string()]
    );
}

#[test]
fn r_wrapper_param_constraints_are_reflected_per_kind() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let params = |kind: &str| {
        node_by_kind(&manifest, kind)
            .params
            .keys()
            .cloned()
            .collect::<Vec<_>>()
    };

    // Params the grf R wrapper does not accept are not declared.
    for kind in [
        "grf_regression_forest",
        "grf_causal_forest",
        "grf_probability_forest",
        "grf_instrumental_forest",
        "grf_lm_forest",
        "grf_multi_arm_causal_forest",
        "grf_causal_survival_forest",
    ] {
        assert!(
            params(kind).contains(&"ci_group_size".to_string()),
            "{kind}"
        );
        assert!(
            params(kind).contains(&"compute_oob_predictions".to_string()),
            "{kind}"
        );
    }
    for kind in [
        "grf_quantile_forest",
        "grf_survival_forest",
        "grf_multi_regression_forest",
    ] {
        assert!(
            !params(kind).contains(&"ci_group_size".to_string()),
            "{kind}"
        );
    }
    for kind in ["grf_quantile_forest", "grf_ll_regression_forest"] {
        assert!(
            !params(kind).contains(&"sample_weights_column".to_string()),
            "{kind}"
        );
    }
    // The grf R wrapper has no compute.oob.predictions for these kinds.
    for kind in [
        "grf_boosted_regression_forest",
        "grf_quantile_forest",
        "grf_ll_regression_forest",
    ] {
        assert!(
            !params(kind).contains(&"compute_oob_predictions".to_string()),
            "{kind}"
        );
    }
    // survival additionally drops imbalance_penalty (R wrapper).
    assert!(!params("grf_survival_forest").contains(&"imbalance_penalty".to_string()));
    // legacy_seed is gone entirely: the runner pins the grf option instead.
    for &kind in KINDS {
        assert!(!params(kind).contains(&"legacy_seed".to_string()), "{kind}");
    }

    // boost_error_reduction carries the official ratio semantics.
    let boosted = node_by_kind(&manifest, "grf_boosted_regression_forest");
    let error_reduction = &boosted.params["boost_error_reduction"];
    assert_eq!(error_reduction.default, Some(json!(0.97)));
    // The causal-survival target is the R string, not the native int.
    let causal_survival = node_by_kind(&manifest, "grf_causal_survival_forest");
    assert_eq!(
        causal_survival.params["target"].default,
        Some(json!("RMST"))
    );
}

#[test]
fn schema_and_bounds_enforce_the_legacy_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);

    // Required params mirror the retired serde specs (no-default fields).
    let causal = node_by_kind(&manifest, "grf_causal_forest");
    let schema = container_plugin::compile::compile_schema(&causal.params);
    let schema_json = serde_json::to_value(&schema).unwrap();
    let mut required = schema_json["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    required.sort();
    assert_eq!(required, vec!["w_column_name", "y_column_name"]);
    // Bounds from the shared hyperparameter set (ParamSpec bounds are f64,
    // so the schema renders them as floats).
    let props = &schema_json["properties"];
    assert_eq!(props["num_trees"]["minimum"], json!(1.0));
    assert_eq!(props["sample_fraction"]["exclusiveMinimum"], json!(0.0));
    assert_eq!(props["sample_fraction"]["exclusiveMaximum"], json!(1.0));
    assert_eq!(props["seed"]["maximum"], json!(2147483647.0));

    // Out-of-range and unknown params are rejected at compile time.
    let node = node_by_kind(&manifest, "grf_causal_forest");
    let err = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({ "y_column_name": "y", "w_column_name": "w", "sample_fraction": 1.0 }),
    )
    .unwrap_err();
    assert!(err.to_string().contains("sample_fraction"), "{err}");
    let err = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({ "y_column_name": "y", "w_column_name": "w", "bogus": 1 }),
    )
    .unwrap_err();
    assert!(err.to_string().contains("bogus"), "{err}");
    let err =
        compile_container_spec(node, &manifest.image, &manifest.panels, &json!({})).unwrap_err();
    // The first missing required param is named (BTreeMap order: w before y).
    assert!(
        err.to_string()
            .contains("missing required param `w_column_name`"),
        "{err}"
    );
}

#[test]
fn resource_and_timeout_tiers_are_pinned() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    for &kind in KINDS {
        let node = node_by_kind(&manifest, kind);
        let expected_timeout: u64 = match kind {
            "grf_boosted_regression_forest" => 7200,
            "grf_predict_forest"
            | "grf_get_forest_weights"
            | "grf_split_frequencies"
            | "grf_variable_importance"
            | "grf_merge_forests" => 900,
            "grf_average_treatment_effect"
            | "grf_best_linear_projection"
            | "grf_test_calibration"
            | "grf_get_scores" => 600,
            "grf_get_tree" | "grf_generate_causal_data" => 300,
            _ => 3600,
        };
        assert_eq!(node.timeout_secs, expected_timeout, "{kind}");
        let expected_memory = match kind {
            "grf_get_forest_weights" | "grf_merge_forests" => "8Gi",
            "grf_get_tree" => "2Gi",
            "grf_generate_causal_data" => "1Gi",
            _ => "4Gi",
        };
        assert_eq!(
            node.resources.memory.as_deref(),
            Some(expected_memory),
            "{kind}"
        );
    }
}
