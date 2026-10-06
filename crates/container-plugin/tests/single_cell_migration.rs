//! Golden parity tests for the single-cell plugin family: each `[[nodes]]`
//! entry must compile to the same `Container` spec the legacy Rust wrappers
//! (`single_cell_container`, `single_cell_h5ad`) produced. The plugin
//! directory lives outside this repository (`/mnt/projects/node-plugins/
//! single-cell` by default, overridable via `NODE_PLUGINS_ROOT`); the tests
//! are skipped when absent so CI without the plugin checkout stays green.

use std::path::PathBuf;

use container_plugin::compile::spec_compile::compile_container_spec;
use container_plugin::manifest::PluginManifest;
use serde_json::json;

const IMAGE: &str = "ghcr.io/auto-nomics/autonomics/single-cell-preprocessor@sha256:c34c26428d13804c2528bc734805c1ccfe909353604ac08da0ce9c68890e7e18";
const CELLTYPIST_MODEL_BUNDLE: &str = "wjixiang/catalog-celltypist-models-pan-immune";

fn plugin_root() -> Option<PathBuf> {
    let explicit = std::env::var_os("NODE_PLUGINS_ROOT");
    let root = explicit
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/projects/node-plugins"));
    let manifest = root.join("single-cell").join("manifest.toml");
    if manifest.is_file() {
        return Some(root);
    }
    if explicit.is_some() {
        // fix-review R04 (2026-10-05): an explicitly set NODE_PLUGINS_ROOT
        // means migration parity was requested. A missing family must fail
        // loudly — early-returns here used to count as *passed* tests, so a
        // green summary claimed coverage that never ran.
        panic!(
            "NODE_PLUGINS_ROOT is set but the single-cell family is not deployed \
             under it ({}) — migration parity cannot run. Deploy the family \
             or unset NODE_PLUGINS_ROOT to skip these tests deliberately.",
            manifest.display()
        );
    }
    eprintln!("skipping: single-cell plugin directory not present (NODE_PLUGINS_ROOT unset)");
    None
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let text = std::fs::read_to_string(root.join("single-cell").join("manifest.toml")).unwrap();
    // Inline script_file the way the loader does.
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        if let Some(relative) = node.command.script_file.clone() {
            let source = std::fs::read_to_string(root.join("single-cell").join(&relative)).unwrap();
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
fn single_cell_preprocessor_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "single_cell_preprocessor");

    let compiled =
        compile_container_spec(node, &manifest.image, &manifest.panels, &json!({})).unwrap();

    assert_eq!(compiled.image, IMAGE);
    // Baked-runner style: the argv is the image's preprocess.py, no script.
    assert_eq!(
        compiled.command,
        vec![
            "python".to_string(),
            "/opt/autonomics/preprocess.py".to_string()
        ]
    );
    assert_eq!(compiled.script, None);
    assert_eq!(compiled.outputs.len(), 2);
    assert_eq!(compiled.outputs[0].path, "preprocess_report.json");
    assert_eq!(
        compiled.outputs[0].format.as_deref(),
        Some("single_cell_preprocess_report_json")
    );
    assert_eq!(compiled.outputs[1].path, "preprocessed.h5ad");
    assert_eq!(compiled.outputs[1].format.as_deref(), Some("h5ad"));
    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert!(matches!(
        compiled.pull_policy,
        container_runtime::PullPolicy::Missing
    ));
    assert_eq!(compiled.timeout_secs, 3600);
    // The legacy wrapper defaulted to the suffixed spelling; kept verbatim.
    assert_eq!(
        compiled.artifact_prefix,
        "/artifacts/single_cell_preprocessor_container"
    );
    assert_eq!(compiled.workdir, None);
    assert!(compiled.panels.is_empty());
    assert_eq!(compiled.cpus, Some(2.0));
    assert_eq!(compiled.memory.as_deref(), Some("8Gi"));
    assert_eq!(compiled.pids_limit, Some(512));
    assert_eq!(compiled.shm_size.as_deref(), Some("1Gi"));
    // The model panel is family-level, so the preprocessor carries the
    // binding too even though it never reads the mount (ldsc munge
    // precedent).
    assert_eq!(compiled.panel_bundles.len(), 1);
    assert_eq!(compiled.panel_bundles[0].panel_id, CELLTYPIST_MODEL_BUNDLE);
    assert_eq!(
        compiled.panel_bundles[0].mount_path,
        "/panels/celltypist_model"
    );
    // Byte-exact env map: the same five keys with the same default values
    // the legacy wrapper built.
    assert_eq!(compiled.env.len(), 5);
    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_SINGLE_CELL_OPERATION")
            .unwrap(),
        "inspect"
    );
    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_SINGLE_CELL_MIN_GENES")
            .unwrap(),
        "0"
    );
    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_SINGLE_CELL_MIN_CELLS")
            .unwrap(),
        "0"
    );
    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_SINGLE_CELL_NORMALIZE_TOTAL")
            .unwrap(),
        "false"
    );
    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_SINGLE_CELL_METADATA_SEP")
            .unwrap(),
        "auto"
    );
}

#[test]
fn single_cell_preprocessor_plugin_renders_submitted_values_into_env() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "single_cell_preprocessor");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({
            "operation": "ingest",
            "min_genes": 10,
            "min_cells": 20,
            "normalize_total": true,
            "metadata_separator": "comma"
        }),
    )
    .unwrap();

    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_SINGLE_CELL_OPERATION")
            .unwrap(),
        "ingest"
    );
    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_SINGLE_CELL_MIN_GENES")
            .unwrap(),
        "10"
    );
    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_SINGLE_CELL_MIN_CELLS")
            .unwrap(),
        "20"
    );
    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_SINGLE_CELL_NORMALIZE_TOTAL")
            .unwrap(),
        "true"
    );
    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_SINGLE_CELL_METADATA_SEP")
            .unwrap(),
        "comma"
    );
}

/// Per-kind legacy expectations for the ten H5AD workflow nodes. Every row
/// is asserted explicitly by
/// `single_cell_h5ad_family_compiles_to_the_legacy_wrapper_contracts`.
struct Expected {
    kind: &'static str,
    operation: &'static str,
    timeout_secs: u64,
    cpus: f64,
    memory: &'static str,
    shm_size: &'static str,
    artifact_prefix: &'static str,
    outputs: &'static [(&'static str, &'static str)],
    /// Required params the legacy build call had to supply.
    values: serde_json::Value,
    /// Explicit default-env spot checks beyond the shared workflow keys.
    env: &'static [(&'static str, &'static str)],
}

fn family_expectations() -> Vec<Expected> {
    vec![
        Expected {
            kind: "h5ad_qc_filter",
            operation: "qc_filter",
            timeout_secs: 3600,
            cpus: 2.0,
            memory: "8Gi",
            shm_size: "1Gi",
            artifact_prefix: "/artifacts/h5ad_qc_filter",
            outputs: &[
                ("output.h5ad", "h5ad"),
                ("report.json", "single_cell_workflow_report_json"),
            ],
            values: json!({}),
            env: &[
                ("SC_P_INT_MIN_GENES", "0"),
                ("SC_P_NUM_MAX_PCT_MT", "100.0"),
                ("SC_P_STR_MT_GENE_PATTERN", "^MT-"),
                ("SC_P_STR_RB_GENE_PATTERN", "^RPL|^RPS"),
            ],
        },
        Expected {
            kind: "h5ad_pca_neighbors_umap_leiden",
            operation: "pca_neighbors_umap_leiden",
            timeout_secs: 7200,
            cpus: 4.0,
            memory: "16Gi",
            shm_size: "2Gi",
            artifact_prefix: "/artifacts/h5ad_pca_neighbors_umap_leiden",
            outputs: &[
                ("output.h5ad", "h5ad"),
                ("report.json", "single_cell_workflow_report_json"),
            ],
            values: json!({}),
            env: &[
                ("SC_P_INT_N_PCS", "30"),
                ("SC_P_INT_N_NEIGHBORS", "15"),
                ("SC_P_NUM_MIN_DIST", "0.5"),
                ("SC_P_BOOL_SCALE", "false"),
            ],
        },
        Expected {
            kind: "h5ad_celltypist_annotate",
            operation: "celltypist_annotate",
            timeout_secs: 3600,
            cpus: 2.0,
            memory: "8Gi",
            shm_size: "1Gi",
            artifact_prefix: "/artifacts/h5ad_celltypist_annotate",
            outputs: &[
                ("output.h5ad", "h5ad"),
                ("report.json", "single_cell_workflow_report_json"),
            ],
            values: json!({}),
            env: &[
                ("SC_P_BOOL_MAJORITY_VOTING", "false"),
                ("SC_P_STR_MODEL_FILE", "Immune_All_Low.pkl"),
                ("SC_MODEL_DIR", "/panels/celltypist_model"),
            ],
        },
        Expected {
            kind: "h5ad_subset_by_obs",
            operation: "subset_by_obs",
            timeout_secs: 3600,
            cpus: 2.0,
            memory: "8Gi",
            shm_size: "1Gi",
            artifact_prefix: "/artifacts/h5ad_subset_by_obs",
            outputs: &[
                ("output.h5ad", "h5ad"),
                ("report.json", "single_cell_workflow_report_json"),
            ],
            values: json!({}),
            env: &[("SC_P_STR_JOIN_COLUMN", "cell_id")],
        },
        Expected {
            kind: "sc_dense_ingest",
            operation: "dense_ingest",
            timeout_secs: 3600,
            cpus: 2.0,
            memory: "8Gi",
            shm_size: "1Gi",
            artifact_prefix: "/artifacts/sc_dense_ingest",
            outputs: &[
                ("output.h5ad", "h5ad"),
                ("report.json", "single_cell_workflow_report_json"),
            ],
            values: json!({"orientation": "genes_by_cells"}),
            env: &[
                ("SC_P_STR_ORIENTATION", "genes_by_cells"),
                ("SC_P_STR_DELIMITER", "auto"),
                ("SC_P_BOOL_HAS_HEADER", "true"),
                ("SC_P_STR_SAMPLE_LABEL", ""),
            ],
        },
        Expected {
            kind: "h5ad_rank_genes_groups",
            operation: "rank_genes_groups",
            timeout_secs: 3600,
            cpus: 2.0,
            memory: "8Gi",
            shm_size: "1Gi",
            artifact_prefix: "/artifacts/h5ad_rank_genes_groups",
            outputs: &[
                ("rank_genes_groups.parquet", "parquet"),
                ("report.json", "single_cell_workflow_report_json"),
                ("output.h5ad", "h5ad"),
            ],
            values: json!({"groupby": "leiden"}),
            env: &[
                ("SC_P_STR_GROUPBY", "leiden"),
                ("SC_P_STR_METHOD", "wilcoxon"),
                ("SC_P_STR_REFERENCE", "rest"),
                ("SC_P_INT_N_GENES", "100"),
            ],
        },
        Expected {
            kind: "h5ad_cluster_mean_expression",
            operation: "cluster_mean_expression",
            timeout_secs: 3600,
            cpus: 2.0,
            memory: "8Gi",
            shm_size: "1Gi",
            artifact_prefix: "/artifacts/h5ad_cluster_mean_expression",
            outputs: &[("cluster_mean_expression.parquet", "parquet")],
            values: json!({"groupby": "leiden"}),
            env: &[
                ("SC_P_STR_GROUPBY", "leiden"),
                ("SC_P_LST_GENES", ""),
                ("SC_P_STR_NORMALIZE", "cp10k"),
                // The legacy Rust spec declared false, but the shipped
                // params.json never materialized Rust defaults and
                // workflow.py applies True; the manifest keeps the
                // effective value.
                ("SC_P_BOOL_INCLUDE_PERCENT_EXPRESSED", "true"),
            ],
        },
        Expected {
            kind: "gene_set_score",
            operation: "gene_set_score",
            timeout_secs: 3600,
            cpus: 2.0,
            memory: "8Gi",
            shm_size: "1Gi",
            artifact_prefix: "/artifacts/gene_set_score",
            outputs: &[
                ("output.h5ad", "h5ad"),
                ("report.json", "single_cell_workflow_report_json"),
            ],
            values: json!({"gene_sets": "{\"cytotoxic\": [\"NKG7\", \"GNLY\"]}"}),
            env: &[
                (
                    "SC_P_JSON_GENE_SETS",
                    "{\"cytotoxic\": [\"NKG7\", \"GNLY\"]}",
                ),
                ("SC_P_INT_CTRL_SIZE", "50"),
                ("SC_P_INT_RANDOM_STATE", "0"),
            ],
        },
        Expected {
            kind: "h5ad_marker_annotate",
            operation: "marker_annotate",
            timeout_secs: 3600,
            cpus: 2.0,
            memory: "8Gi",
            shm_size: "1Gi",
            artifact_prefix: "/artifacts/h5ad_marker_annotate",
            outputs: &[
                ("output.h5ad", "h5ad"),
                ("report.json", "single_cell_workflow_report_json"),
            ],
            values: json!({"marker_sets": "{\"T cell\": [\"CD3D\", \"CD3E\"]}"}),
            env: &[
                (
                    "SC_P_JSON_MARKER_SETS",
                    "{\"T cell\": [\"CD3D\", \"CD3E\"]}",
                ),
                ("SC_P_STR_GROUPBY", ""),
                ("SC_P_NUM_MIN_SCORE", "0.0"),
                ("SC_P_STR_UNKNOWN_LABEL", "Unknown"),
            ],
        },
        Expected {
            kind: "h5ad_ucell_score",
            operation: "ucell_score",
            timeout_secs: 3600,
            cpus: 2.0,
            memory: "8Gi",
            shm_size: "1Gi",
            artifact_prefix: "/artifacts/h5ad_ucell_score",
            outputs: &[
                ("output.h5ad", "h5ad"),
                ("report.json", "single_cell_workflow_report_json"),
            ],
            values: json!({"gene_sets": "{\"cytotoxic\": [\"NKG7\", \"GNLY\"]}"}),
            env: &[(
                "SC_P_JSON_GENE_SETS",
                "{\"cytotoxic\": [\"NKG7\", \"GNLY\"]}",
            )],
        },
    ]
}

#[test]
fn single_cell_h5ad_family_compiles_to_the_legacy_wrapper_contracts() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);

    for expected in family_expectations() {
        let node = node_by_kind(&manifest, expected.kind);
        let compiled =
            compile_container_spec(node, &manifest.image, &manifest.panels, &expected.values)
                .unwrap_or_else(|error| panic!("{} failed to compile: {error}", expected.kind));

        assert_eq!(compiled.image, IMAGE, "{}", expected.kind);
        // Injected-script style: bare interpreter argv, shim staged at
        // /work/.autonomics/script by the runtime (the legacy shape was
        // `python` plus the staged workflow.py).
        assert_eq!(
            compiled.command,
            vec!["python".to_string()],
            "{}",
            expected.kind
        );
        let script = compiled
            .script
            .as_deref()
            .unwrap_or_else(|| panic!("{} must stage the runner shim", expected.kind));
        // Semantic shim markers, not byte equality with the legacy
        // injected workflow.py: params travel as typed SC_P_ env vars and
        // control hands off to the image-baked runner.
        assert!(script.contains("SC_P_"), "{}", expected.kind);
        assert!(
            script.contains("AUTONOMICS_SINGLE_CELL_PARAMS"),
            "{}",
            expected.kind
        );
        assert!(
            script
                .contains("runpy.run_path(\"/opt/autonomics/workflow.py\", run_name=\"__main__\")"),
            "{}",
            expected.kind
        );

        assert_eq!(compiled.network, "isolated", "{}", expected.kind);
        assert!(compiled.read_only_rootfs, "{}", expected.kind);
        assert!(
            matches!(compiled.pull_policy, container_runtime::PullPolicy::Missing),
            "{}",
            expected.kind
        );
        assert_eq!(compiled.workdir, None, "{}", expected.kind);
        assert!(compiled.panels.is_empty(), "{}", expected.kind);
        assert_eq!(
            compiled.timeout_secs, expected.timeout_secs,
            "{}",
            expected.kind
        );
        assert_eq!(
            compiled.artifact_prefix, expected.artifact_prefix,
            "{}",
            expected.kind
        );
        assert_eq!(compiled.cpus, Some(expected.cpus), "{}", expected.kind);
        assert_eq!(
            compiled.memory.as_deref(),
            Some(expected.memory),
            "{}",
            expected.kind
        );
        assert_eq!(compiled.pids_limit, Some(512), "{}", expected.kind);
        assert_eq!(
            compiled.shm_size.as_deref(),
            Some(expected.shm_size),
            "{}",
            expected.kind
        );

        let outputs: Vec<(&str, Option<&str>)> = compiled
            .outputs
            .iter()
            .map(|o| (o.path.as_str(), o.format.as_deref()))
            .collect();
        assert_eq!(
            outputs,
            expected
                .outputs
                .iter()
                .map(|(path, format)| (*path, Some(*format)))
                .collect::<Vec<_>>(),
            "{}",
            expected.kind
        );

        assert_eq!(
            compiled
                .env
                .get("AUTONOMICS_SINGLE_CELL_WORKFLOW")
                .map(String::as_str),
            Some(expected.operation),
            "{}",
            expected.kind
        );
        assert_eq!(
            compiled
                .env
                .get("AUTONOMICS_SINGLE_CELL_PARAMS")
                .map(String::as_str),
            Some("/work/.autonomics/files/params.json"),
            "{}",
            expected.kind
        );
        for (key, value) in expected.env {
            assert_eq!(
                compiled.env.get(*key).map(String::as_str),
                Some(*value),
                "{} env {key}",
                expected.kind
            );
        }

        // The family panel reaches every node; only celltypist reads it.
        let panel_ids: Vec<&str> = compiled
            .panel_bundles
            .iter()
            .map(|p| p.panel_id.as_str())
            .collect();
        assert_eq!(
            panel_ids,
            vec![CELLTYPIST_MODEL_BUNDLE],
            "{}",
            expected.kind
        );
    }
}

#[test]
fn single_cell_h5ad_plugin_renders_submitted_values_into_env() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);

    let qc = compile_container_spec(
        node_by_kind(&manifest, "h5ad_qc_filter"),
        &manifest.image,
        &manifest.panels,
        &json!({
            "min_genes": 10,
            "max_pct_mt": 15.5,
            "mt_gene_pattern": "^MT-2024-"
        }),
    )
    .unwrap();
    assert_eq!(qc.env.get("SC_P_INT_MIN_GENES").unwrap(), "10");
    // serde_json renders f64 15.5 as "15.5"; strings pass through the env
    // surface verbatim (podman never re-shells env values).
    assert_eq!(qc.env.get("SC_P_NUM_MAX_PCT_MT").unwrap(), "15.5");
    assert_eq!(qc.env.get("SC_P_STR_MT_GENE_PATTERN").unwrap(), "^MT-2024-");

    let embed = compile_container_spec(
        node_by_kind(&manifest, "h5ad_pca_neighbors_umap_leiden"),
        &manifest.image,
        &manifest.panels,
        &json!({
            "resolution": 0.8,
            "random_state": 17,
            "normalize": false
        }),
    )
    .unwrap();
    assert_eq!(embed.env.get("SC_P_NUM_RESOLUTION").unwrap(), "0.8");
    assert_eq!(embed.env.get("SC_P_INT_RANDOM_STATE").unwrap(), "17");
    assert_eq!(embed.env.get("SC_P_BOOL_NORMALIZE").unwrap(), "false");

    let marker = compile_container_spec(
        node_by_kind(&manifest, "h5ad_marker_annotate"),
        &manifest.image,
        &manifest.panels,
        &json!({
            "marker_sets": "{\"B cell\": [\"MS4A1\", \"CD79A\"]}",
            "groupby": "leiden",
            "min_score": 0.25
        }),
    )
    .unwrap();
    // JSON strings with spaces survive the env surface byte-for-byte; the
    // shim json.loads them back into the legacy params.json shape.
    assert_eq!(
        marker.env.get("SC_P_JSON_MARKER_SETS").unwrap(),
        "{\"B cell\": [\"MS4A1\", \"CD79A\"]}"
    );
    assert_eq!(marker.env.get("SC_P_STR_GROUPBY").unwrap(), "leiden");
    assert_eq!(marker.env.get("SC_P_NUM_MIN_SCORE").unwrap(), "0.25");
}

#[test]
fn single_cell_h5ad_plugin_schema_marks_required_and_optional_params() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);

    let dense = serde_json::to_value(container_plugin::compile::compile_schema(
        &node_by_kind(&manifest, "sc_dense_ingest").params,
    ))
    .unwrap();
    let required = dense["required"].as_array().unwrap();
    assert!(required.contains(&json!("orientation")));
    assert_eq!(dense["properties"]["sample_label"]["type"], "string");
    assert!(
        dense["properties"]["sample_label"]
            .get("minItems")
            .is_none()
    );
    assert_eq!(dense["properties"]["min_genes"]["type"], "integer");

    let marker = serde_json::to_value(container_plugin::compile::compile_schema(
        &node_by_kind(&manifest, "h5ad_marker_annotate").params,
    ))
    .unwrap();
    let required = marker["required"].as_array().unwrap();
    assert!(required.contains(&json!("marker_sets")));
    assert!(
        !required.iter().any(|name| name == "groupby"),
        "groupby is optional in the legacy spec"
    );

    let mean = serde_json::to_value(container_plugin::compile::compile_schema(
        &node_by_kind(&manifest, "h5ad_cluster_mean_expression").params,
    ))
    .unwrap();
    assert_eq!(mean["properties"]["genes"]["type"], "array");
    assert_eq!(mean["properties"]["genes"]["items"]["type"], "string");

    let ucell = serde_json::to_value(container_plugin::compile::compile_schema(
        &node_by_kind(&manifest, "h5ad_ucell_score").params,
    ))
    .unwrap();
    assert!(
        ucell["required"]
            .as_array()
            .unwrap()
            .contains(&json!("gene_sets"))
    );
}

#[test]
fn plugin_workflow_copy_stays_in_parity_with_the_legacy_runner_source() {
    // While the legacy wrapper still compiles, it include_str!s the repo
    // copy of workflow.py; the plugin build tree carries the image copy.
    // The two must stay identical until the wrapper is deleted.
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let plugin_copy = root.join("single-cell").join("workflow.py");
    let repo_copy = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../containers/single-cell-preprocessor/workflow.py"
    ));
    let (Some(plugin_source), Some(repo_source)) = (
        std::fs::read_to_string(&plugin_copy).ok(),
        std::fs::read_to_string(&repo_copy).ok(),
    ) else {
        eprintln!("skipping: one of the workflow.py copies is absent (wrapper deleted?)");
        return;
    };
    assert_eq!(
        plugin_source, repo_source,
        "plugin workflow.py diverged from the runner the legacy wrapper injects"
    );
}
