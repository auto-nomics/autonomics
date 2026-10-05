//! Golden parity tests for the deseq2 plugin: the `[[nodes]]` entry must
//! compile to the same `Container` spec the legacy Rust wrapper
//! (`nodes-io/src/deseq2_container.rs`) produced. The plugin directory lives
//! outside this repository (`/mnt/projects/node-plugins/deseq2` by default,
//! overridable via `NODE_PLUGINS_ROOT`); the test is skipped when absent so
//! CI without the plugin checkout stays green.
//!
//! deseq2 is the first baked-runner migration: the pinned image carries
//! `/opt/autonomics/deseq2_runner.R`, so the compiled command is
//! interpreter + fixed argv with `script: None`, and parity hangs on the
//! six `AUTONOMICS_DESEQ2_*` env names rendering byte-identically to the
//! legacy `BTreeMap::from([...])` channel.

use std::path::PathBuf;

use container_plugin::compile::spec_compile::compile_container_spec;
use container_plugin::manifest::PluginManifest;
use serde_json::json;

fn plugin_root() -> Option<PathBuf> {
    let root = std::env::var_os("NODE_PLUGINS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/projects/node-plugins"));
    let manifest = root.join("deseq2").join("manifest.toml");
    manifest.is_file().then_some(root)
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let text = std::fs::read_to_string(root.join("deseq2").join("manifest.toml")).unwrap();
    // Inline script_file the way the loader does.
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        if let Some(relative) = node.command.script_file.clone() {
            let source = std::fs::read_to_string(root.join("deseq2").join(&relative)).unwrap();
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

/// The two condition levels have no serde default in the legacy
/// `Deseq2DeContainerSpec` (plain required String fields), so every compile
/// must submit them.
fn required_params() -> serde_json::Value {
    json!({
        "condition_reference": "untreated",
        "condition_test": "treated",
    })
}

#[test]
fn deseq2_de_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "deseq2_de");

    let compiled =
        compile_container_spec(node, &manifest.image, &manifest.panels, &required_params())
            .unwrap();

    assert_eq!(
        compiled.image,
        "ghcr.io/auto-nomics/autonomics/deseq2@sha256:9bd9a2e95a59d7a1725351b99fe188a71202a68a7255830213fe39a226e7864f"
    );
    assert_eq!(compiled.outputs.len(), 5);
    assert_eq!(compiled.outputs[0].path, "results.tsv");
    assert_eq!(
        compiled.outputs[0].format.as_deref(),
        Some("deseq2_results_tsv")
    );
    assert_eq!(compiled.outputs[1].path, "normalized_counts.tsv");
    assert_eq!(
        compiled.outputs[1].format.as_deref(),
        Some("deseq2_normalized_counts_tsv")
    );
    assert_eq!(compiled.outputs[2].path, "size_factors.tsv");
    assert_eq!(
        compiled.outputs[2].format.as_deref(),
        Some("deseq2_size_factors_tsv")
    );
    assert_eq!(compiled.outputs[3].path, "deseq2_dataset.rds");
    assert_eq!(compiled.outputs[3].format.as_deref(), Some("r_rds"));
    assert_eq!(compiled.outputs[4].path, "run_report.json");
    assert_eq!(
        compiled.outputs[4].format.as_deref(),
        Some("deseq2_run_report_json")
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
    // Deliberate delta: the legacy default was "/artifacts/deseq2_de_container";
    // the plugin drops the `_container` suffix together with the kind rename
    // (the same rule the ldsc/mrpresso/mvmr migrations applied).
    assert_eq!(compiled.artifact_prefix, "/artifacts/deseq2_de");
    assert_eq!(compiled.workdir, None);
    // The legacy wrapper bound no panels and no panel bundles.
    assert!(compiled.panels.is_empty());
    assert!(compiled.panel_bundles.is_empty());

    // Baked runner: byte-exact command (interpreter + argv), no inline
    // script, no inline files — exactly the legacy `command`/`script: None`.
    assert_eq!(
        compiled.command,
        vec![
            "Rscript".to_string(),
            "--vanilla".to_string(),
            "/opt/autonomics/deseq2_runner.R".to_string()
        ]
    );
    assert!(compiled.script.is_none());
    assert!(compiled.files.is_empty());

    // The env channel is the whole param contract for a baked runner, so the
    // AUTONOMICS_DESEQ2_* names are asserted as an exact key set (BTreeMap
    // order) plus default values byte-equal to the legacy `container_spec`
    // output: empty covariates join, enum label, serde_json number spellings.
    // Deliberate delta: LFC_SHRINK postdates the legacy wrapper — the
    // pre-apeglm image silently delivered raw MLE coefficients while
    // downstream consumers assumed DESeq2-recommended apeglm shrinkage.
    let env_names: Vec<&str> = compiled.env.keys().map(String::as_str).collect();
    assert_eq!(
        env_names,
        vec![
            "AUTONOMICS_DESEQ2_ALPHA",
            "AUTONOMICS_DESEQ2_CONDITION_REFERENCE",
            "AUTONOMICS_DESEQ2_CONDITION_TEST",
            "AUTONOMICS_DESEQ2_COVARIATES",
            "AUTONOMICS_DESEQ2_FIT_TYPE",
            "AUTONOMICS_DESEQ2_LFC_SHRINK",
            "AUTONOMICS_DESEQ2_THREADS",
        ]
    );
    assert_eq!(compiled.env.get("AUTONOMICS_DESEQ2_ALPHA").unwrap(), "0.1");
    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_DESEQ2_CONDITION_REFERENCE")
            .unwrap(),
        "untreated"
    );
    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_DESEQ2_CONDITION_TEST")
            .unwrap(),
        "treated"
    );
    // Legacy `spec.covariates.join(",")` of the empty default: "".
    assert_eq!(
        compiled.env.get("AUTONOMICS_DESEQ2_COVARIATES").unwrap(),
        ""
    );
    assert_eq!(
        compiled.env.get("AUTONOMICS_DESEQ2_FIT_TYPE").unwrap(),
        "parametric"
    );
    assert_eq!(
        compiled.env.get("AUTONOMICS_DESEQ2_LFC_SHRINK").unwrap(),
        "apeglm"
    );
    assert_eq!(compiled.env.get("AUTONOMICS_DESEQ2_THREADS").unwrap(), "1");

    // Resources are byte-exact against the legacy DEFAULT_CPUS / DEFAULT_MEMORY
    // / DEFAULT_PIDS_LIMIT / DEFAULT_SHM_SIZE constants; network, read-only
    // rootfs, and pull policy ride the hardened manifest defaults.
    assert_eq!(compiled.cpus, Some(2.0));
    assert_eq!(compiled.memory.as_deref(), Some("4Gi"));
    assert_eq!(compiled.pids_limit, Some(256));
    assert_eq!(compiled.shm_size.as_deref(), Some("1Gi"));
    assert!(compiled.gpus.is_none());
    assert!(compiled.user.is_none());
}

#[test]
fn deseq2_de_plugin_renders_submitted_values_into_env() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "deseq2_de");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({
            "condition_reference": "control",
            "condition_test": "knockout",
            "covariates": "type,batch",
            "fit_type": "local",
            "lfc_shrink": "none",
            "alpha": 0.05,
            "threads": 1,
        }),
    )
    .unwrap();

    // Covariates join parity: the legacy Rust wrapper did
    // `spec.covariates.join(",")`; the manifest param is a comma-separated
    // string, so the rendered env bytes are identical ("type,batch") and the
    // baked runner's `strsplit(value, ",", fixed = TRUE)` sees exactly what
    // it always saw. Space-joined string_array params would have broken the
    // pinned runner — this is the documented serialized-string delta.
    assert_eq!(
        compiled.env.get("AUTONOMICS_DESEQ2_COVARIATES").unwrap(),
        "type,batch"
    );
    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_DESEQ2_CONDITION_REFERENCE")
            .unwrap(),
        "control"
    );
    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_DESEQ2_CONDITION_TEST")
            .unwrap(),
        "knockout"
    );
    assert_eq!(
        compiled.env.get("AUTONOMICS_DESEQ2_FIT_TYPE").unwrap(),
        "local"
    );
    assert_eq!(
        compiled.env.get("AUTONOMICS_DESEQ2_LFC_SHRINK").unwrap(),
        "none"
    );
    assert_eq!(compiled.env.get("AUTONOMICS_DESEQ2_ALPHA").unwrap(), "0.05");
    assert_eq!(compiled.env.get("AUTONOMICS_DESEQ2_THREADS").unwrap(), "1");
}

#[test]
fn deseq2_de_plugin_schema_matches_the_legacy_param_surface() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "deseq2_de");

    let schema =
        serde_json::to_value(container_plugin::compile::compile_schema(&node.params)).unwrap();
    // BTreeMap order: exactly the two legacy non-defaulted fields.
    let required = schema["required"].as_array().unwrap();
    assert_eq!(
        required,
        &vec![json!("condition_reference"), json!("condition_test")]
    );
    // covariates changes shape: the legacy schema advertised a string array;
    // the DSL's env renderer space-joins arrays while the baked runner splits
    // on commas, so it is a comma-separated string for now (see README).
    assert_eq!(schema["properties"]["covariates"]["type"], "string");
    assert_eq!(schema["properties"]["covariates"]["default"], "");
    // fit_type loses its enum: v0 has no enum param type; the runner rejects
    // values outside parametric/local/mean.
    assert_eq!(schema["properties"]["fit_type"]["type"], "string");
    assert_eq!(schema["properties"]["fit_type"]["default"], "parametric");
    // lfc_shrink postdates the legacy wrapper: apeglm posterior estimates
    // by default, none for raw MLE coefficients.
    assert_eq!(schema["properties"]["lfc_shrink"]["type"], "string");
    assert_eq!(schema["properties"]["lfc_shrink"]["default"], "apeglm");
    // alpha bounds per the legacy validate() code: strict (0, 1).
    let alpha = &schema["properties"]["alpha"];
    assert_eq!(alpha["type"], "number");
    assert_eq!(alpha["default"], 0.1);
    assert_eq!(alpha["exclusiveMinimum"], 0.0);
    assert_eq!(alpha["exclusiveMaximum"], 1.0);
    // threads pinned to exactly 1 encodes "threads must be 1 in the
    // deterministic first contract".
    let threads = &schema["properties"]["threads"];
    assert_eq!(threads["type"], "integer");
    assert_eq!(threads["default"], 1);
    assert_eq!(threads["minimum"], 1.0);
    assert_eq!(threads["maximum"], 1.0);
}

#[test]
fn deseq2_de_plugin_enforces_the_legacy_validate_bounds() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "deseq2_de");

    // The legacy spec demanded the condition levels; the plugin names the
    // first missing param instead of rendering an empty env.
    let error =
        compile_container_spec(node, &manifest.image, &manifest.panels, &json!({})).unwrap_err();
    assert!(error.to_string().contains("condition_reference"), "{error}");

    // Legacy "alpha must be finite and lie strictly between 0 and 1":
    // exclusive bounds reject both endpoints at compile time.
    let error = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({"condition_reference": "untreated", "condition_test": "treated", "alpha": 1.0}),
    )
    .unwrap_err();
    assert!(error.to_string().contains("alpha"), "{error}");
    assert!(error.to_string().contains("exclusiveMaximum"), "{error}");

    // Legacy "threads must be 1 in the deterministic first contract":
    // min = max = 1 rejects anything else.
    let error = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({"condition_reference": "untreated", "condition_test": "treated", "threads": 2}),
    )
    .unwrap_err();
    assert!(error.to_string().contains("threads"), "{error}");
    assert!(error.to_string().contains("maximum"), "{error}");
}
