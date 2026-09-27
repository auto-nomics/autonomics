//! Golden parity tests for the pathology plugin family: each of the seven
//! `[[nodes]]` entries must compile to the same `Container` spec the legacy
//! Rust wrapper (`nodes-io::pathology_container`) produced. The plugin
//! directory lives outside this repository (`/mnt/projects/node-plugins/
//! pathology` by default, overridable via `NODE_PLUGINS_ROOT`); the test is
//! skipped when absent so CI without the plugin checkout stays green.
//!
//! Parity shape for this family: six of the seven nodes are pure
//! baked-runner nodes (`python /opt/pathology/pathology_runner.py
//! <subcommand>`), so their compiled command AND their single settings-env
//! value are asserted byte-exact — the manifest env templates rebuild the
//! compact serde_json blob the wrapper serialized. `pathology_ihc_quant`
//! carries a script (the optional `roi_label` needs conditional JSON
//! assembly), so it is asserted on semantic script markers plus its flat
//! env channel, per the migration doc's "script is semantic, not
//! byte-exact" rule.

use std::path::PathBuf;

use container_plugin::compile::spec_compile::compile_container_spec;
use container_plugin::manifest::PluginManifest;
use serde_json::json;

fn plugin_root() -> Option<PathBuf> {
    let root = std::env::var_os("NODE_PLUGINS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/projects/node-plugins"));
    let manifest = root.join("pathology").join("manifest.toml");
    manifest.is_file().then_some(root)
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let text = std::fs::read_to_string(root.join("pathology").join("manifest.toml")).unwrap();
    // Inline script_file the way the loader does.
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        if let Some(relative) = node.command.script_file.clone() {
            let source = std::fs::read_to_string(root.join("pathology").join(&relative)).unwrap();
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

const IMAGE: &str =
    "ghcr.io/auto-nomics/autonomics/pathology@sha256:a0edcb6cca25f009f669723406207651284960425f7255891be5b91b29b63f2f";
const RUNNER: &str = "/opt/pathology/pathology_runner.py";

struct Expected {
    kind: &'static str,
    /// Full compiled `command` (interpreter first), byte-exact.
    command: &'static [&'static str],
    /// The single settings env key and its byte-exact default value.
    settings_key: &'static str,
    settings_default: &'static str,
    outputs: &'static [(&'static str, &'static str)],
    timeout_secs: u64,
    artifact_prefix: &'static str,
    gpus: Option<&'static str>,
}

fn expected_cases() -> Vec<Expected> {
    vec![
        Expected {
            kind: "pathology_wsi_ingest",
            command: &["python", RUNNER, "wsi-ingest"],
            settings_key: "PATHOLOGY_WSI_SETTINGS",
            settings_default: r#"{"max_thumbnail_width":2048}"#,
            outputs: &[("thumbnail.png", "png"), ("slide_meta.json", "json")],
            timeout_secs: 3600,
            artifact_prefix: "/artifacts/pathology_wsi_ingest",
            gpus: None,
        },
        Expected {
            kind: "pathology_wsi_qc",
            command: &["python", RUNNER, "wsi-qc"],
            settings_key: "PATHOLOGY_QC_SETTINGS",
            settings_default: concat!(
                r#"{"level":-1,"max_downsample":16.0,"tile_size":512,"max_tiles":400,"#,
                r#""saturation_threshold":0.2,"value_floor":0.15,"value_ceiling":0.92,"#,
                r#""focus_threshold":40.0,"min_slide_tissue_fraction":0.05}"#
            ),
            outputs: &[("tile_qc.parquet", "parquet"), ("qc_summary.json", "json")],
            timeout_secs: 3600,
            artifact_prefix: "/artifacts/pathology_wsi_qc",
            gpus: None,
        },
        Expected {
            kind: "pathology_patch_sample",
            command: &["python", RUNNER, "patch-sample"],
            settings_key: "PATHOLOGY_PATCH_SETTINGS",
            settings_default: concat!(
                r#"{"level":0,"patch_size":256,"max_patches":5000,"min_tissue_fraction":0.5,"#,
                r#""seed":0,"mask_max_downsample":64.0,"mask_max_width":4096,"#,
                r#""mask_open_radius":3,"mask_close_radius":3,"mask_min_object_px":500,"#,
                r#""saturation_threshold":0.2,"value_floor":0.15,"value_ceiling":0.92}"#
            ),
            outputs: &[
                ("patches.parquet", "parquet"),
                ("tissue_mask.png", "png"),
                ("patch_meta.json", "json"),
            ],
            timeout_secs: 3600,
            artifact_prefix: "/artifacts/pathology_patch_sample",
            gpus: None,
        },
        Expected {
            kind: "pathology_wsi_embed",
            command: &["python", RUNNER, "wsi-embed"],
            settings_key: "PATHOLOGY_EMBED_SETTINGS",
            // The legacy serializer kept `gpus` inside the blob (the runner
            // ignores it); the template hardcodes the same value as
            // [nodes.resources].gpus for byte parity.
            settings_default: r#"{"batch_size":32,"device":"auto","amp":false,"gpus":"all"}"#,
            outputs: &[("embeddings.h5", "hdf5"), ("embed_meta.json", "json")],
            timeout_secs: 7200,
            artifact_prefix: "/artifacts/pathology_wsi_embed",
            gpus: Some("all"),
        },
        Expected {
            kind: "pathology_domain_check",
            command: &["python", RUNNER, "domain-check"],
            settings_key: "PATHOLOGY_DOMAIN_SETTINGS",
            settings_default: r#"{"l2_normalize":true,"shrinkage":0.1,"silhouette_max_samples":20000,"seed":0}"#,
            outputs: &[
                ("domain_metrics.parquet", "parquet"),
                ("domain_meta.json", "json"),
            ],
            timeout_secs: 3600,
            artifact_prefix: "/artifacts/pathology_domain_check",
            gpus: None,
        },
        // ihc_quant is script-driven; its contract is asserted separately.
        Expected {
            kind: "pathology_ihc_quant",
            command: &["sh"],
            settings_key: "PATHOLOGY_IHC_ROI_LABEL",
            settings_default: "",
            outputs: &[("tile_ihc.parquet", "parquet"), ("ihc_summary.json", "json")],
            timeout_secs: 3600,
            artifact_prefix: "/artifacts/pathology_ihc_quant",
            gpus: None,
        },
        Expected {
            kind: "pathology_qupath_import",
            command: &["python", RUNNER, "qupath-import"],
            settings_key: "PATHOLOGY_QUPATH_SETTINGS",
            settings_default: concat!(
                r#"{"mask_downsample":16.0,"label_names":{},"#,
                r#""simplify_tolerance_px":2.0,"min_region_px":200}"#
            ),
            outputs: &[
                ("annotations.geojson", "geojson"),
                ("geojson_meta.json", "json"),
            ],
            timeout_secs: 3600,
            artifact_prefix: "/artifacts/pathology_qupath_import",
            gpus: None,
        },
    ]
}

#[test]
fn pathology_nodes_compile_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    assert_eq!(manifest.plugin_name, "pathology");
    assert_eq!(manifest.nodes.len(), 7);
    assert!(manifest.panels.is_empty(), "the family binds no panels");
    for node in &manifest.nodes {
        container_plugin::node_definition::validate(node)
            .unwrap_or_else(|error| panic!("{}: {error}", node.kind));
    }

    for case in expected_cases() {
        let node = node_by_kind(&manifest, case.kind);
        let compiled = compile_container_spec(node, &manifest.image, &manifest.panels, &json!({}))
            .unwrap_or_else(|error| panic!("{}: {error}", case.kind));

        assert_eq!(compiled.image, IMAGE, "{}", case.kind);
        assert_eq!(
            compiled.command,
            case.command.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            "{}",
            case.kind
        );
        assert_eq!(compiled.timeout_secs, case.timeout_secs, "{}", case.kind);
        assert_eq!(compiled.artifact_prefix, case.artifact_prefix, "{}", case.kind);
        assert_eq!(compiled.workdir, None, "{}", case.kind);
        assert!(compiled.files.is_empty(), "{}", case.kind);
        assert_eq!(compiled.network, "isolated", "{}", case.kind);
        assert!(compiled.read_only_rootfs, "{}", case.kind);
        assert!(matches!(
            compiled.pull_policy,
            container_runtime::PullPolicy::Missing
        ));
        assert_eq!(
            compiled.gpus.as_deref(),
            case.gpus,
            "gpu profile mismatch on {}",
            case.kind
        );
        assert!(compiled.panels.is_empty(), "{}", case.kind);
        assert!(compiled.panel_bundles.is_empty(), "{}", case.kind);

        assert_eq!(
            compiled.outputs.len(),
            case.outputs.len(),
            "{}",
            case.kind
        );
        for (output, (path, format)) in compiled.outputs.iter().zip(case.outputs) {
            assert_eq!(output.path, *path, "{}", case.kind);
            assert_eq!(output.format.as_deref(), Some(*format), "{}", case.kind);
        }

        if case.kind == "pathology_ihc_quant" {
            // Script-driven variant: the settings blob is assembled at
            // runtime from the flat env channel; see the dedicated test.
            assert_eq!(compiled.env.len(), 7, "{}", case.kind);
            assert!(compiled.script.is_some(), "{}", case.kind);
        } else {
            assert_eq!(compiled.env.len(), 1, "{}", case.kind);
            assert_eq!(
                compiled.env.get(case.settings_key).unwrap(),
                case.settings_default,
                "settings blob drifted from the legacy wrapper on {}",
                case.kind
            );
            assert!(compiled.script.is_none(), "{}", case.kind);
        }
    }
}

#[test]
fn pathology_ihc_quant_script_assembles_the_legacy_settings_blob() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "pathology_ihc_quant");

    let compiled = compile_container_spec(node, &manifest.image, &manifest.panels, &json!({}))
        .unwrap();
    let script = compiled.script.as_deref().unwrap();

    // The script dispatches the baked runner itself (the semantic equivalent
    // of legacy command[2] == "ihc-quant").
    assert!(script.contains(&format!("exec python {RUNNER} ihc-quant")));
    // Optional roi_label: included only when the flat env channel carries a
    // value, so the runner's smallest-label default engages when omitted.
    assert!(script.contains(r#"[ -n "$PATHOLOGY_IHC_ROI_LABEL" ]"#));
    assert!(script.contains(r#"\"roi_label\":${PATHOLOGY_IHC_ROI_LABEL}"#));
    // The remaining keys mirror the legacy struct order and spellings.
    assert!(script.contains(r#"\"mask_downsample\":${PATHOLOGY_IHC_MASK_DOWNSAMPLE}"#));
    assert!(script.contains(r#"\"dab_weak_threshold\":${PATHOLOGY_IHC_DAB_WEAK_THRESHOLD}"#));
    assert!(script.contains(r#"\"dab_strong_threshold\":${PATHOLOGY_IHC_DAB_STRONG_THRESHOLD}"#));
    assert!(script.contains(r#"\"max_tiles\":${PATHOLOGY_IHC_MAX_TILES}"#));
    assert!(script.contains("export PATHOLOGY_IHC_SETTINGS"));

    // Flat env channel with defaults applied; roi_label absent renders "".
    assert_eq!(compiled.env.get("PATHOLOGY_IHC_ROI_LABEL").unwrap(), "");
    assert_eq!(
        compiled.env.get("PATHOLOGY_IHC_MASK_DOWNSAMPLE").unwrap(),
        "16.0"
    );
    assert_eq!(
        compiled.env.get("PATHOLOGY_IHC_DAB_WEAK_THRESHOLD").unwrap(),
        "0.15"
    );
    assert_eq!(compiled.env.get("PATHOLOGY_IHC_TILE_SIZE").unwrap(), "512");
    assert_eq!(compiled.env.get("PATHOLOGY_IHC_MAX_TILES").unwrap(), "20000");
}

#[test]
fn pathology_wsi_embed_renders_submitted_values_into_the_settings_blob() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "pathology_wsi_embed");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({"batch_size": 8, "device": "cpu", "amp": true}),
    )
    .unwrap();

    // Strings stay quoted, bools render unquoted, and the pinned gpus value
    // survives — byte-identical to the legacy env for the same spec.
    assert_eq!(
        compiled.env.get("PATHOLOGY_EMBED_SETTINGS").unwrap(),
        r#"{"batch_size":8,"device":"cpu","amp":true,"gpus":"all"}"#
    );
    assert_eq!(compiled.gpus.as_deref(), Some("all"));
}

#[test]
fn pathology_wsi_qc_renders_submitted_values_into_the_settings_blob() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "pathology_wsi_qc");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({"level": 2, "tile_size": 128, "max_downsample": 8.0}),
    )
    .unwrap();

    // serde_json spelling: f64 8.0 renders "8.0", ints render bare.
    assert_eq!(
        compiled.env.get("PATHOLOGY_QC_SETTINGS").unwrap(),
        concat!(
            r#"{"level":2,"max_downsample":8.0,"tile_size":128,"max_tiles":400,"#,
            r#""saturation_threshold":0.2,"value_floor":0.15,"value_ceiling":0.92,"#,
            r#""focus_threshold":40.0,"min_slide_tissue_fraction":0.05}"#
        )
    );
}

#[test]
fn pathology_qupath_import_carries_the_label_name_map_verbatim() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "pathology_qupath_import");

    // label_names is a string param holding a JSON object literal (the v0
    // DSL has no object params); a compact submission reproduces the legacy
    // serde BTreeMap bytes exactly.
    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({"label_names": "{\"1\":\"Tumor\"}"}),
    )
    .unwrap();
    assert_eq!(
        compiled.env.get("PATHOLOGY_QUPATH_SETTINGS").unwrap(),
        r#"{"mask_downsample":16.0,"label_names":{"1":"Tumor"},"simplify_tolerance_px":2.0,"min_region_px":200}"#
    );
}

#[test]
fn pathology_schema_marks_roi_label_optional_and_surfaces_bounds() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);

    let ihc = serde_json::to_value(container_plugin::compile::compile_schema(
        &node_by_kind(&manifest, "pathology_ihc_quant").params,
    ))
    .unwrap();
    let required = ihc["required"].as_array().unwrap();
    assert!(required.is_empty(), "every ihc param has a default or is optional");
    assert_eq!(ihc["properties"]["roi_label"]["type"], "integer");
    assert!(ihc["properties"]["roi_label"].get("default").is_none());
    assert_eq!(
        ihc["properties"]["dab_weak_threshold"]["exclusiveMinimum"],
        0.0
    );

    let domain = serde_json::to_value(container_plugin::compile::compile_schema(
        &node_by_kind(&manifest, "pathology_domain_check").params,
    ))
    .unwrap();
    assert_eq!(domain["properties"]["shrinkage"]["exclusiveMaximum"], 1.0);

    let qc = serde_json::to_value(container_plugin::compile::compile_schema(
        &node_by_kind(&manifest, "pathology_wsi_qc").params,
    ))
    .unwrap();
    assert_eq!(qc["properties"]["level"]["minimum"], -1.0);
    assert_eq!(qc["properties"]["value_floor"]["minimum"], 0.0);
    assert_eq!(qc["properties"]["value_floor"]["maximum"], 1.0);
}

#[test]
fn pathology_bounds_reject_the_values_the_wrapper_rejected() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);

    // Legacy: shrinkage must lie in [0, 1).
    let domain = node_by_kind(&manifest, "pathology_domain_check");
    assert!(compile_container_spec(domain, &manifest.image, &manifest.panels, &json!({"shrinkage": 1.0})).is_err());

    // Legacy: mask_max_width must be at least 256.
    let patch = node_by_kind(&manifest, "pathology_patch_sample");
    assert!(compile_container_spec(patch, &manifest.image, &manifest.panels, &json!({"mask_max_width": 128})).is_err());

    // Legacy: max_downsample must be >= 1.
    let qc = node_by_kind(&manifest, "pathology_wsi_qc");
    assert!(compile_container_spec(qc, &manifest.image, &manifest.panels, &json!({"max_downsample": 0.5})).is_err());
}
