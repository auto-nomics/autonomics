//! Golden parity tests for the mixer plugin family: each `[[nodes]]` entry
//! must compile to the same `Container` spec the legacy Rust wrapper
//! (`nodes-io/src/mixer_container.rs`) produced. The plugin directory lives
//! outside this repository (`/mnt/projects/node-plugins/mixer` by default,
//! overridable via `NODE_PLUGINS_ROOT`); the test is skipped when absent so
//! CI without the plugin checkout stays green.

use std::path::PathBuf;

use container_plugin::compile::spec_compile::compile_container_spec;
use container_plugin::manifest::PluginManifest;
use serde_json::json;

fn plugin_root() -> Option<PathBuf> {
    let root = std::env::var_os("NODE_PLUGINS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/projects/node-plugins"));
    let manifest = root.join("mixer").join("manifest.toml");
    manifest.is_file().then_some(root)
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let text = std::fs::read_to_string(root.join("mixer").join("manifest.toml")).unwrap();
    // Inline script_file the way the loader does.
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        if let Some(relative) = node.command.script_file.clone() {
            let source = std::fs::read_to_string(root.join("mixer").join(&relative)).unwrap();
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
fn mixer_fit1_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "mixer_fit1");

    let compiled =
        compile_container_spec(node, &manifest.image, &manifest.panels, &json!({})).unwrap();

    assert_eq!(
        compiled.image,
        "ghcr.io/auto-nomics/autonomics/mixer@sha256:3bd67cccf298bd3c9af3d2b013dd7dfacde9ad13d51bc78b2f7f1315f01bebb7"
    );
    assert_eq!(compiled.outputs.len(), 2);
    assert_eq!(compiled.outputs[0].path, "mixer_fit1.json");
    assert_eq!(
        compiled.outputs[0].format.as_deref(),
        Some("mixer_fit_json")
    );
    assert_eq!(compiled.outputs[1].path, "mixer_fit1.log");
    assert_eq!(compiled.outputs[1].format.as_deref(), Some("mixer_log"));
    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert!(matches!(
        compiled.pull_policy,
        container_runtime::PullPolicy::Missing
    ));
    assert_eq!(compiled.timeout_secs, 21_600);
    assert_eq!(compiled.artifact_prefix, "/artifacts/mixer_container");
    assert_eq!(compiled.workdir, None);
    assert!(compiled.panels.is_empty());
    assert_eq!(compiled.panel_bundles.len(), 1);
    assert_eq!(
        compiled.panel_bundles[0].panel_id,
        "wjixiang/catalog-mixer-g1000-eur-rsid"
    );
    assert_eq!(
        compiled.panel_bundles[0].mount_path,
        "/panels/mixer_g1000_eur"
    );
    assert_eq!(compiled.command, vec!["sh".to_string()]);
    assert_eq!(compiled.cpus, Some(2.0));
    assert_eq!(compiled.memory.as_deref(), Some("16Gi"));
    assert_eq!(compiled.pids_limit, Some(512));

    // Default spec values travel through env exactly as the legacy wrapper
    // stringified them.
    assert_eq!(compiled.env.get("MIXER_CHR2USE").unwrap(), "1-22");
    assert_eq!(compiled.env.get("MIXER_SEED").unwrap(), "123");
    assert_eq!(
        compiled.env.get("MIXER_DIFFEVO_FAST_REPEATS").unwrap(),
        "20"
    );
    assert_eq!(compiled.env.get("MIXER_FAST_RUN").unwrap(), "true");
    assert_eq!(compiled.env.get("MIXER_KMAX_PDF").unwrap(), "10");
    assert_eq!(compiled.env.get("MIXER_DOWNSAMPLE_FACTOR").unwrap(), "1000");
    assert_eq!(compiled.env.get("MIXER_THREADS").unwrap(), "2");

    // Semantic script markers, not byte equality: the legacy wrapper built
    // this argv in Rust; the plugin script emits the same official tokens.
    let script = compiled.script.as_deref().unwrap();
    assert!(script.contains("python /tools/mixer/precimed/mixer.py fit1"));
    assert!(script.contains("--bim-file /panels/mixer_g1000_eur/stage_flat/chr@.bim"));
    assert!(script.contains("--ld-file /panels/mixer_g1000_eur/ld_mixer/1000G.EUR.chr@"));
    assert!(script.contains("--extract /panels/mixer_g1000_eur/snps/g1000_eur_chr@.snps"));
    assert!(script.contains("--lib /tools/mixer/lib/libbgmg.so"));
    // Default run (fast_run = true) selects the fast sequence; the tokens
    // live in the FIT_SEQUENCE assignment and reach argv through the
    // unquoted `--fit-sequence $FIT_SEQUENCE` expansion (the plugin's
    // variable name; the legacy wrapper inlined the tokens in Rust).
    assert!(script.contains("--fit-sequence $FIT_SEQUENCE"));
    assert!(script.contains("FIT_SEQUENCE=\"diffevo-fast neldermead-fast\""));
    // Legacy variable name was out_prefix; the plugin keeps the same name
    // and the same `${AUTONOMICS_OUTPUT0%.json}` derivation.
    assert!(script.contains("out_prefix=\"${AUTONOMICS_OUTPUT0%.json}\""));
    assert!(script.contains("--trait1-file \"$AUTONOMICS_INPUT0\""));
    assert!(script.contains("--out \"$out_prefix\""));
    assert!(script.contains("--log \"$AUTONOMICS_OUTPUT1\""));
    // fit1 never carries the bivariate flags.
    assert!(!script.contains("--trait2-file"));
    assert!(!script.contains("--trait1-params-file"));
}

#[test]
fn mixer_fit2_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "mixer_fit2");

    let compiled =
        compile_container_spec(node, &manifest.image, &manifest.panels, &json!({})).unwrap();

    assert_eq!(
        compiled.image,
        "ghcr.io/auto-nomics/autonomics/mixer@sha256:3bd67cccf298bd3c9af3d2b013dd7dfacde9ad13d51bc78b2f7f1315f01bebb7"
    );
    assert_eq!(compiled.outputs.len(), 2);
    assert_eq!(compiled.outputs[0].path, "mixer_fit2.json");
    assert_eq!(
        compiled.outputs[0].format.as_deref(),
        Some("mixer_fit_json")
    );
    assert_eq!(compiled.outputs[1].path, "mixer_fit2.log");
    assert_eq!(compiled.outputs[1].format.as_deref(), Some("mixer_log"));
    assert_eq!(compiled.timeout_secs, 21_600);
    assert_eq!(compiled.artifact_prefix, "/artifacts/mixer_container");
    assert_eq!(compiled.panel_bundles.len(), 1);
    assert_eq!(
        compiled.panel_bundles[0].panel_id,
        "wjixiang/catalog-mixer-g1000-eur-rsid"
    );
    assert_eq!(compiled.command, vec!["sh".to_string()]);
    assert_eq!(compiled.cpus, Some(2.0));
    assert_eq!(compiled.memory.as_deref(), Some("16Gi"));
    assert_eq!(compiled.pids_limit, Some(512));

    let script = compiled.script.as_deref().unwrap();
    assert!(script.contains("python /tools/mixer/precimed/mixer.py fit2"));
    // Bivariate inputs in legacy order: trait1 sumstats, trait2 sumstats,
    // trait1 fit1 JSON, trait2 fit1 JSON.
    assert!(script.contains("--trait1-file \"$AUTONOMICS_INPUT0\""));
    assert!(script.contains("--trait2-file \"$AUTONOMICS_INPUT1\""));
    assert!(script.contains("--trait1-params-file \"$AUTONOMICS_INPUT2\""));
    assert!(script.contains("--trait2-params-file \"$AUTONOMICS_INPUT3\""));
    // Default (fast_run = true) sequence matches fit_sequence(true, true);
    // the slow branch carries the bivariate-only brute1/brent1 -fast steps.
    assert!(script.contains("--fit-sequence $FIT_SEQUENCE"));
    assert!(script.contains("FIT_SEQUENCE=\"diffevo-fast neldermead-fast\""));
    assert!(script.contains("brute1-fast brent1-fast"));
    assert_eq!(compiled.env.get("MIXER_FAST_RUN").unwrap(), "true");
}

#[test]
fn mixer_fit1_plugin_renders_submitted_values_into_env() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "mixer_fit1");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({
            "chr2use": "21-22",
            "seed": 42,
            "diffevo_fast_repeats": 5,
            "fast_run": false,
            "kmax_pdf": 20,
            "downsample_factor": 500,
            "threads": 4,
        }),
    )
    .unwrap();

    assert_eq!(compiled.env.get("MIXER_CHR2USE").unwrap(), "21-22");
    assert_eq!(compiled.env.get("MIXER_SEED").unwrap(), "42");
    assert_eq!(compiled.env.get("MIXER_DIFFEVO_FAST_REPEATS").unwrap(), "5");
    assert_eq!(compiled.env.get("MIXER_FAST_RUN").unwrap(), "false");
    assert_eq!(compiled.env.get("MIXER_KMAX_PDF").unwrap(), "20");
    assert_eq!(compiled.env.get("MIXER_DOWNSAMPLE_FACTOR").unwrap(), "500");
    assert_eq!(compiled.env.get("MIXER_THREADS").unwrap(), "4");
}
