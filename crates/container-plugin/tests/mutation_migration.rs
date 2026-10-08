//! Golden parity tests for the mutation plugin family: each `[[nodes]]`
//! entry must compile to the same `Container` spec the legacy Rust wrapper
//! (`nodes-io/src/mutation_analysis_container.rs`) produced. The plugin
//! directory lives outside this repository
//! (`/mnt/projects/node-plugins/mutation` by default, overridable via
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
    let manifest = root.join("mutation").join("manifest.toml");
    if manifest.is_file() {
        return Some(root);
    }
    if explicit.is_some() {
        // fix-review R04 (2026-10-05): an explicitly set NODE_PLUGINS_ROOT
        // means migration parity was requested. A missing family must fail
        // loudly — early-returns here used to count as *passed* tests, so a
        // green summary claimed coverage that never ran.
        panic!(
            "NODE_PLUGINS_ROOT is set but the mutation family is not deployed \
             under it ({}) — migration parity cannot run. Deploy the family \
             or unset NODE_PLUGINS_ROOT to skip these tests deliberately.",
            manifest.display()
        );
    }
    eprintln!("skipping: mutation plugin directory not present (NODE_PLUGINS_ROOT unset)");
    None
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let text = std::fs::read_to_string(root.join("mutation").join("manifest.toml")).unwrap();
    // Inline script_file the way the loader does.
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        if let Some(relative) = node.command.script_file.clone() {
            let source = std::fs::read_to_string(root.join("mutation").join(&relative)).unwrap();
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

/// `maf_path` (and its optional sibling `clinical_path`) are provenance
/// labels per the legacy Spec: required in the schema, never sent to the
/// container. The compile under test must still submit the required one.
fn base_params() -> serde_json::Value {
    json!({ "maf_path": "/data/sample.maf" })
}

#[test]
fn mutation_analysis_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "mutation_analysis");

    let compiled =
        compile_container_spec(node, &manifest.image, &manifest.panels, &base_params()).unwrap();

    assert_eq!(
        compiled.image,
        "ghcr.io/auto-nomics/autonomics/mutation-analysis@sha256:6f33327237fb5b5cb01b65b194244a742d791a0466844a6f5a9e18af2f99158a"
    );
    assert_eq!(compiled.outputs.len(), 2);
    assert_eq!(compiled.outputs[0].path, "mutation_analysis_report.tsv");
    assert_eq!(
        compiled.outputs[0].format.as_deref(),
        Some("mutation_analysis_report_tsv")
    );
    assert_eq!(compiled.outputs[1].path, "mutation_analysis_details.json");
    assert_eq!(
        compiled.outputs[1].format.as_deref(),
        Some("mutation_analysis_details_json")
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
    // "/artifacts/mutation_analysis_container"; the plugin follows the kind
    // rename (`_container` suffix dropped), the rule the ldsc and mrpresso
    // migrations applied.
    assert_eq!(
        compiled.artifact_prefix,
        Some("/artifacts/mutation_analysis".to_string())
    );
    assert_eq!(compiled.workdir, None);
    // The legacy wrapper bound no panels.
    assert!(compiled.panels.is_empty());
    assert!(compiled.panel_bundles.is_empty());
    // Legacy command was ["Rscript", "/opt/autonomics/mutation_analysis.R"];
    // the plugin stages its own runner at argv[1] (mrpresso/visualization
    // pattern), so the compiled command carries the interpreter only.
    assert_eq!(compiled.command, vec!["Rscript".to_string()]);

    // Resource profile byte-equal to the legacy container_spec: the
    // DEFAULT_CPUS/DEFAULT_MEMORY/DEFAULT_PIDS_LIMIT constants plus the
    // hardcoded shm_size.
    assert_eq!(compiled.cpus, Some(2.0));
    assert_eq!(compiled.memory.as_deref(), Some("8Gi"));
    assert_eq!(compiled.pids_limit, Some(512));
    assert_eq!(compiled.shm_size.as_deref(), Some("1Gi"));
    assert!(compiled.gpus.is_none());
    assert!(compiled.user.is_none());

    // The JSON MUTATION_CONFIG blob became per-param MUTATION_* env vars.
    // Defaults render through serde_json: f64 38.0 as "38.0" (equal after
    // R's as.numeric), the empty optional array as "".
    assert_eq!(compiled.env.get("MUTATION_OPERATION").unwrap(), "tmb");
    assert_eq!(compiled.env.get("MUTATION_PANEL_SIZE_MB").unwrap(), "38.0");
    assert_eq!(compiled.env.get("MUTATION_TMB_GROUP_COL").unwrap(), "group");
    assert_eq!(compiled.env.get("MUTATION_TMB_GROUPS").unwrap(), "");
    assert_eq!(compiled.env.get("MUTATION_TOP_N").unwrap(), "20");
    assert_eq!(
        compiled.env.get("MUTATION_GENE_COL").unwrap(),
        "Hugo_Symbol"
    );
    assert_eq!(
        compiled.env.get("MUTATION_VARIANT_COL").unwrap(),
        "Variant_Classification"
    );
    assert_eq!(
        compiled.env.get("MUTATION_TUMOR_SAMPLE_COL").unwrap(),
        "Tumor_Sample_Barcode"
    );
    assert_eq!(
        compiled.env.get("MUTATION_VARIANT_TYPE_COL").unwrap(),
        "Variant_Type"
    );
    assert_eq!(
        compiled.env.get("MUTATION_CHROMOSOME_COL").unwrap(),
        "Chromosome"
    );
    assert_eq!(
        compiled.env.get("MUTATION_START_POSITION_COL").unwrap(),
        "Start_Position"
    );
    assert_eq!(
        compiled.env.get("MUTATION_END_POSITION_COL").unwrap(),
        "End_Position"
    );
    assert_eq!(
        compiled.env.get("MUTATION_REFERENCE_ALLELE_COL").unwrap(),
        "Reference_Allele"
    );
    assert_eq!(
        compiled.env.get("MUTATION_TUMOR_SEQ_ALLELE_COL").unwrap(),
        "Tumor_Seq_Allele2"
    );
    // Provenance labels shape the schema only; the legacy wrapper never
    // sent them to the container and the manifest does not either.
    assert!(compiled.env.keys().all(|key| !key.contains("MAF_PATH")));
    assert!(compiled.env.keys().all(|key| !key.contains("CLINICAL")));

    // Script parity is semantic, not byte-exact: the plugin reassembles the
    // legacy config list from env, but the maftools pipeline below must
    // match the baked runner token for token.
    let script = compiled.script.as_deref().unwrap();
    assert!(script.contains("maftools::read.maf("));
    assert!(script.contains("read_table(env_value(\"AUTONOMICS_INPUT0\"))"));
    // The clinical channel stays optional at the script layer: the base
    // kind has no port 1, and the group comparison fails with the legacy
    // message exactly as it did when the port was unconnected.
    assert!(script.contains("Sys.getenv(\"AUTONOMICS_INPUT1\", unset = \"\")"));
    assert!(script.contains("tmb groups require a clinical input"));
    // String arrays: the env channel space-joins; a single-space fixed
    // strsplit is the exact inverse of that join.
    assert!(script.contains("strsplit(Sys.getenv(\"MUTATION_TMB_GROUPS\"), \" \", fixed = TRUE)"));
    // Legacy validate() checks the DSL cannot express live as guards with
    // the legacy error messages.
    assert!(script.contains("MAF column mappings must be unique"));
    assert!(script.contains("tmb_groups must contain two distinct nonempty labels"));
    assert!(script.contains("tmb_groups are only valid for operation `tmb`"));
    // Same five-way dispatch as the baked runner.
    assert!(script.contains("tmb = tmb_result(variants, config)"));
    assert!(script.contains("summary = summary_result(variants)"));
    assert!(script.contains("top_genes = top_genes_result(variants, config)"));
    assert!(script.contains("titv = titv_result(variants)"));
    assert!(script.contains("mutex = mutex_result(variants, config)$table"));
    assert!(script.contains("wilcox.test(first, second"));
    // Epilogue: the same report/JSON artifact writes.
    assert!(script.contains("write.table(report, env_value(\"AUTONOMICS_OUTPUT0\")"));
    assert!(script.contains("jsonlite::write_json(details, env_value(\"AUTONOMICS_OUTPUT1\")"));
}

#[test]
fn mutation_analysis_plugin_renders_submitted_values_into_env() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "mutation_analysis");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({
            "maf_path": "/data/cohort.maf",
            "operation": "top_genes",
            "panel_size_mb": 41.5,
            "top_n": 5,
            "tmb_groups": ["primary", "control"],
            "gene_col": "Gene"
        }),
    )
    .unwrap();

    assert_eq!(compiled.env.get("MUTATION_OPERATION").unwrap(), "top_genes");
    // serde_json renders f64 41.5 as "41.5".
    assert_eq!(compiled.env.get("MUTATION_PANEL_SIZE_MB").unwrap(), "41.5");
    assert_eq!(compiled.env.get("MUTATION_TOP_N").unwrap(), "5");
    // Arrays render space-joined on the env channel; the script re-splits
    // on a single space, preserving the legacy JSON array semantics.
    assert_eq!(
        compiled.env.get("MUTATION_TMB_GROUPS").unwrap(),
        "primary control"
    );
    assert_eq!(compiled.env.get("MUTATION_GENE_COL").unwrap(), "Gene");
}

#[test]
fn mutation_analysis_clinical_kind_differs_only_in_ports() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    // The v0 port DSL cannot express the legacy optional clinical input, so
    // the family splits by port layout: the clinical kind is the legacy
    // with_clinical=true shape, the base kind the with_clinical=false one.
    let base = node_by_kind(&manifest, "mutation_analysis");
    let clinical = node_by_kind(&manifest, "mutation_analysis_clinical");

    let base_ports = container_plugin::node_definition::compile_ports(&base.ports);
    let clinical_ports = container_plugin::node_definition::compile_ports(&clinical.ports);
    assert_eq!(base_ports.input_ports().len(), 1);
    assert_eq!(clinical_ports.input_ports().len(), 2);
    assert_eq!(base_ports.output_ports().len(), 2);
    assert_eq!(clinical_ports.output_ports().len(), 2);

    let base_compiled =
        compile_container_spec(base, &manifest.image, &manifest.panels, &base_params()).unwrap();
    let clinical_compiled =
        compile_container_spec(clinical, &manifest.image, &manifest.panels, &base_params())
            .unwrap();

    // Same image, outputs, resources, env, and script; only the port
    // layout and the derived artifact prefix differ. (ContainerCommandSpec
    // derives no PartialEq, so outputs compare field by field.)
    assert_eq!(base_compiled.image, clinical_compiled.image);
    assert_eq!(base_compiled.outputs.len(), clinical_compiled.outputs.len());
    for (base_output, clinical_output) in base_compiled
        .outputs
        .iter()
        .zip(clinical_compiled.outputs.iter())
    {
        assert_eq!(base_output.path, clinical_output.path);
        assert_eq!(base_output.format, clinical_output.format);
    }
    assert_eq!(base_compiled.env, clinical_compiled.env);
    assert_eq!(base_compiled.script, clinical_compiled.script);
    assert_eq!(base_compiled.cpus, clinical_compiled.cpus);
    assert_eq!(base_compiled.memory, clinical_compiled.memory);
    assert_eq!(base_compiled.pids_limit, clinical_compiled.pids_limit);
    assert_eq!(base_compiled.shm_size, clinical_compiled.shm_size);
    assert_eq!(
        clinical_compiled.artifact_prefix.as_deref(),
        Some("/artifacts/mutation_analysis_clinical")
    );
}

#[test]
fn mutation_analysis_plugin_schema_marks_provenance_labels_per_spec() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "mutation_analysis");

    let schema =
        serde_json::to_value(container_plugin::compile::compile_schema(&node.params)).unwrap();
    // Only the required legacy Spec field is required; clinical_path is the
    // Option -> optional mapping.
    let required = schema["required"].as_array().unwrap();
    assert_eq!(required, &vec![json!("maf_path")]);
    assert_eq!(schema["properties"]["maf_path"]["type"], "string");
    assert_eq!(schema["properties"]["clinical_path"]["type"], "string");
    assert!(
        schema["properties"]["clinical_path"]
            .get("default")
            .is_none()
    );
    // operation: the legacy enum as a string with the legacy default; the
    // switch in the script rejects unsupported values at run time.
    assert_eq!(schema["properties"]["operation"]["type"], "string");
    assert_eq!(schema["properties"]["operation"]["default"], "tmb");
    // Bounds per the legacy validate() code.
    let panel = &schema["properties"]["panel_size_mb"];
    assert_eq!(panel["type"], "number");
    assert_eq!(panel["default"], 38.0);
    assert_eq!(panel["exclusiveMinimum"], 0.0);
    let top_n = &schema["properties"]["top_n"];
    assert_eq!(top_n["type"], "integer");
    assert_eq!(top_n["default"], 20);
    assert_eq!(top_n["minimum"], 1.0);
    let groups = &schema["properties"]["tmb_groups"];
    assert_eq!(groups["type"], "array");
    assert_eq!(groups["items"]["type"], "string");
    // Distinctness/emptiness of the two labels is the script's guard, not a
    // schema bound, so no minItems/maxItems here.
    assert!(groups.get("minItems").is_none());
}
