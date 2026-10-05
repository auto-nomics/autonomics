//! Golden parity tests for the bulk-rnaseq plugin: each `[[nodes]]` entry
//! (limma_voom, wgcna) must compile to the same `Container` spec the legacy
//! Rust wrappers (`nodes-io/src/limma_voom_container.rs`,
//! `nodes-io/src/wgcna_container.rs`) produced. The plugin directory lives
//! outside this repository (`/mnt/projects/node-plugins/bulk-rnaseq` by
//! default, overridable via `NODE_PLUGINS_ROOT`); the test is skipped when
//! absent so CI without the plugin checkout stays green.
//!
//! bulk-rnaseq is a baked-runner family: the pinned image carries
//! `/opt/autonomics/limma_voom_runner.R` and `/opt/autonomics/wgcna_runner.R`,
//! so each compiled command is interpreter + fixed argv with `script: None`,
//! and parity hangs on the `AUTONOMICS_LIMMA_*` / `AUTONOMICS_WGCNA_*` env
//! names rendering byte-identically to the legacy `BTreeMap::from([...])`
//! channel. List-valued params travel comma-joined — byte-identical to the
//! legacy `join(",")` for every legacy-legal input.

use std::path::PathBuf;

use container_plugin::compile::spec_compile::compile_container_spec;
use container_plugin::manifest::PluginManifest;
use serde_json::json;

/// Pinned after the 1.0.1 rebuild (the 1.0.0 runner lacked the limma
/// covariate / contrast_levels contract checks; see the manifest comment).
const IMAGE_REFERENCE: &str = "ghcr.io/auto-nomics/autonomics/bulk-rnaseq@sha256:23071886af3864a0338242987980727753923a0a67f362aef7366d1277c39734";

fn plugin_root() -> Option<PathBuf> {
    let root = std::env::var_os("NODE_PLUGINS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/projects/node-plugins"));
    let manifest = root.join("bulk-rnaseq").join("manifest.toml");
    manifest.is_file().then_some(root)
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let text = std::fs::read_to_string(root.join("bulk-rnaseq").join("manifest.toml")).unwrap();
    // Inline script_file the way the loader does.
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        if let Some(relative) = node.command.script_file.clone() {
            let source = std::fs::read_to_string(root.join("bulk-rnaseq").join(&relative)).unwrap();
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
fn limma_voom_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "limma_voom");

    // Required params only: every defaulted param must resolve through the
    // manifest defaults exactly as the legacy serde defaults did.
    let params = json!({
        "outcome_var": "condition",
        "contrast_var": "condition",
    });
    let compiled =
        compile_container_spec(node, &manifest.image, &manifest.panels, &params).unwrap();

    assert_eq!(compiled.image, IMAGE_REFERENCE);
    // Byte-exact command: interpreter + fixed argv into the baked runner.
    assert_eq!(
        compiled.command,
        vec![
            "Rscript".to_string(),
            "--vanilla".to_string(),
            "/opt/autonomics/limma_voom_runner.R".to_string()
        ]
    );
    assert!(compiled.script.is_none());
    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert!(matches!(
        compiled.pull_policy,
        container_runtime::PullPolicy::Missing
    ));
    assert_eq!(compiled.timeout_secs, 3600);
    // Deliberate delta: the legacy default was "/artifacts/limma_voom_container";
    // the plugin drops the `_container` suffix together with the kind rename
    // (the same rule the ldsc/deseq2 migrations applied).
    assert_eq!(compiled.artifact_prefix, "/artifacts/limma_voom");
    assert_eq!(compiled.workdir, None);
    // The legacy wrapper bound no panels and no panel bundles.
    assert!(compiled.panels.is_empty());
    assert!(compiled.panel_bundles.is_empty());
    // Legacy DEFAULT_CPUS/MEMORY/PIDS_LIMIT/SHM_SIZE.
    assert_eq!(compiled.cpus, Some(2.0));
    assert_eq!(compiled.memory.as_deref(), Some("6Gi"));
    assert_eq!(compiled.pids_limit, Some(256));
    assert_eq!(compiled.shm_size.as_deref(), Some("1Gi"));

    // Outputs byte-exact: five artifacts with the legacy formats.
    let expected_outputs = [
        ("results.tsv", "limma_voom_results_tsv"),
        ("voom_weights.tsv", "limma_voom_weights_tsv"),
        ("normalized_expression.tsv", "limma_voom_normalized_tsv"),
        ("contrast_summary.tsv", "limma_voom_contrast_summary_tsv"),
        ("run_report.json", "limma_voom_run_report_json"),
    ];
    assert_eq!(compiled.outputs.len(), expected_outputs.len());
    for (output, (path, format)) in compiled.outputs.iter().zip(expected_outputs) {
        assert_eq!(output.path, path);
        assert_eq!(output.format.as_deref(), Some(format));
    }

    // Env channel: defaults render to the legacy serde defaults, and the
    // joined-string list params render as the empty string the runner's
    // split_csv folds into an empty vector.
    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_LIMMA_OUTCOME")
            .map(String::as_str),
        Some("condition")
    );
    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_LIMMA_CONTRAST_VAR")
            .map(String::as_str),
        Some("condition")
    );
    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_LIMMA_CONTRAST_LEVELS")
            .map(String::as_str),
        Some("")
    );
    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_LIMMA_COVARIATES")
            .map(String::as_str),
        Some("")
    );
    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_LIMMA_OUTCOME_MODE")
            .map(String::as_str),
        Some("categorical")
    );
    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_LIMMA_NORMALIZATION")
            .map(String::as_str),
        Some("tmm")
    );
    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_LIMMA_ALPHA")
            .map(String::as_str),
        Some("0.05")
    );
    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_LIMMA_THREADS")
            .map(String::as_str),
        Some("1")
    );

    // A fully-populated spec renders the list params comma-joined — the
    // byte-identical channel the legacy join(",") produced.
    let full = json!({
        "outcome_var": "condition",
        "contrast_var": "condition",
        "contrast_levels": "ctrl,case",
        "covariates": "batch,sex",
        "outcome_mode": "categorical",
        "normalization": "quantile",
        "alpha": 0.01,
    });
    let compiled_full =
        compile_container_spec(node, &manifest.image, &manifest.panels, &full).unwrap();
    assert_eq!(
        compiled_full
            .env
            .get("AUTONOMICS_LIMMA_CONTRAST_LEVELS")
            .map(String::as_str),
        Some("ctrl,case")
    );
    assert_eq!(
        compiled_full
            .env
            .get("AUTONOMICS_LIMMA_COVARIATES")
            .map(String::as_str),
        Some("batch,sex")
    );
    assert_eq!(
        compiled_full
            .env
            .get("AUTONOMICS_LIMMA_NORMALIZATION")
            .map(String::as_str),
        Some("quantile")
    );
    assert_eq!(
        compiled_full
            .env
            .get("AUTONOMICS_LIMMA_ALPHA")
            .map(String::as_str),
        Some("0.01")
    );
}

#[test]
fn wgcna_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "wgcna");

    // Every wgcna param has a manifest default, so the empty spec compiles —
    // the legacy spec was all-defaults too.
    let compiled =
        compile_container_spec(node, &manifest.image, &manifest.panels, &json!({})).unwrap();

    assert_eq!(compiled.image, IMAGE_REFERENCE);
    assert_eq!(
        compiled.command,
        vec![
            "Rscript".to_string(),
            "--vanilla".to_string(),
            "/opt/autonomics/wgcna_runner.R".to_string()
        ]
    );
    assert!(compiled.script.is_none());
    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert!(matches!(
        compiled.pull_policy,
        container_runtime::PullPolicy::Missing
    ));
    assert_eq!(compiled.timeout_secs, 7200);
    assert_eq!(compiled.artifact_prefix, "/artifacts/wgcna");
    assert!(compiled.panels.is_empty());
    assert!(compiled.panel_bundles.is_empty());
    // Legacy DEFAULT_CPUS/MEMORY/PIDS_LIMIT/SHM_SIZE.
    assert_eq!(compiled.cpus, Some(4.0));
    assert_eq!(compiled.memory.as_deref(), Some("12Gi"));
    assert_eq!(compiled.pids_limit, Some(256));
    assert_eq!(compiled.shm_size.as_deref(), Some("2Gi"));

    let expected_outputs = [
        ("soft_threshold.tsv", "wgcna_soft_threshold_tsv"),
        ("adjacency_stats.tsv", "wgcna_adjacency_stats_tsv"),
        ("tom_stats.tsv", "wgcna_tom_stats_tsv"),
        ("modules.tsv", "wgcna_modules_tsv"),
        ("module_eigengenes.tsv", "wgcna_eigengenes_tsv"),
        ("run_report.json", "wgcna_run_report_json"),
    ];
    assert_eq!(compiled.outputs.len(), expected_outputs.len());
    for (output, (path, format)) in compiled.outputs.iter().zip(expected_outputs) {
        assert_eq!(output.path, path);
        assert_eq!(output.format.as_deref(), Some(format));
    }

    // Defaults render the legacy serde defaults.
    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_WGCNA_NETWORK_TYPE")
            .map(String::as_str),
        Some("signed")
    );
    assert_eq!(
        compiled.env.get("AUTONOMICS_WGCNA_COR").map(String::as_str),
        Some("pearson")
    );
    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_WGCNA_MIN_MODULE_SIZE")
            .map(String::as_str),
        Some("30")
    );
    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_WGCNA_DEEP_SPLIT")
            .map(String::as_str),
        Some("2")
    );
    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_WGCNA_MERGE_THRESHOLD")
            .map(String::as_str),
        Some("0.25")
    );
    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_WGCNA_MAX_BLOCK_SIZE")
            .map(String::as_str),
        Some("5000")
    );
    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_WGCNA_THREADS")
            .map(String::as_str),
        Some("1")
    );

    // Overrides render through the same channel.
    let tuned = json!({
        "network_type": "unsigned",
        "cor": "bicor",
        "min_module_size": 50,
        "deep_split": 3,
        "merge_threshold": 0.3,
        "max_block_size": 8000,
    });
    let compiled_tuned =
        compile_container_spec(node, &manifest.image, &manifest.panels, &tuned).unwrap();
    assert_eq!(
        compiled_tuned
            .env
            .get("AUTONOMICS_WGCNA_NETWORK_TYPE")
            .map(String::as_str),
        Some("unsigned")
    );
    assert_eq!(
        compiled_tuned
            .env
            .get("AUTONOMICS_WGCNA_COR")
            .map(String::as_str),
        Some("bicor")
    );
    assert_eq!(
        compiled_tuned
            .env
            .get("AUTONOMICS_WGCNA_MIN_MODULE_SIZE")
            .map(String::as_str),
        Some("50")
    );
    assert_eq!(
        compiled_tuned
            .env
            .get("AUTONOMICS_WGCNA_MERGE_THRESHOLD")
            .map(String::as_str),
        Some("0.3")
    );
    assert_eq!(
        compiled_tuned
            .env
            .get("AUTONOMICS_WGCNA_MAX_BLOCK_SIZE")
            .map(String::as_str),
        Some("8000")
    );
}

/// The moved build tree stays the source of truth for the image: the baked
/// runners must keep the load-bearing statistical calls and — added in the
/// 1.0.1 rebuild — the limma contract checks the manifest DSL cannot express.
#[test]
fn baked_runners_keep_the_semantic_contract_markers() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let limma =
        std::fs::read_to_string(root.join("bulk-rnaseq").join("limma_voom_runner.R")).unwrap();
    // Statistical contract.
    assert!(limma.contains("edgeR::DGEList"));
    assert!(limma.contains("edgeR::calcNormFactors"));
    assert!(limma.contains("limma::voom"));
    assert!(limma.contains("limma::lmFit"));
    assert!(limma.contains("limma::eBayes"));
    assert!(limma.contains("limma::makeContrasts"));
    // The 1.0.1 additions: legacy validate() rules enforced runner-side.
    assert!(
        limma.contains("contrast_levels must contain exactly two entries for categorical outcome")
    );
    assert!(limma.contains("covariates cannot contain duplicates"));
    assert!(limma.contains("covariate `%s` is reserved"));
    assert!(limma.contains("covariate `%s` must be a simple R identifier"));

    let wgcna = std::fs::read_to_string(root.join("bulk-rnaseq").join("wgcna_runner.R")).unwrap();
    assert!(wgcna.contains("WGCNA::pickSoftThreshold"));
    assert!(wgcna.contains("WGCNA::blockwiseModules"));
    assert!(wgcna.contains("WGCNA::signedKME"));
    // Every legacy single-field validation lives in the runner.
    assert!(wgcna.contains("network_type must be signed or unsigned"));
    assert!(wgcna.contains("cor must be pearson or bicor"));
    assert!(wgcna.contains("min_module_size must be at least 5"));
    assert!(wgcna.contains("deep_split must be between 0 and 4"));
    assert!(wgcna.contains("merge_threshold must lie strictly between 0 and 1"));
    assert!(wgcna.contains("max_block_size must be at least 500"));
    // Optional metadata input stays optional when the DSL wires only INPUT0.
    assert!(wgcna.contains("if (nzchar(metadata_path))"));
}
