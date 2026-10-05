//! Golden parity tests for the music plugin: the `[[nodes]]` entry must
//! compile to the same `Container` spec the legacy Rust wrapper
//! (`nodes-io/src/music_deconvolution_container.rs`) produced. The plugin
//! directory lives outside this repository
//! (`/mnt/projects/node-plugins/music` by default, overridable via
//! `NODE_PLUGINS_ROOT`); the test is skipped when absent so CI without
//! the plugin checkout stays green.
//!
//! music is a baked-runner migration (the deseq2 pattern): the pinned image
//! carries `/opt/autonomics/music_runner.R`, so the compiled command is
//! interpreter + fixed argv with `script: None`, and parity hangs on the
//! nine `AUTONOMICS_MUSIC_*` env names rendering byte-identically to the
//! legacy `BTreeMap::from([...])` channel.

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
    let manifest = root.join("music").join("manifest.toml");
    if manifest.is_file() {
        return Some(root);
    }
    if explicit.is_some() {
        // fix-review R04 (2026-10-05): an explicitly set NODE_PLUGINS_ROOT
        // means migration parity was requested. A missing family must fail
        // loudly — early-returns here used to count as *passed* tests, so a
        // green summary claimed coverage that never ran.
        panic!(
            "NODE_PLUGINS_ROOT is set but the music family is not deployed \
             under it ({}) — migration parity cannot run. Deploy the family \
             or unset NODE_PLUGINS_ROOT to skip these tests deliberately.",
            manifest.display()
        );
    }
    eprintln!("skipping: music plugin directory not present (NODE_PLUGINS_ROOT unset)");
    None
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let text = std::fs::read_to_string(root.join("music").join("manifest.toml")).unwrap();
    // Inline script_file the way the loader does.
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        if let Some(relative) = node.command.script_file.clone() {
            let source = std::fs::read_to_string(root.join("music").join(&relative)).unwrap();
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
fn music_deconvolution_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "music_deconvolution");

    let compiled =
        compile_container_spec(node, &manifest.image, &manifest.panels, &json!({})).unwrap();

    assert_eq!(
        compiled.image,
        "ghcr.io/auto-nomics/autonomics/music-deconvolution@sha256:886b83135179a48fffe2009238eb7d45ec89d32713cc1a2370078583e3bbfeaf"
    );
    assert_eq!(compiled.outputs.len(), 5);
    assert_eq!(compiled.outputs[0].path, "cell_type_proportions.tsv");
    assert_eq!(
        compiled.outputs[0].format.as_deref(),
        Some("music_proportions_tsv")
    );
    assert_eq!(compiled.outputs[1].path, "nnls_proportions.tsv");
    assert_eq!(
        compiled.outputs[1].format.as_deref(),
        Some("music_nnls_proportions_tsv")
    );
    assert_eq!(compiled.outputs[2].path, "gene_weights.tsv");
    assert_eq!(
        compiled.outputs[2].format.as_deref(),
        Some("music_gene_weights_tsv")
    );
    assert_eq!(compiled.outputs[3].path, "diagnostics.tsv");
    assert_eq!(
        compiled.outputs[3].format.as_deref(),
        Some("music_diagnostics_tsv")
    );
    assert_eq!(compiled.outputs[4].path, "run_report.json");
    assert_eq!(
        compiled.outputs[4].format.as_deref(),
        Some("music_run_report_json")
    );
    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert!(matches!(
        compiled.pull_policy,
        container_runtime::PullPolicy::Missing
    ));
    // Legacy DEFAULT_TIMEOUT_SECS. The legacy spec accepted a per-instance
    // override; the plugin DSL pins it per kind.
    assert_eq!(compiled.timeout_secs, 7200);
    // Deliberate delta: the legacy default was
    // "/artifacts/music_deconvolution_container"; the plugin drops the
    // `_container` suffix together with the kind rename (the same rule the
    // ldsc/mrpresso/deseq2 migrations applied).
    assert_eq!(compiled.artifact_prefix, "/artifacts/music_deconvolution");
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
            "/opt/autonomics/music_runner.R".to_string()
        ]
    );
    assert!(compiled.script.is_none());
    assert!(compiled.files.is_empty());

    // The env channel is the whole param contract for a baked runner, so the
    // nine legacy AUTONOMICS_MUSIC_* names are asserted as an exact key set
    // (BTreeMap order) plus default values byte-equal to the legacy
    // `container_spec` output: empty cell-type join, serde_json number
    // spellings, Rust `bool::to_string` booleans.
    let env_names: Vec<&str> = compiled.env.keys().map(String::as_str).collect();
    assert_eq!(
        env_names,
        vec![
            "AUTONOMICS_MUSIC_CELL_TYPE_COL",
            "AUTONOMICS_MUSIC_CENTERED",
            "AUTONOMICS_MUSIC_CT_COV",
            "AUTONOMICS_MUSIC_EPSILON",
            "AUTONOMICS_MUSIC_ITER_MAX",
            "AUTONOMICS_MUSIC_NORMALIZE",
            "AUTONOMICS_MUSIC_NU",
            "AUTONOMICS_MUSIC_SELECT_CELL_TYPES",
            "AUTONOMICS_MUSIC_SUBJECT_COL",
        ]
    );
    assert_eq!(
        compiled.env.get("AUTONOMICS_MUSIC_CELL_TYPE_COL").unwrap(),
        "cell_type"
    );
    assert_eq!(
        compiled.env.get("AUTONOMICS_MUSIC_CENTERED").unwrap(),
        "false"
    );
    assert_eq!(
        compiled.env.get("AUTONOMICS_MUSIC_CT_COV").unwrap(),
        "false"
    );
    assert_eq!(
        compiled.env.get("AUTONOMICS_MUSIC_EPSILON").unwrap(),
        "0.01"
    );
    assert_eq!(
        compiled.env.get("AUTONOMICS_MUSIC_ITER_MAX").unwrap(),
        "1000"
    );
    assert_eq!(
        compiled.env.get("AUTONOMICS_MUSIC_NORMALIZE").unwrap(),
        "false"
    );
    assert_eq!(compiled.env.get("AUTONOMICS_MUSIC_NU").unwrap(), "0.0001");
    // Legacy `spec.select_cell_types.join(",")` of the empty default: "".
    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_MUSIC_SELECT_CELL_TYPES")
            .unwrap(),
        ""
    );
    assert_eq!(
        compiled.env.get("AUTONOMICS_MUSIC_SUBJECT_COL").unwrap(),
        "subject_id"
    );

    // Resources are byte-exact against the legacy DEFAULT_CPUS / DEFAULT_MEMORY
    // / DEFAULT_PIDS_LIMIT / DEFAULT_SHM_SIZE constants; network, read-only
    // rootfs, and pull policy ride the hardened manifest defaults.
    assert_eq!(compiled.cpus, Some(4.0));
    assert_eq!(compiled.memory.as_deref(), Some("12Gi"));
    assert_eq!(compiled.pids_limit, Some(256));
    assert_eq!(compiled.shm_size.as_deref(), Some("1Gi"));
    assert!(compiled.gpus.is_none());
    assert!(compiled.user.is_none());

    // Port delta (see README): the legacy layout also declared two OPTIONAL
    // unlabeled inputs (cell sizes, markers). The v0 PortLayout cannot
    // express optional inputs — every declared input compiles required —
    // so the plugin declares exactly the three required labeled inputs.
    let ports = container_plugin::node_definition::compile_ports(&node.ports);
    assert_eq!(ports.input_ports().len(), 3);
    assert_eq!(ports.output_ports().len(), 5);
}

#[test]
fn music_deconvolution_plugin_renders_submitted_values_into_env() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "music_deconvolution");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({
            "cell_type_col": "ClusterType",
            "subject_col": "Donor",
            "select_cell_types": "neuron,astrocyte",
            "iter_max": 500,
            "nu": 0.0005,
            "epsilon": 0.005,
            "centered": true,
            "normalize": false,
            "ct_cov": true,
        }),
    )
    .unwrap();

    // Cell-type join parity: the legacy Rust wrapper did
    // `spec.select_cell_types.join(",")`; the manifest param is a
    // comma-separated string, so the rendered env bytes are identical
    // ("neuron,astrocyte") and the baked runner's split_csv sees exactly
    // what it always saw. Space-joined string_array params would have
    // broken the pinned runner — this is the documented serialized-string
    // delta (deseq2 covariates pattern).
    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_MUSIC_SELECT_CELL_TYPES")
            .unwrap(),
        "neuron,astrocyte"
    );
    assert_eq!(
        compiled.env.get("AUTONOMICS_MUSIC_CELL_TYPE_COL").unwrap(),
        "ClusterType"
    );
    assert_eq!(
        compiled.env.get("AUTONOMICS_MUSIC_SUBJECT_COL").unwrap(),
        "Donor"
    );
    assert_eq!(
        compiled.env.get("AUTONOMICS_MUSIC_ITER_MAX").unwrap(),
        "500"
    );
    // serde_json renders f64 0.0005 as "0.0005", identical to the legacy
    // `spec.nu.to_string()` for the same value.
    assert_eq!(compiled.env.get("AUTONOMICS_MUSIC_NU").unwrap(), "0.0005");
    assert_eq!(
        compiled.env.get("AUTONOMICS_MUSIC_EPSILON").unwrap(),
        "0.005"
    );
    assert_eq!(
        compiled.env.get("AUTONOMICS_MUSIC_CENTERED").unwrap(),
        "true"
    );
    assert_eq!(
        compiled.env.get("AUTONOMICS_MUSIC_NORMALIZE").unwrap(),
        "false"
    );
    assert_eq!(compiled.env.get("AUTONOMICS_MUSIC_CT_COV").unwrap(), "true");
}

#[test]
fn music_deconvolution_plugin_schema_matches_the_legacy_param_surface() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "music_deconvolution");

    let schema =
        serde_json::to_value(container_plugin::compile::compile_schema(&node.params)).unwrap();
    // Every legacy spec field carried a serde default, so nothing is
    // required.
    let required = schema["required"].as_array().unwrap();
    assert!(required.is_empty());
    // select_cell_types changes shape: the legacy schema advertised a string
    // array; the DSL's env renderer space-joins arrays while the baked runner
    // splits on commas, so it is a comma-separated string for now (see
    // README).
    assert_eq!(schema["properties"]["select_cell_types"]["type"], "string");
    assert_eq!(schema["properties"]["select_cell_types"]["default"], "");
    // nu / epsilon bounds per the legacy validate() code: finite and > 0.
    let nu = &schema["properties"]["nu"];
    assert_eq!(nu["type"], "number");
    assert_eq!(nu["default"], 0.0001);
    assert_eq!(nu["exclusiveMinimum"], 0.0);
    let epsilon = &schema["properties"]["epsilon"];
    assert_eq!(epsilon["type"], "number");
    assert_eq!(epsilon["default"], 0.01);
    assert_eq!(epsilon["exclusiveMinimum"], 0.0);
    // iter_max: legacy u32 with "must be greater than zero".
    let iter_max = &schema["properties"]["iter_max"];
    assert_eq!(iter_max["type"], "integer");
    assert_eq!(iter_max["default"], 1000);
    assert_eq!(iter_max["minimum"], 1.0);
    assert_eq!(schema["properties"]["centered"]["type"], "boolean");
    assert_eq!(schema["properties"]["centered"]["default"], false);
    assert_eq!(schema["properties"]["normalize"]["type"], "boolean");
    assert_eq!(schema["properties"]["normalize"]["default"], false);
    assert_eq!(schema["properties"]["ct_cov"]["type"], "boolean");
    assert_eq!(schema["properties"]["ct_cov"]["default"], false);
}

#[test]
fn music_deconvolution_plugin_enforces_the_legacy_validate_bounds() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "music_deconvolution");

    // Legacy "iter_max must be greater than zero": min = 1 rejects 0 at
    // compile time.
    let error = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({"iter_max": 0}),
    )
    .unwrap_err();
    assert!(error.to_string().contains("iter_max"), "{error}");
    assert!(error.to_string().contains("minimum"), "{error}");

    // Legacy "nu must be finite and greater than zero": exclusive_min
    // rejects 0 at compile time.
    let error =
        compile_container_spec(node, &manifest.image, &manifest.panels, &json!({"nu": 0.0}))
            .unwrap_err();
    assert!(error.to_string().contains("nu"), "{error}");
    assert!(error.to_string().contains("exclusiveMinimum"), "{error}");

    // Legacy "epsilon must be finite and greater than zero".
    let error = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({"epsilon": -0.5}),
    )
    .unwrap_err();
    assert!(error.to_string().contains("epsilon"), "{error}");
    assert!(error.to_string().contains("exclusiveMinimum"), "{error}");
}
