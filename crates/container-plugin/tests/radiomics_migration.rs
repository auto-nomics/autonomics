//! Golden parity tests for the radiomics plugin family: each of the 21
//! `[[nodes]]` entries must compile to the same `Container` spec the legacy
//! Rust wrapper (`nodes-io/src/radiomics_container.rs`) produced. The
//! plugin directory lives outside this repository
//! (`/mnt/projects/node-plugins/radiomics` by default, overridable via
//! `NODE_PLUGINS_ROOT`); the test is skipped when absent so CI without the
//! plugin checkout stays green.
//!
//! Parity shape (see `docs/plugin-node-migration.md`): image, outputs,
//! panels, resources, timeout, and artifact prefix are byte-exact; the
//! script is semantic only. The legacy wrapper built one JSON settings
//! blob per variant inside Rust, so the plugin scripts rebuild that blob
//! from typed env vars — the compiled `env` therefore carries the typed
//! variables, and the script markers assert the rebuild + exec contract.

use std::path::PathBuf;

use container_plugin::compile::spec_compile::compile_container_spec;
use container_plugin::manifest::PluginManifest;
use serde_json::json;

const IMAGE_REFERENCE: &str = "ghcr.io/auto-nomics/autonomics/pyradiomics@sha256:bccbe15b2ec8d079e1bf869c4f06bfe4143642015394453c584dc981e5403fbe";
const RUNNER: &str = "/opt/radiomics/radiomics_runner.py";
const TIMEOUT_SECS: u64 = 3600;

/// The legacy kind constants (`radiomics_container.rs`), unchanged: the
/// family never carried a `_container` kind suffix to strip.
const LEGACY_KINDS: &[&str] = &[
    "radiomics_image_ingest",
    "radiomics_mask_ingest",
    "radiomics_pair_validate",
    "radiomics_preprocess",
    "pyradiomics_extract",
    "pyradiomics_batch_extract",
    "radiomics_dicom_metadata",
    "radiomics_phi_scrub",
    "radiomics_voi_dice_hausdorff",
    "radiomics_image_qc",
    "radiomics_rtstruct_geometry",
    "radiomics_ivh_extract",
    "radiomics_shape_topology",
    "radiomics_register",
    "radiomics_delta_features",
    "radiomics_bias_correct",
    "radiomics_robust_normalize",
    "radiomics_peritumoral_ring",
    "radiomics_habitat_fit",
    "radiomics_habitat_assign",
    "radiomics_perturb_stability",
];

struct Contract {
    kind: &'static str,
    artifact_prefix: &'static str,
    outputs: &'static [(&'static str, &'static str)],
    runner_command: &'static str,
    /// Selected `(env var, rendered default)` pairs: ints, bools, and plain
    /// decimals are byte-exact like the legacy `serde_json` spellings. The
    /// one deliberate f64 delta (`convergence_threshold` `1e-6` vs the
    /// script's `1e-06`) is parse-checked in the bias-correct test below.
    env: &'static [(&'static str, &'static str)],
}

const CONTRACTS: &[Contract] = &[
    Contract {
        kind: "radiomics_image_ingest",
        artifact_prefix: "/artifacts/radiomics_image_ingest",
        outputs: &[("image.mha", "mha"), ("image_meta.json", "json")],
        runner_command: "ingest-image",
        env: &[
            ("RADIOMICS_Z_SORT", "position"),
            ("RADIOMICS_Z_DIRECTION", "ascending"),
        ],
    },
    Contract {
        kind: "radiomics_mask_ingest",
        artifact_prefix: "/artifacts/radiomics_mask_ingest",
        outputs: &[("mask.mha", "mha"), ("roi_meta.json", "json")],
        runner_command: "ingest-mask",
        env: &[
            ("RADIOMICS_ROI_NAME", ""),
            ("RADIOMICS_Z_SORT", "position"),
            ("RADIOMICS_Z_DIRECTION", "ascending"),
        ],
    },
    Contract {
        kind: "radiomics_pair_validate",
        artifact_prefix: "/artifacts/radiomics_pair_validate",
        outputs: &[
            ("pair_validation.parquet", "parquet"),
            ("geometry.json", "json"),
        ],
        runner_command: "validate-pair",
        env: &[
            ("RADIOMICS_MASK_LABEL", "1"),
            ("RADIOMICS_MINIMUM_MASK_VOXELS", "1"),
            ("RADIOMICS_GEOMETRY_TOLERANCE_MM", "0.01"),
        ],
    },
    Contract {
        kind: "radiomics_preprocess",
        artifact_prefix: "/artifacts/radiomics_preprocess",
        outputs: &[
            ("image.mha", "mha"),
            ("mask.mha", "mha"),
            ("preprocess_meta.json", "json"),
        ],
        runner_command: "preprocess",
        env: &[
            ("RADIOMICS_RESAMPLED_SPACING", ""),
            ("RADIOMICS_INTERPOLATOR", "sitkBSpline"),
            ("RADIOMICS_RESEGMENT_RANGE", ""),
            ("RADIOMICS_NORMALIZE", "false"),
        ],
    },
    Contract {
        kind: "pyradiomics_extract",
        artifact_prefix: "/artifacts/pyradiomics_extract",
        outputs: &[
            ("features_wide.parquet", "parquet"),
            ("features_long.parquet", "parquet"),
            ("feature_metadata.parquet", "parquet"),
            ("diagnostics.parquet", "parquet"),
            ("provenance.json", "json"),
        ],
        runner_command: "extract-single",
        env: &[
            ("RADIOMICS_ROI_NAME", ""),
            ("RADIOMICS_MASK_LABEL", "1"),
            ("RADIOMICS_BIN_WIDTH", "25.0"),
            ("RADIOMICS_RESAMPLED_SPACING", ""),
            ("RADIOMICS_FORCE2D", "false"),
            ("RADIOMICS_FORCE2D_DIMENSION", "0"),
            ("RADIOMICS_IMAGE_TYPES", "Original"),
            (
                "RADIOMICS_FEATURE_CLASSES",
                "shape firstorder glcm glrlm glszm gldm ngtdm",
            ),
            ("RADIOMICS_LOG_SIGMAS", "2 3 4 5"),
        ],
    },
    Contract {
        kind: "pyradiomics_batch_extract",
        artifact_prefix: "/artifacts/pyradiomics_batch_extract",
        outputs: &[
            ("features_wide.parquet", "parquet"),
            ("features_long.parquet", "parquet"),
            ("feature_metadata.parquet", "parquet"),
            ("diagnostics.parquet", "parquet"),
            ("provenance.json", "json"),
        ],
        runner_command: "extract-batch",
        env: &[
            ("RADIOMICS_VALID_ONLY", "true"),
            ("RADIOMICS_MASK_LABEL", "1"),
            ("RADIOMICS_BIN_WIDTH", "25.0"),
        ],
    },
    Contract {
        kind: "radiomics_dicom_metadata",
        artifact_prefix: "/artifacts/radiomics_dicom_metadata",
        outputs: &[
            ("dicom_metadata.parquet", "parquet"),
            ("dicom_metadata_report.json", "json"),
        ],
        runner_command: "dicom-metadata",
        env: &[("RADIOMICS_EXTRA_TAGS", "")],
    },
    Contract {
        kind: "radiomics_phi_scrub",
        artifact_prefix: "/artifacts/radiomics_phi_scrub",
        outputs: &[
            ("scrubbed_dicom.zip", "dicom_zip"),
            ("phi_scrub_report.parquet", "parquet"),
            ("phi_scrub_report.json", "json"),
        ],
        runner_command: "phi-scrub",
        env: &[
            ("RADIOMICS_KEEP_PATIENT_ID", "false"),
            ("RADIOMICS_PSEUDONYM", ""),
        ],
    },
    Contract {
        kind: "radiomics_voi_dice_hausdorff",
        artifact_prefix: "/artifacts/radiomics_voi_dice_hausdorff",
        outputs: &[
            ("voi_similarity.parquet", "parquet"),
            ("voi_similarity.json", "json"),
        ],
        runner_command: "voi-similarity",
        env: &[
            ("RADIOMICS_MASK_LABEL", "1"),
            ("RADIOMICS_SURFACE_TOLERANCE_MM", "1.0"),
        ],
    },
    Contract {
        kind: "radiomics_image_qc",
        artifact_prefix: "/artifacts/radiomics_image_qc",
        outputs: &[("image_qc.parquet", "parquet"), ("image_qc.json", "json")],
        runner_command: "image-qc",
        env: &[("RADIOMICS_MASK_LABEL", "1")],
    },
    Contract {
        kind: "radiomics_rtstruct_geometry",
        artifact_prefix: "/artifacts/radiomics_rtstruct_geometry",
        outputs: &[
            ("rtstruct_geometry.parquet", "parquet"),
            ("rtstruct_geometry.json", "json"),
        ],
        runner_command: "rtstruct-geometry",
        env: &[("RADIOMICS_ROI_NAME", "")],
    },
    Contract {
        kind: "radiomics_ivh_extract",
        artifact_prefix: "/artifacts/radiomics_ivh_extract",
        outputs: &[("ivh.parquet", "parquet"), ("ivh.json", "json")],
        runner_command: "ivh-extract",
        env: &[
            ("RADIOMICS_MASK_LABEL", "1"),
            (
                "RADIOMICS_VOLUME_FRACTIONS",
                "0.05 0.1 0.15 0.2 0.25 0.3 0.35 0.4 0.45 0.5 0.55 0.6 0.65 0.7 0.75 0.8 0.85 0.9 0.95",
            ),
        ],
    },
    Contract {
        kind: "radiomics_shape_topology",
        artifact_prefix: "/artifacts/radiomics_shape_topology",
        outputs: &[
            ("shape_topology.parquet", "parquet"),
            ("slice_profile.parquet", "parquet"),
            ("shape_topology.json", "json"),
        ],
        runner_command: "shape-topology",
        env: &[("RADIOMICS_MASK_LABEL", "1")],
    },
    Contract {
        kind: "radiomics_register",
        artifact_prefix: "/artifacts/radiomics_register",
        outputs: &[
            ("registered.mha", "mha"),
            ("transform.tfm", "simpleitk_transform"),
            ("registration.json", "json"),
        ],
        runner_command: "register",
        env: &[
            ("RADIOMICS_TRANSFORM_TYPE", "rigid"),
            ("RADIOMICS_ITERATIONS", "100"),
            ("RADIOMICS_LEARNING_RATE", "1.0"),
            ("RADIOMICS_SAMPLING_PERCENT", "0.15"),
        ],
    },
    Contract {
        kind: "radiomics_delta_features",
        artifact_prefix: "/artifacts/radiomics_delta_features",
        outputs: &[
            ("delta_features.parquet", "parquet"),
            ("delta_features_report.json", "json"),
        ],
        runner_command: "delta-features",
        env: &[
            ("RADIOMICS_ID_COLUMN", "patient_id"),
            ("RADIOMICS_TIMEPOINT_COLUMN", "timepoint"),
            ("RADIOMICS_BASELINE", "baseline"),
            ("RADIOMICS_FOLLOWUP", "followup"),
        ],
    },
    Contract {
        kind: "radiomics_bias_correct",
        artifact_prefix: "/artifacts/radiomics_bias_correct",
        outputs: &[
            ("corrected.mha", "mha"),
            ("bias_field.mha", "mha"),
            ("bias_meta.json", "json"),
        ],
        runner_command: "bias-correct",
        env: &[
            ("RADIOMICS_MASK_LABEL", "1"),
            ("RADIOMICS_SHRINK_FACTOR", "4"),
            ("RADIOMICS_MAX_ITERATIONS", "50 50 50 50"),
        ],
    },
    Contract {
        kind: "radiomics_robust_normalize",
        artifact_prefix: "/artifacts/radiomics_robust_normalize",
        outputs: &[("normalized.mha", "mha"), ("normalize_meta.json", "json")],
        runner_command: "normalize",
        env: &[
            ("RADIOMICS_MASK_LABEL", "1"),
            ("RADIOMICS_LOWER_PERCENTILE", "1.0"),
            ("RADIOMICS_UPPER_PERCENTILE", "99.0"),
        ],
    },
    Contract {
        kind: "radiomics_peritumoral_ring",
        artifact_prefix: "/artifacts/radiomics_peritumoral_ring",
        outputs: &[
            ("mask_ring.mha", "mha"),
            ("mask_tumor_ring.mha", "mha"),
            ("mask_combined.mha", "mha"),
            ("ring_meta.json", "json"),
        ],
        runner_command: "peritumoral-ring",
        env: &[
            ("RADIOMICS_MASK_LABEL", "1"),
            ("RADIOMICS_INNER_MM", "0.0"),
            ("RADIOMICS_OUTER_MM", "5.0"),
        ],
    },
    Contract {
        kind: "radiomics_habitat_fit",
        artifact_prefix: "/artifacts/radiomics_habitat_fit",
        outputs: &[
            ("habitats.json", "json"),
            ("habitat_fit_samples.parquet", "parquet"),
        ],
        runner_command: "habitat-fit",
        env: &[
            ("RADIOMICS_MASK_LABEL", "1"),
            ("RADIOMICS_N_HABITATS", "3"),
            ("RADIOMICS_SAMPLE_VOXELS_PER_CASE", "10000"),
            ("RADIOMICS_SEED", "0"),
            ("RADIOMICS_STANDARDIZE", "true"),
            ("RADIOMICS_N_INIT", "8"),
            ("RADIOMICS_MAX_ITER", "300"),
        ],
    },
    Contract {
        kind: "radiomics_habitat_assign",
        artifact_prefix: "/artifacts/radiomics_habitat_assign",
        outputs: &[
            ("habitat_mask.mha", "mha"),
            ("habitat_features.parquet", "parquet"),
            ("assign_meta.json", "json"),
        ],
        runner_command: "habitat-assign",
        env: &[("RADIOMICS_MASK_LABEL", "1")],
    },
    Contract {
        kind: "radiomics_perturb_stability",
        artifact_prefix: "/artifacts/radiomics_perturb_stability",
        outputs: &[
            ("replicates_wide.parquet", "parquet"),
            ("replicates_long.parquet", "parquet"),
            ("perturb_meta.json", "json"),
        ],
        runner_command: "perturb-stability",
        env: &[
            (
                "RADIOMICS_PERTURBATIONS",
                "dilate1 erode1 translate_x translate_y translate_z noise",
            ),
            ("RADIOMICS_NOISE_SIGMA_PCT", "2.0"),
            ("RADIOMICS_SEED", "0"),
        ],
    },
];

fn plugin_root() -> Option<PathBuf> {
    let explicit = std::env::var_os("NODE_PLUGINS_ROOT");
    let root = explicit
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/projects/node-plugins"));
    let manifest = root.join("radiomics").join("manifest.toml");
    if manifest.is_file() {
        return Some(root);
    }
    if explicit.is_some() {
        // fix-review R04 (2026-10-05): an explicitly set NODE_PLUGINS_ROOT
        // means migration parity was requested. A missing family must fail
        // loudly — early-returns here used to count as *passed* tests, so a
        // green summary claimed coverage that never ran.
        panic!(
            "NODE_PLUGINS_ROOT is set but the radiomics family is not deployed \
             under it ({}) — migration parity cannot run. Deploy the family \
             or unset NODE_PLUGINS_ROOT to skip these tests deliberately.",
            manifest.display()
        );
    }
    eprintln!("skipping: radiomics plugin directory not present (NODE_PLUGINS_ROOT unset)");
    None
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let text = std::fs::read_to_string(root.join("radiomics").join("manifest.toml")).unwrap();
    // Inline script_file the way the loader does.
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        if let Some(relative) = node.command.script_file.clone() {
            let source = std::fs::read_to_string(root.join("radiomics").join(&relative)).unwrap();
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

/// Seven variants (pair_validate, extract, voi, image_qc, ivh,
/// shape_topology, perturb) carry required identity strings with no
/// default, so the default-value contract compile needs stand-ins for them.
/// They are all unbounded strings, so a constant filler never trips bounds.
fn stand_in_required_values(
    node: &container_plugin::node_definition::NodeDefinition,
) -> serde_json::Value {
    use container_plugin::node_definition::ParamType;
    let mut values = serde_json::Map::new();
    for (name, spec) in &node.params {
        if spec.default.is_some() || spec.optional {
            continue;
        }
        let filler = match spec.r#type {
            ParamType::Bool => json!(false),
            // Flag accepts the same boolean JSON shape (F02).
            ParamType::Flag => json!(false),
            ParamType::Int => json!(1),
            ParamType::Number => json!(1.0),
            ParamType::String => json!("golden"),
            ParamType::StringArray => json!(["golden"]),
        };
        values.insert(name.clone(), filler);
    }
    serde_json::Value::Object(values)
}

#[test]
fn manifest_covers_every_legacy_container_kind_exactly() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let mut kinds: Vec<&str> = manifest.nodes.iter().map(|n| n.kind.as_str()).collect();
    kinds.sort_unstable();
    let mut expected = LEGACY_KINDS.to_vec();
    expected.sort_unstable();
    assert_eq!(kinds, expected, "plugin kinds must equal the legacy set");
}

#[test]
fn every_kind_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);

    // The family shares the pyradiomics image, no panels, and no GPU: the
    // legacy base_spec pinned them once for every variant.
    assert_eq!(manifest.image.reference.to_string(), IMAGE_REFERENCE);
    assert!(manifest.panels.is_empty());

    for contract in CONTRACTS {
        let node = node_by_kind(&manifest, contract.kind);
        let compiled = compile_container_spec(
            node,
            &manifest.image,
            &manifest.panels,
            &stand_in_required_values(node),
        )
        .unwrap_or_else(|error| panic!("{} failed to compile: {error}", contract.kind));

        assert_eq!(compiled.image, IMAGE_REFERENCE, "{}", contract.kind);
        assert_eq!(compiled.timeout_secs, TIMEOUT_SECS, "{}", contract.kind);
        assert_eq!(
            compiled.artifact_prefix, contract.artifact_prefix,
            "{}",
            contract.kind
        );
        assert_eq!(compiled.network, "isolated", "{}", contract.kind);
        assert!(compiled.read_only_rootfs, "{}", contract.kind);
        assert!(matches!(
            compiled.pull_policy,
            container_runtime::PullPolicy::Missing
        ));
        assert_eq!(compiled.workdir, None, "{}", contract.kind);
        assert!(compiled.panels.is_empty(), "{}", contract.kind);
        assert!(compiled.panel_bundles.is_empty(), "{}", contract.kind);
        // Legacy base_spec hard-pinned the resource knobs; the manifest
        // omits [nodes.resources] everywhere and must keep it that way.
        assert!(compiled.gpus.is_none(), "{}", contract.kind);
        assert!(compiled.cpus.is_none(), "{}", contract.kind);
        assert!(compiled.memory.is_none(), "{}", contract.kind);
        assert!(compiled.pids_limit.is_none(), "{}", contract.kind);
        assert!(compiled.shm_size.is_none(), "{}", contract.kind);
        assert!(compiled.user.is_none(), "{}", contract.kind);

        let outputs: Vec<(&str, Option<&str>)> = compiled
            .outputs
            .iter()
            .map(|o| (o.path.as_str(), o.format.as_deref()))
            .collect();
        let expected: Vec<(&str, Option<&str>)> = contract
            .outputs
            .iter()
            .map(|(path, format)| (*path, Some(*format)))
            .collect();
        assert_eq!(outputs, expected, "{} output contract moved", contract.kind);

        // The published plugin runs every kind through a shell wrapper
        // (`interpreter = "sh"`): each script rebuilds the legacy settings
        // blob with `python -c` and then execs the pinned runner, where the
        // legacy command vec passed `python` as the entrypoint directly.
        assert_eq!(compiled.command, vec!["sh".to_string()]);
        let script = compiled.script.as_deref().unwrap();
        assert!(
            script.contains(RUNNER),
            "{} script must invoke the pinned runner",
            contract.kind
        );
        assert!(
            script.contains(&format!("{RUNNER} {}", contract.runner_command)),
            "{} script must exec the legacy runner command",
            contract.kind
        );
        assert!(
            script.contains("json.dumps"),
            "{} script must rebuild the legacy settings blob",
            contract.kind
        );

        for (name, expected) in contract.env {
            assert_eq!(
                compiled.env.get(*name).map(String::as_str),
                Some(*expected),
                "{} env {name}",
                contract.kind
            );
        }
    }
}

#[test]
fn bias_correct_convergence_threshold_parses_to_the_legacy_value() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "radiomics_bias_correct");
    let compiled =
        compile_container_spec(node, &manifest.image, &manifest.panels, &json!({})).unwrap();
    // The default is TOML 1e-6; serde_json and the script json.dumps spell
    // the shortest f64 differently (documented nuance), so assert by parse.
    let rendered = compiled.env.get("RADIOMICS_CONVERGENCE_THRESHOLD").unwrap();
    let parsed: f64 = rendered.parse().unwrap();
    assert!(
        (parsed - 1e-6).abs() < f64::EPSILON,
        "convergence_threshold rendered as {rendered}"
    );
}

#[test]
fn pyradiomics_extract_renders_submitted_values_into_env() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "pyradiomics_extract");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({
            "extraction_id": "case1",
            "patient_id": "patient1",
            "image_id": "image1",
            "roi_id": "gtv",
            "roi_name": "GTV_Mass",
            "modality": "CT",
            "preset_id": "pyradiomics_original_v1",
            "mask_label": 2,
            "bin_width": 32.0,
            "resampled_spacing": "1.0 1.0 1.0",
            "force2d": true,
            "force2d_dimension": 2,
            "image_types": ["Original", "Wavelet"],
            "feature_classes": ["firstorder"],
            "log_sigmas": "2.5 3.5"
        }),
    )
    .unwrap();

    let env = |name: &str| compiled.env.get(name).unwrap().as_str();
    assert_eq!(env("RADIOMICS_EXTRACTION_ID"), "case1");
    assert_eq!(env("RADIOMICS_PATIENT_ID"), "patient1");
    assert_eq!(env("RADIOMICS_IMAGE_ID"), "image1");
    assert_eq!(env("RADIOMICS_ROI_ID"), "gtv");
    assert_eq!(env("RADIOMICS_ROI_NAME"), "GTV_Mass");
    assert_eq!(env("RADIOMICS_MODALITY"), "CT");
    assert_eq!(env("RADIOMICS_PRESET_ID"), "pyradiomics_original_v1");
    assert_eq!(env("RADIOMICS_MASK_LABEL"), "2");
    assert_eq!(env("RADIOMICS_BIN_WIDTH"), "32.0");
    assert_eq!(env("RADIOMICS_RESAMPLED_SPACING"), "1.0 1.0 1.0");
    assert_eq!(env("RADIOMICS_FORCE2D"), "true");
    assert_eq!(env("RADIOMICS_FORCE2D_DIMENSION"), "2");
    // String arrays render space-joined; the script splits them back into
    // the legacy JSON array.
    assert_eq!(env("RADIOMICS_IMAGE_TYPES"), "Original Wavelet");
    assert_eq!(env("RADIOMICS_FEATURE_CLASSES"), "firstorder");
    assert_eq!(env("RADIOMICS_LOG_SIGMAS"), "2.5 3.5");

    // The rebuild script owns the legacy shape2D/force2d gate and the
    // membership checks the manifest schema cannot carry.
    let script = compiled.script.as_deref().unwrap();
    assert!(script.contains("shape2D requires force2d=true"));
    assert!(script.contains("unsupported PyRadiomics image type(s)"));
    assert!(script.contains("log_sigmas must contain 1-10 positive finite values"));
    assert!(script.contains("RADIOMICS_EXTRACTION"));
}

#[test]
fn radiomics_perturb_stability_renders_submitted_values_into_env() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "radiomics_perturb_stability");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({
            "extraction_id": "case1",
            "patient_id": "patient1",
            "image_id": "image1",
            "roi_id": "gtv",
            "modality": "MR",
            "preset_id": "pyradiomics_original_v1",
            "perturbations": ["noise", "dilate1"],
            "noise_sigma_pct": 5.5,
            "seed": 7
        }),
    )
    .unwrap();

    let env = |name: &str| compiled.env.get(name).unwrap().as_str();
    assert_eq!(env("RADIOMICS_PERTURBATIONS"), "noise dilate1");
    assert_eq!(env("RADIOMICS_NOISE_SIGMA_PCT"), "5.5");
    assert_eq!(env("RADIOMICS_SEED"), "7");

    let script = compiled.script.as_deref().unwrap();
    // The legacy wrapper dropped the identity blob the runner requires
    // (see the plugin README): the script now rebuilds RADIOMICS_EXTRACTION.
    assert!(script.contains("RADIOMICS_EXTRACTION"));
    assert!(script.contains("RADIOMICS_PERTURB_SETTINGS"));
    assert!(script.contains("perturbations must be unique"));
}

#[test]
fn radiomics_preprocess_renders_optional_lists_and_nulls() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "radiomics_preprocess");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({
            "resampled_spacing": "1.5 1.5 2.0",
            "resegment_range": "0 200",
            "normalize": true
        }),
    )
    .unwrap();

    let env = |name: &str| compiled.env.get(name).unwrap().as_str();
    assert_eq!(env("RADIOMICS_RESAMPLED_SPACING"), "1.5 1.5 2.0");
    assert_eq!(env("RADIOMICS_RESEGMENT_RANGE"), "0 200");
    assert_eq!(env("RADIOMICS_NORMALIZE"), "true");

    // Empty optionals must reach the script as empty strings, which the
    // script maps to JSON null exactly like the legacy None.
    let script = compiled.script.as_deref().unwrap();
    assert!(script.contains("or None"));
    assert!(script.contains("resegment_range must be finite and ordered"));
}

#[test]
fn pyradiomics_extract_schema_marks_identity_required_and_rest_defaulted() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "pyradiomics_extract");

    let schema =
        serde_json::to_value(container_plugin::compile::compile_schema(&node.params)).unwrap();
    let mut required = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect::<Vec<_>>();
    required.sort_unstable();
    assert_eq!(
        required,
        vec![
            "extraction_id",
            "image_id",
            "modality",
            "patient_id",
            "preset_id",
            "roi_id"
        ]
    );
    // Optionals resolve to null (not required); bounds carry the legacy
    // scalar checks.
    assert_eq!(schema["properties"]["roi_name"]["type"], "string");
    assert_eq!(schema["properties"]["mask_label"]["minimum"], 1.0);
    assert_eq!(schema["properties"]["bin_width"]["exclusiveMinimum"], 0.0);
    assert_eq!(schema["properties"]["force2d_dimension"]["maximum"], 255.0);
    assert_eq!(
        schema["properties"]["image_types"]["items"]["type"],
        "string"
    );
}
