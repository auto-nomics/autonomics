//! Golden parity tests for the timesfm plugin: the `[[nodes]]` entry must
//! compile to the same `Container` spec the legacy Rust wrapper
//! (`nodes-io/src/timesfm_container.rs`) produced. The plugin directory
//! lives outside this repository (`/mnt/projects/node-plugins/timesfm` by
//! default, overridable via `NODE_PLUGINS_ROOT`); the test is skipped when
//! absent so CI without the plugin checkout stays green.
//!
//! timesfm is a baked-runner migration like deseq2: the pinned image
//! carries the `timesfm_service` package, so the compiled command is
//! interpreter + fixed argv (`python -m timesfm_service.runner`) with
//! `script: None`, and parity hangs on the five `TIMESFM_*` env names
//! rendering byte-identically to the legacy `container_spec` output. The
//! checkpoint is baked into the image (`TIMESFM_LOCAL_FILES_ONLY=true`),
//! so the isolated-network default must never be relaxed.

use std::path::PathBuf;

use container_plugin::compile::spec_compile::compile_container_spec;
use container_plugin::manifest::PluginManifest;
use serde_json::json;

fn plugin_root() -> Option<PathBuf> {
    let root = std::env::var_os("NODE_PLUGINS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/projects/node-plugins"));
    let manifest = root.join("timesfm").join("manifest.toml");
    manifest.is_file().then_some(root)
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let text = std::fs::read_to_string(root.join("timesfm").join("manifest.toml")).unwrap();
    // Inline script_file the way the loader does.
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        if let Some(relative) = node.command.script_file.clone() {
            let source = std::fs::read_to_string(root.join("timesfm").join(&relative)).unwrap();
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
fn timesfm_forecast_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "timesfm_forecast");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({}),
    )
    .unwrap();

    assert_eq!(
        compiled.image,
        "ghcr.io/auto-nomics/autonomics/timesfm@sha256:9fa439cb6b84df08bf686e4e9e69993ec9223b2f713a6b205c2dce373ca33991"
    );
    assert_eq!(compiled.outputs.len(), 2);
    assert_eq!(compiled.outputs[0].path, "forecast_result.json");
    assert_eq!(
        compiled.outputs[0].format.as_deref(),
        Some("timesfm_forecast_json")
    );
    assert_eq!(compiled.outputs[1].path, "forecast.log");
    assert_eq!(compiled.outputs[1].format.as_deref(), Some("timesfm_log"));
    // The node runs offline: isolated network, read-only rootfs, pull on
    // first use — exactly the legacy `container_spec` profile. The baked
    // checkpoint means these must never relax toward egress.
    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert!(matches!(
        compiled.pull_policy,
        container_runtime::PullPolicy::Missing
    ));
    // Legacy DEFAULT_TIMEOUT_SECS. The legacy spec accepted a per-instance
    // override; the plugin DSL pins it per kind.
    assert_eq!(compiled.timeout_secs, 1800);
    // Deliberate delta: the legacy default was
    // "/artifacts/timesfm_forecast_container"; the plugin drops the
    // `_container` suffix together with the kind rename (the same rule the
    // mrpresso/deseq2 migrations applied).
    assert_eq!(compiled.artifact_prefix, "/artifacts/timesfm_forecast");
    assert_eq!(compiled.workdir, None);
    // The legacy wrapper bound no panels and no panel bundles.
    assert!(compiled.panels.is_empty());
    assert!(compiled.panel_bundles.is_empty());

    // Baked runner: byte-exact command (interpreter + argv), no inline
    // script, no inline files — exactly the legacy
    // `command: ["python", "-m", "timesfm_service.runner"]`/`script: None`.
    assert_eq!(
        compiled.command,
        vec![
            "python".to_string(),
            "-m".to_string(),
            "timesfm_service.runner".to_string()
        ]
    );
    assert!(compiled.script.is_none());
    assert!(compiled.files.is_empty());

    // The env channel is the whole param contract, so the five legacy
    // TIMESFM_* names are asserted as an exact key set (BTreeMap order)
    // with default values byte-equal to the legacy `container_spec` output:
    // horizon 12, max_context 1024, the static checkpoint trio verbatim.
    let env_names: Vec<&str> = compiled.env.keys().map(String::as_str).collect();
    assert_eq!(
        env_names,
        vec![
            "TIMESFM_CHECKPOINT",
            "TIMESFM_HORIZON",
            "TIMESFM_LOCAL_FILES_ONLY",
            "TIMESFM_MAX_CONTEXT",
            "TIMESFM_REVISION",
        ]
    );
    assert_eq!(
        compiled.env.get("TIMESFM_CHECKPOINT").unwrap(),
        "/opt/timesfm/checkpoint"
    );
    assert_eq!(compiled.env.get("TIMESFM_HORIZON").unwrap(), "12");
    assert_eq!(
        compiled.env.get("TIMESFM_LOCAL_FILES_ONLY").unwrap(),
        "true"
    );
    assert_eq!(compiled.env.get("TIMESFM_MAX_CONTEXT").unwrap(), "1024");
    assert_eq!(
        compiled.env.get("TIMESFM_REVISION").unwrap(),
        "1d952420fba87f3c6dee4f240de0f1a0fbc790e3"
    );

    // Resources are byte-exact against the legacy container_spec constants;
    // network, read-only rootfs, and pull policy ride the hardened manifest
    // defaults.
    assert_eq!(compiled.cpus, Some(4.0));
    assert_eq!(compiled.memory.as_deref(), Some("8Gi"));
    assert_eq!(compiled.pids_limit, Some(512));
    assert_eq!(compiled.shm_size.as_deref(), Some("1Gi"));
    assert!(compiled.gpus.is_none());
    assert!(compiled.user.is_none());
}

#[test]
fn timesfm_forecast_plugin_renders_submitted_values_into_env() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "timesfm_forecast");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({"horizon": 24, "max_context": 512}),
    )
    .unwrap();

    // Integer params render without a decimal point, matching the legacy
    // `spec.horizon.to_string()` / `spec.max_context.to_string()` channel.
    assert_eq!(compiled.env.get("TIMESFM_HORIZON").unwrap(), "24");
    assert_eq!(compiled.env.get("TIMESFM_MAX_CONTEXT").unwrap(), "512");
    // Static env is unaffected by submitted values.
    assert_eq!(
        compiled.env.get("TIMESFM_LOCAL_FILES_ONLY").unwrap(),
        "true"
    );
}

#[test]
fn timesfm_forecast_plugin_schema_matches_the_legacy_param_surface() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "timesfm_forecast");

    let schema =
        serde_json::to_value(container_plugin::compile::compile_schema(&node.params)).unwrap();
    // Both legacy params have serde defaults, so nothing is required.
    // Deliberate delta: the legacy schema also advertised
    // `artifact_prefix` and `timeout_secs` (harness metadata the runner
    // never reads); the plugin DSL pins them at node level instead.
    let required = schema["required"].as_array().unwrap();
    assert!(required.is_empty());
    let horizon = &schema["properties"]["horizon"];
    assert_eq!(horizon["type"], "integer");
    assert_eq!(horizon["default"], 12);
    assert_eq!(horizon["minimum"], 1.0);
    assert_eq!(horizon["maximum"], 256.0);
    let max_context = &schema["properties"]["max_context"];
    assert_eq!(max_context["type"], "integer");
    assert_eq!(max_context["default"], 1024);
    assert_eq!(max_context["minimum"], 1.0);
    assert_eq!(max_context["maximum"], 16128.0);
}

#[test]
fn timesfm_forecast_plugin_enforces_the_legacy_validate_bounds() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "timesfm_forecast");

    // Legacy "horizon must be between 1 and 256": inclusive bounds reject
    // both endpoints at compile time.
    let error = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({"horizon": 0}),
    )
    .unwrap_err();
    assert!(error.to_string().contains("horizon"), "{error}");
    assert!(error.to_string().contains("minimum"), "{error}");

    let error = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({"horizon": 257}),
    )
    .unwrap_err();
    assert!(error.to_string().contains("horizon"), "{error}");
    assert!(error.to_string().contains("maximum"), "{error}");

    // Legacy "max_context must be between 1 and 16128": the wrapper's own
    // unit test rejected 16384.
    let error = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({"max_context": 16384}),
    )
    .unwrap_err();
    assert!(error.to_string().contains("max_context"), "{error}");
    assert!(error.to_string().contains("maximum"), "{error}");

    // Boundary values the legacy validate() accepted stay accepted.
    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({"horizon": 256, "max_context": 16128}),
    )
    .unwrap();
    assert_eq!(compiled.env.get("TIMESFM_HORIZON").unwrap(), "256");
    assert_eq!(compiled.env.get("TIMESFM_MAX_CONTEXT").unwrap(), "16128");
}
