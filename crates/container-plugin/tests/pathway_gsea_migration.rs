//! Golden parity tests for the pathway-gsea plugin: the `[[nodes]]` entry
//! must compile to the same `Container` spec the legacy Rust wrapper
//! (`nodes-io/src/pathway_gsea_container.rs`) produced. The plugin
//! directory lives outside this repository
//! (`/mnt/projects/node-plugins/pathway-gsea` by default, overridable via
//! `NODE_PLUGINS_ROOT`); the test is skipped when absent so CI without
//! the plugin checkout stays green.

use std::path::PathBuf;

use container_plugin::compile::spec_compile::compile_container_spec;
use container_plugin::manifest::PluginManifest;
use serde_json::json;

fn plugin_root() -> Option<PathBuf> {
    let root = std::env::var_os("NODE_PLUGINS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/projects/node-plugins"));
    let manifest = root.join("pathway-gsea").join("manifest.toml");
    manifest.is_file().then_some(root)
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let text = std::fs::read_to_string(root.join("pathway-gsea").join("manifest.toml")).unwrap();
    // Inline script_file the way the loader does.
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        if let Some(relative) = node.command.script_file.clone() {
            let source =
                std::fs::read_to_string(root.join("pathway-gsea").join(&relative)).unwrap();
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
fn pathway_gsea_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "pathway_gsea");

    let compiled =
        compile_container_spec(node, &manifest.image, &manifest.panels, &json!({})).unwrap();

    assert_eq!(
        compiled.image,
        "ghcr.io/auto-nomics/autonomics/pathway-gsea@sha256:1eb3a32abe2f910e8a3e2c7b5cd8d9f45bf267dc7c57605103e5c30310740275"
    );
    assert_eq!(compiled.command, vec!["Rscript".to_string()]);
    assert_eq!(compiled.outputs.len(), 2);
    assert_eq!(compiled.outputs[0].path, "fgsea_report.tsv");
    assert_eq!(
        compiled.outputs[0].format.as_deref(),
        Some("fgsea_report_tsv")
    );
    assert_eq!(compiled.outputs[1].path, "fgsea_report.json");
    assert_eq!(
        compiled.outputs[1].format.as_deref(),
        Some("fgsea_report_json")
    );
    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert!(matches!(
        compiled.pull_policy,
        container_runtime::PullPolicy::Missing
    ));
    // Legacy DEFAULT_TIMEOUT_SECS. The legacy spec accepted a per-instance
    // override; the plugin DSL pins it per kind.
    assert_eq!(compiled.timeout_secs, 3600);
    // Deliberate delta: the legacy default was
    // "/artifacts/pathway_gsea_container"; the plugin follows the kind
    // rename (`_container` suffix dropped), the same rule the ldsc and
    // mrpresso migrations applied.
    assert_eq!(compiled.artifact_prefix, "/artifacts/pathway_gsea");
    assert_eq!(compiled.workdir, None);
    // The legacy wrapper bound no panels.
    assert!(compiled.panels.is_empty());
    assert!(compiled.panel_bundles.is_empty());
    // Legacy container_spec resolved these unconditionally
    // (DEFAULT_CPUS/DEFAULT_MEMORY/DEFAULT_PIDS_LIMIT/DEFAULT_SHM_SIZE);
    // the manifest pins them under [nodes.resources], so the compiled
    // resources are byte-equal.
    assert_eq!(compiled.cpus, Some(2.0));
    assert_eq!(compiled.memory.as_deref(), Some("8Gi"));
    assert_eq!(compiled.pids_limit, Some(512));
    assert_eq!(compiled.shm_size.as_deref(), Some("1Gi"));
    assert!(compiled.files.is_empty());

    // Legacy env contract with all spec defaults; the env variable names
    // are the exact ones the legacy wrapper used.
    assert_eq!(compiled.env.len(), 7);
    assert_eq!(compiled.env.get("AUTONOMICS_GENE_COL").unwrap(), "gene");
    assert_eq!(compiled.env.get("AUTONOMICS_SCORE_COL").unwrap(), "score");
    assert_eq!(
        compiled.env.get("AUTONOMICS_PATHWAY_COL").unwrap(),
        "pathway_name"
    );
    assert_eq!(
        compiled.env.get("AUTONOMICS_PATHWAY_GENE_COL").unwrap(),
        "gene"
    );
    assert_eq!(compiled.env.get("AUTONOMICS_MIN_SIZE").unwrap(), "15");
    assert_eq!(compiled.env.get("AUTONOMICS_MAX_SIZE").unwrap(), "500");
    // serde_json renders f64 1e-50 as "1e-50"; the legacy Rust
    // `f64::to_string` spelled out the full decimal expansion. Equal after
    // float parsing (the documented f64 env-rendering nuance).
    assert_eq!(compiled.env.get("AUTONOMICS_EPS").unwrap(), "1e-50");

    // Script parity is semantic, not byte-exact — though here the legacy
    // wrapper already read every parameter from AUTONOMICS_* env vars, so
    // the plugin script is the legacy embedded R code with the validate()
    // guards prepended. Assert the load-bearing tokens.
    let script = compiled.script.as_deref().unwrap();
    // Prologue: env contract and the validation the DSL cannot express,
    // enforced with the legacy error messages (min_size >= 1 and eps > 0
    // are DSL bounds instead).
    assert!(script.contains("gene_col <- Sys.getenv(\"AUTONOMICS_GENE_COL\")"));
    assert!(script.contains("eps <- as.numeric(Sys.getenv(\"AUTONOMICS_EPS\"))"));
    assert!(script.contains("stop(\"gene_col cannot be empty\")"));
    assert!(script.contains("stop(\"pathway_gene_col cannot be empty\")"));
    assert!(script.contains("stop(\"max_size must be at least min_size\")"));
    // Rank table: unique non-empty genes, non-null scores.
    assert!(script.contains(
        "rank_table <- data.table::fread(Sys.getenv(\"AUTONOMICS_INPUT0\"), check.names = FALSE)"
    ));
    assert!(script.contains("stats <- setNames(scores, genes)"));
    // The GMT/TSV gene-set branch on input 1.
    assert!(script.contains("grepl(\"\\\\.gmt$\", Sys.getenv(\"AUTONOMICS_INPUT1\"))"));
    assert!(script.contains("pathways <- split(set_genes, set_names)"));
    // The official call: same package path, argument names, and order.
    assert!(script.contains("result <- fgsea::fgseaMultilevel("));
    assert!(script.contains("minSize = min_size"));
    assert!(script.contains("maxSize = max_size"));
    assert!(script.contains("eps = eps"));
    // Epilogue: leadingEdge join, report columns, TSV + JSON artifacts.
    assert!(script.contains(
        "result$leadingEdge <- vapply(result$leadingEdge, paste, character(1), collapse = \";\")"
    ));
    assert!(script.contains("report <- result[, .(pathway, pval, padj, NES, size, leadingEdge)]"));
    assert!(script.contains(
        "data.table::fwrite(report, Sys.getenv(\"AUTONOMICS_OUTPUT0\"), sep = \"\\t\", quote = FALSE)"
    ));
    assert!(
        script.contains("jsonlite::toJSON(json, auto_unbox = TRUE, pretty = TRUE, na = \"null\")")
    );
    assert!(script.contains("Sys.getenv(\"AUTONOMICS_OUTPUT1\")"));
}

#[test]
fn pathway_gsea_plugin_renders_submitted_values_into_env() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "pathway_gsea");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({
            "gene_col": "SYMBOL",
            "score_col": "t_stat",
            "pathway_col": "set_name",
            "pathway_gene_col": "ensembl_id",
            "min_size": 5,
            "max_size": 100,
            "eps": 1e-10
        }),
    )
    .unwrap();

    assert_eq!(compiled.env.get("AUTONOMICS_GENE_COL").unwrap(), "SYMBOL");
    assert_eq!(compiled.env.get("AUTONOMICS_SCORE_COL").unwrap(), "t_stat");
    assert_eq!(
        compiled.env.get("AUTONOMICS_PATHWAY_COL").unwrap(),
        "set_name"
    );
    assert_eq!(
        compiled.env.get("AUTONOMICS_PATHWAY_GENE_COL").unwrap(),
        "ensembl_id"
    );
    assert_eq!(compiled.env.get("AUTONOMICS_MIN_SIZE").unwrap(), "5");
    assert_eq!(compiled.env.get("AUTONOMICS_MAX_SIZE").unwrap(), "100");
    // serde_json renders f64 1e-10 as "1e-10".
    assert_eq!(compiled.env.get("AUTONOMICS_EPS").unwrap(), "1e-10");
}

#[test]
fn pathway_gsea_plugin_schema_defaults_every_param() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "pathway_gsea");

    let schema =
        serde_json::to_value(container_plugin::compile::compile_schema(&node.params)).unwrap();
    // Every legacy spec field carried a serde default, so nothing is
    // required.
    let required = schema["required"].as_array().unwrap();
    assert!(required.is_empty());
    let min_size = &schema["properties"]["min_size"];
    assert_eq!(min_size["type"], "integer");
    assert_eq!(min_size["default"], 15);
    // The legacy validate() "min_size must be greater than zero" becomes
    // the inclusive integer bound minimum = 1.
    assert_eq!(min_size["minimum"], 1.0);
    let max_size = &schema["properties"]["max_size"];
    assert_eq!(max_size["type"], "integer");
    assert_eq!(max_size["default"], 500);
    assert_eq!(max_size["minimum"], 1.0);
    // The legacy validate() "eps must be finite and greater than zero"
    // becomes exclusiveMinimum = 0 (JSON numbers are finite by definition).
    let eps = &schema["properties"]["eps"];
    assert_eq!(eps["type"], "number");
    assert_eq!(eps["default"], json!(1e-50));
    assert_eq!(eps["exclusiveMinimum"], 0.0);
    let gene_col = &schema["properties"]["gene_col"];
    assert_eq!(gene_col["type"], "string");
    assert_eq!(gene_col["default"], "gene");
    assert_eq!(
        schema["properties"]["pathway_col"]["default"],
        "pathway_name"
    );
    assert_eq!(schema["properties"]["pathway_gene_col"]["default"], "gene");
}
