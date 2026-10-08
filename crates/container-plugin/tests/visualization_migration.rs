//! Golden parity tests for the visualization plugin: the `[[nodes]]` entry
//! must compile to the same `Container` spec the legacy Rust wrapper
//! (`crates/node-bundles/nodes-io/src/visualization_container.rs`) produced.
//! The plugin directory lives outside this repository
//! (`/mnt/projects/node-plugins/visualization` by default, overridable via
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
    let manifest = root.join("visualization").join("manifest.toml");
    if manifest.is_file() {
        return Some(root);
    }
    if explicit.is_some() {
        // fix-review R04 (2026-10-05): an explicitly set NODE_PLUGINS_ROOT
        // means migration parity was requested. A missing family must fail
        // loudly — early-returns here used to count as *passed* tests, so a
        // green summary claimed coverage that never ran.
        panic!(
            "NODE_PLUGINS_ROOT is set but the visualization family is not deployed \
             under it ({}) — migration parity cannot run. Deploy the family \
             or unset NODE_PLUGINS_ROOT to skip these tests deliberately.",
            manifest.display()
        );
    }
    eprintln!("skipping: visualization plugin directory not present (NODE_PLUGINS_ROOT unset)");
    None
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let text = std::fs::read_to_string(root.join("visualization").join("manifest.toml")).unwrap();
    // Inline script_file the way the loader does.
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        if let Some(relative) = node.command.script_file.clone() {
            let source =
                std::fs::read_to_string(root.join("visualization").join(&relative)).unwrap();
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
fn visualization_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "visualization");

    // `data_format` is required (the legacy Spec had no serde default for
    // it either); the legacy wrapper test's baseline spec used Csv.
    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({"data_format": "csv"}),
    )
    .unwrap();

    assert_eq!(
        compiled.image,
        "ghcr.io/auto-nomics/autonomics/visualization@sha256:ee9592b77bc5ea0cebfafafbe39550c377204019f451d7a37e13e4ce2e884f15"
    );
    assert_eq!(compiled.outputs.len(), 1);
    assert_eq!(compiled.outputs[0].path, "plot.png");
    assert_eq!(compiled.outputs[0].format.as_deref(), Some("png"));
    // The legacy wrapper built `["Rscript", "/opt/autonomics/render.R"]` with
    // no inline script; the plugin owns the runner and stages it, so the
    // compiled command is the bare interpreter and ContainerCommandNode
    // inserts `/work/.autonomics/script` at argv[1] at run time. Same
    // interpreter, same per-run behaviour.
    assert_eq!(compiled.command, vec!["Rscript".to_string()]);
    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert!(matches!(
        compiled.pull_policy,
        container_runtime::PullPolicy::Missing
    ));
    assert_eq!(compiled.timeout_secs, 300);
    // The legacy DEFAULT_ARTIFACT_PREFIX is kept verbatim; the manifest-level
    // default would have derived `/artifacts/visualization` (kind minus the
    // `_container` suffix).
    assert_eq!(
        compiled.artifact_prefix,
        Some("/artifacts/visualization_container".to_string())
    );
    assert_eq!(compiled.workdir, None);
    assert!(compiled.panels.is_empty());
    assert!(compiled.panel_bundles.is_empty());
    // The legacy wrapper hard-baked DEFAULT_CPUS/DEFAULT_MEMORY/
    // DEFAULT_PIDS_LIMIT into the compiled spec; the manifest pins the same
    // values in [nodes.resources].
    assert_eq!(compiled.cpus, Some(2.0));
    assert_eq!(compiled.memory.as_deref(), Some("2Gi"));
    assert_eq!(compiled.pids_limit, Some(256));
    // The user R script does NOT travel through the files channel (that map
    // is manifest-static text); it stays a DAG input file on port 1, exactly
    // as the legacy wrapper staged it.
    assert!(compiled.files.is_empty());

    // Env defaults. serde_json renders the f64 defaults with a trailing `.0`
    // (`8.0`) where the legacy `f64::to_string` produced `"8"`; values are
    // equal after R's as.numeric() (documented pitfall 8).
    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_VISUALIZATION_DATA_FORMAT")
            .unwrap(),
        "csv"
    );
    assert_eq!(
        compiled.env.get("AUTONOMICS_VISUALIZATION_WIDTH").unwrap(),
        "8.0"
    );
    assert_eq!(
        compiled.env.get("AUTONOMICS_VISUALIZATION_HEIGHT").unwrap(),
        "6.0"
    );
    assert_eq!(
        compiled.env.get("AUTONOMICS_VISUALIZATION_DPI").unwrap(),
        "150.0"
    );
}

#[test]
fn visualization_plugin_runner_keeps_the_legacy_dispatch_semantics() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "visualization");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({"data_format": "csv"}),
    )
    .unwrap();

    // Semantic runner markers, not byte equality: the plugin runner is the
    // legacy image-baked render.R adapted, so the AUTONOMICS_* bindings and
    // the five-way data-format dispatch are the load-bearing tokens.
    let script = compiled.script.as_deref().unwrap();
    assert!(script.contains("Sys.getenv(\"AUTONOMICS_INPUT0\")"));
    assert!(script.contains("Sys.getenv(\"AUTONOMICS_INPUT1\")"));
    assert!(script.contains("Sys.getenv(\"AUTONOMICS_OUTPUT0\")"));
    assert!(script.contains("Sys.getenv(\"AUTONOMICS_VISUALIZATION_DATA_FORMAT\")"));
    assert!(script.contains("Sys.getenv(\"AUTONOMICS_VISUALIZATION_WIDTH\")"));
    assert!(script.contains("Sys.getenv(\"AUTONOMICS_VISUALIZATION_HEIGHT\")"));
    assert!(script.contains("Sys.getenv(\"AUTONOMICS_VISUALIZATION_DPI\")"));

    // Data-format dispatch kept identical to the legacy render.R switch.
    assert!(script.contains("csv = arrow::read_csv_arrow"));
    assert!(script.contains("tsv = arrow::read_tsv_arrow"));
    assert!(script.contains("parquet = arrow::read_parquet"));
    assert!(script.contains("arrow_stream = arrow::read_ipc_stream"));
    assert!(script.contains("arrow_file = arrow::read_feather"));
    assert!(script.contains("unsupported visualization data format"));

    // The user-script channel: the staged port-1 file is sourced with `df`
    // preloaded, `p` must exist, and `p` is saved as the PNG output.
    assert!(script.contains("script_env$df <- df"));
    assert!(script.contains("source(script_path, local = script_env)"));
    assert!(script.contains("must assign a plot to a variable named `p`"));
    assert!(script.contains("ggplot2::ggsave"));
}

#[test]
fn visualization_plugin_renders_submitted_values_into_env() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "visualization");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({"data_format": "parquet", "width": 10, "height": 4.5, "dpi": 300}),
    )
    .unwrap();

    assert_eq!(
        compiled
            .env
            .get("AUTONOMICS_VISUALIZATION_DATA_FORMAT")
            .unwrap(),
        "parquet"
    );
    // Integer 10 renders "10", matching the legacy f64 10.0.to_string();
    // 4.5 renders "4.5" on both paths. (Defaults render as "8.0"/"6.0" —
    // see the contract test's pitfall-8 note.)
    assert_eq!(
        compiled.env.get("AUTONOMICS_VISUALIZATION_WIDTH").unwrap(),
        "10"
    );
    assert_eq!(
        compiled.env.get("AUTONOMICS_VISUALIZATION_HEIGHT").unwrap(),
        "4.5"
    );
    assert_eq!(
        compiled.env.get("AUTONOMICS_VISUALIZATION_DPI").unwrap(),
        "300"
    );
}

#[test]
fn visualization_plugin_schema_requires_data_format_and_bounds_dimensions() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "visualization");

    let schema =
        serde_json::to_value(container_plugin::compile::compile_schema(&node.params)).unwrap();
    // data_format is the only required param (no default, not optional);
    // width/height/dpi carry defaults and the legacy "finite and > 0" rule.
    assert_eq!(
        schema["required"],
        json!(["data_format"]),
        "only data_format must be required"
    );
    assert_eq!(schema["properties"]["data_format"]["type"], "string");
    assert_eq!(schema["properties"]["width"]["type"], "number");
    assert_eq!(schema["properties"]["width"]["default"], 8.0);
    assert_eq!(schema["properties"]["width"]["exclusiveMinimum"], 0.0);
    assert_eq!(schema["properties"]["height"]["default"], 6.0);
    assert_eq!(schema["properties"]["height"]["exclusiveMinimum"], 0.0);
    assert_eq!(schema["properties"]["dpi"]["default"], 150.0);
    assert_eq!(schema["properties"]["dpi"]["exclusiveMinimum"], 0.0);
}

#[test]
fn visualization_plugin_keeps_the_two_input_port_layout() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "visualization");

    // Port 0 `data`, port 1 `r_script`, one output: the user R script keeps
    // flowing through the DAG input channel, so the plugin layout must match
    // the legacy port_layout() exactly.
    let ports = container_plugin::node_definition::compile_ports(&node.ports);
    assert_eq!(ports.input_ports().len(), 2);
    assert_eq!(ports.output_ports().len(), 1);
}
