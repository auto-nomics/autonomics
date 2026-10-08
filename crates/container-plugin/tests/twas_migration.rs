//! Golden parity tests for the twas plugin: the `[[nodes]]` entry must
//! compile to the same `Container` spec the legacy Rust wrapper
//! (`nodes-io/src/twas_fusion_container.rs`) produced. The plugin directory
//! lives outside this repository (`/mnt/projects/node-plugins/twas` by
//! default, overridable via `NODE_PLUGINS_ROOT`); the test is skipped when
//! absent so CI without the plugin checkout stays green.

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
    let manifest = root.join("twas").join("manifest.toml");
    if manifest.is_file() {
        return Some(root);
    }
    if explicit.is_some() {
        // fix-review R04 (2026-10-05): an explicitly set NODE_PLUGINS_ROOT
        // means migration parity was requested. A missing family must fail
        // loudly — early-returns here used to count as *passed* tests, so a
        // green summary claimed coverage that never ran.
        panic!(
            "NODE_PLUGINS_ROOT is set but the twas family is not deployed \
             under it ({}) — migration parity cannot run. Deploy the family \
             or unset NODE_PLUGINS_ROOT to skip these tests deliberately.",
            manifest.display()
        );
    }
    eprintln!("skipping: twas plugin directory not present (NODE_PLUGINS_ROOT unset)");
    None
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let text = std::fs::read_to_string(root.join("twas").join("manifest.toml")).unwrap();
    // Inline script_file the way the loader does.
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        if let Some(relative) = node.command.script_file.clone() {
            let source = std::fs::read_to_string(root.join("twas").join(&relative)).unwrap();
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

/// `tissue` and `chr` have no defaults and are not optional, so every
/// compile under test must submit them (the legacy wrapper's Spec had them
/// as plain required fields).
fn required_params() -> serde_json::Value {
    json!({ "tissue": "Whole_Blood", "chr": 21 })
}

#[test]
fn twas_fusion_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "twas_fusion");

    let compiled =
        compile_container_spec(node, &manifest.image, &manifest.panels, &required_params())
            .unwrap();

    assert_eq!(
        compiled.image,
        "ghcr.io/auto-nomics/autonomics/fusion@sha256:91d11747476967b0131571308f2a64bb12aa459205f282103f6bed4fb69ca0bf"
    );
    assert_eq!(compiled.outputs.len(), 3);
    assert_eq!(compiled.outputs[0].path, "twas_fusion.dat");
    assert_eq!(compiled.outputs[0].format.as_deref(), Some("fusion_twas"));
    assert_eq!(compiled.outputs[1].path, "twas_fusion.log");
    assert_eq!(
        compiled.outputs[1].format.as_deref(),
        Some("fusion_twas_log")
    );
    assert_eq!(compiled.outputs[2].path, "twas_fusion.mhc.dat");
    assert_eq!(
        compiled.outputs[2].format.as_deref(),
        Some("fusion_twas_mhc")
    );
    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert!(matches!(
        compiled.pull_policy,
        container_runtime::PullPolicy::Missing
    ));
    assert_eq!(compiled.timeout_secs, 3600);
    // Deliberate delta: the legacy default was
    // "/artifacts/twas_fusion_container"; the plugin follows the kind
    // rename (`_container` suffix dropped), the same rule the ldsc and
    // mrpresso family migrations applied.
    assert_eq!(
        compiled.artifact_prefix,
        Some("/artifacts/twas_fusion".to_string())
    );
    assert_eq!(compiled.workdir, None);
    assert!(compiled.panels.is_empty());

    // The whole FUSION reference (GTEx v8 weight archives + 1000G EUR
    // LDREF) rides in one catalog panel, exactly as the legacy wrapper
    // mounted it. The LD reference is not a node input.
    assert_eq!(compiled.panel_bundles.len(), 1);
    assert_eq!(
        compiled.panel_bundles[0].panel_id,
        "wjixiang/catalog-fusion-gtex-v8"
    );
    assert_eq!(compiled.panel_bundles[0].mount_path, "/panels/fusion_ref");

    // Legacy command was the baked ["/opt/fusion/bin/run_fusion_twas.sh"];
    // the plugin compiles to sh + the inlined script (materialized at
    // /work/.autonomics/script). Documented structural delta.
    assert_eq!(compiled.command, vec!["sh".to_string()]);

    // Defaults render through the env channel with the same values the
    // legacy container_spec produced. Booleans are the documented delta:
    // the legacy shell_bool rendered "0"/"1", the plugin renders the
    // serde_json spellings and the script tests = "true".
    assert_eq!(
        compiled.env.get("FUSION_TISSUE_ARCHIVE").unwrap(),
        "GTExv8.ALL.Whole_Blood.tar.gz"
    );
    assert_eq!(compiled.env.get("FUSION_CHR").unwrap(), "21");
    assert_eq!(
        compiled.env.get("FUSION_USE_NOFILTER_WEIGHTS").unwrap(),
        "false"
    );
    assert_eq!(compiled.env.get("FUSION_FORCE_MODEL").unwrap(), "");
    assert_eq!(compiled.env.get("FUSION_MAX_IMPUTE").unwrap(), "0.5");
    assert_eq!(compiled.env.get("FUSION_MIN_R2PRED").unwrap(), "0.7");
    assert_eq!(compiled.env.get("FUSION_PERM").unwrap(), "0");
    assert_eq!(compiled.env.get("FUSION_PERM_MINP").unwrap(), "0.05");

    // Script parity is semantic, not byte-exact: the plugin script is
    // adapted from the image-baked run_fusion_twas.sh, so the
    // FUSION.assoc_test.R invocation tokens are identical while the
    // parameter plumbing runs through env.
    let script = compiled.script.as_deref().unwrap();
    // Panel layout: weights archives and LDREF under one mount.
    assert!(script.contains("/panels/fusion_ref/weights"));
    assert!(script.contains("/panels/fusion_ref/LDREF"));
    // Tissue safety guard: the legacy Rust validate() charset check the
    // DSL cannot express lives as a fail-closed case stanza over the
    // archive envelope (GTExv8.ALL.<tissue>.tar.gz).
    assert!(script.contains("tissue=${archive_name#GTExv8.ALL.}"));
    assert!(script.contains("A-Za-z0-9_-"));
    assert!(script.contains("GTExv8.ALL."));
    // nofilter selection: boolean spelled "true" on the plugin env channel.
    assert!(
        script.contains(
            "\"${FUSION_USE_NOFILTER_WEIGHTS:-false}\" = \"true\" ] && printf nofilter.pos"
        )
    );
    // Weight archive extraction, identical to the legacy runner.
    assert!(script.contains("tar --no-same-owner -xzf"));
    // Optional force_model flag with the enum guard.
    assert!(script.contains("blup | lasso | top1 | enet"));
    assert!(script.contains("--force_model ${FUSION_FORCE_MODEL}"));
    // Permutation flags only when perm > 0.
    assert!(script.contains("--perm ${FUSION_PERM} --perm_minp ${FUSION_PERM_MINP}"));
    // The official call: same argument names, same order, same values.
    assert!(script.contains("Rscript /opt/fusion/FUSION.assoc_test.R \\"));
    assert!(script.contains("--sumstats \"${AUTONOMICS_INPUT0}\""));
    assert!(script.contains("--weights \"${weights_pos}\""));
    assert!(script.contains("--weights_dir /work/weights"));
    assert!(script.contains("--ref_ld_chr \"${ldref_root}/1000G.EUR.\""));
    assert!(script.contains("--chr \"${FUSION_CHR}\""));
    assert!(script.contains("--max_impute \"${FUSION_MAX_IMPUTE}\""));
    assert!(script.contains("--min_r2pred \"${FUSION_MIN_R2PRED}\""));
    assert!(script.contains("--out \"${out_prefix}\""));
    // Artifact plumbing: main table, log, and the separated MHC table.
    assert!(script.contains("cp \"${out_prefix}\" \"${AUTONOMICS_OUTPUT0}\""));
    assert!(script.contains("cp \"${out_prefix}.MHC\" \"${AUTONOMICS_OUTPUT2}\""));
    assert!(script.contains("> \"${AUTONOMICS_OUTPUT1}\" 2>&1"));
}

#[test]
fn twas_fusion_plugin_renders_submitted_values_into_env() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "twas_fusion");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({
            "tissue": "Skin_Sun_Exposed",
            "chr": 9,
            "force_model": "top1",
            "use_nofilter_weights": true,
            "perm": 100,
            "perm_minp": 0.01
        }),
    )
    .unwrap();

    assert_eq!(
        compiled.env.get("FUSION_TISSUE_ARCHIVE").unwrap(),
        "GTExv8.ALL.Skin_Sun_Exposed.tar.gz"
    );
    assert_eq!(compiled.env.get("FUSION_CHR").unwrap(), "9");
    assert_eq!(compiled.env.get("FUSION_FORCE_MODEL").unwrap(), "top1");
    assert_eq!(
        compiled.env.get("FUSION_USE_NOFILTER_WEIGHTS").unwrap(),
        "true"
    );
    assert_eq!(compiled.env.get("FUSION_PERM").unwrap(), "100");
    assert_eq!(compiled.env.get("FUSION_PERM_MINP").unwrap(), "0.01");
    // Untouched numeric default keeps its serde_json spelling.
    assert_eq!(compiled.env.get("FUSION_MAX_IMPUTE").unwrap(), "0.5");
}

#[test]
fn twas_fusion_plugin_bounds_match_the_legacy_validate() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "twas_fusion");

    // Legacy validate(): chr must lie in 1..=22.
    let error = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({ "tissue": "Whole_Blood", "chr": 23 }),
    )
    .unwrap_err();
    assert!(error.to_string().contains("violates `maximum`"), "{error}");

    // Legacy validate(): max_impute must lie in (0, 1].
    let error = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({ "tissue": "Whole_Blood", "chr": 21, "max_impute": 0.0 }),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("violates `exclusiveMinimum`"),
        "{error}"
    );

    let error = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({ "tissue": "Whole_Blood", "chr": 21, "min_r2pred": 1.5 }),
    )
    .unwrap_err();
    assert!(error.to_string().contains("violates `maximum`"), "{error}");
}

#[test]
fn twas_fusion_plugin_schema_marks_tissue_and_chr_required() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "twas_fusion");

    let schema =
        serde_json::to_value(container_plugin::compile::compile_schema(&node.params)).unwrap();
    // BTreeMap order: the two legacy required fields stay required; every
    // defaulted or optional field drops out of `required`.
    let required = schema["required"].as_array().unwrap();
    assert_eq!(required, &vec![json!("chr"), json!("tissue")]);

    let chr = &schema["properties"]["chr"];
    assert_eq!(chr["type"], "integer");
    assert_eq!(chr["minimum"], 1.0);
    assert_eq!(chr["maximum"], 22.0);

    let tissue = &schema["properties"]["tissue"];
    assert_eq!(tissue["type"], "string");

    let force_model = &schema["properties"]["force_model"];
    assert_eq!(force_model["type"], "string");

    let nofilter = &schema["properties"]["use_nofilter_weights"];
    assert_eq!(nofilter["type"], "boolean");
    assert_eq!(nofilter["default"], false);

    let max_impute = &schema["properties"]["max_impute"];
    assert_eq!(max_impute["type"], "number");
    assert_eq!(max_impute["default"], 0.5);
    // The legacy validate() rejected 0.0: (0, 1], not [0, 1].
    assert_eq!(max_impute["exclusiveMinimum"], 0.0);
    assert_eq!(max_impute["maximum"], 1.0);

    let min_r2pred = &schema["properties"]["min_r2pred"];
    assert_eq!(min_r2pred["default"], 0.7);
    assert_eq!(min_r2pred["exclusiveMinimum"], 0.0);
    assert_eq!(min_r2pred["maximum"], 1.0);

    let perm = &schema["properties"]["perm"];
    assert_eq!(perm["type"], "integer");
    assert_eq!(perm["default"], 0);
    assert_eq!(perm["minimum"], 0.0);

    let perm_minp = &schema["properties"]["perm_minp"];
    assert_eq!(perm_minp["default"], 0.05);
    assert_eq!(perm_minp["exclusiveMinimum"], 0.0);
    assert_eq!(perm_minp["maximum"], 1.0);
}
