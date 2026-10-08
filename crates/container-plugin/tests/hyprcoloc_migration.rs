//! Golden parity test for the hyprcoloc plugin: the `[[nodes]]` entry must
//! compile to the same `Container` spec the legacy Rust wrapper
//! (`nodes-io/src/hyprcoloc_container.rs`) produced. The plugin directory
//! lives outside this repository (`/mnt/projects/node-plugins/hyprcoloc` by
//! default, overridable via `NODE_PLUGINS_ROOT`); the test is skipped when
//! absent so CI without the plugin checkout stays green.
//!
//! The legacy wrapper generated the R script in Rust (a string template with
//! conditional optional arguments); the plugin ships the equivalent as
//! `scripts/hyprcoloc_runner.R`, so parity hangs on the byte-exact command
//! (`["Rscript"]` + script), the semantic script markers, and the
//! `HYPRCOLOC_*` env channel. The legacy spec carried no env at all — every
//! parameter was baked into the generated script — so the env channel here
//! is the plugin's replacement for the template substitution, and the
//! validation the manifest DSL cannot express lives in the script.

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
    let manifest = root.join("hyprcoloc").join("manifest.toml");
    if manifest.is_file() {
        return Some(root);
    }
    if explicit.is_some() {
        // fix-review R04 (2026-10-05): an explicitly set NODE_PLUGINS_ROOT
        // means migration parity was requested. A missing family must fail
        // loudly — early-returns here used to count as *passed* tests, so a
        // green summary claimed coverage that never ran.
        panic!(
            "NODE_PLUGINS_ROOT is set but the hyprcoloc family is not deployed \
             under it ({}) — migration parity cannot run. Deploy the family \
             or unset NODE_PLUGINS_ROOT to skip these tests deliberately.",
            manifest.display()
        );
    }
    eprintln!("skipping: hyprcoloc plugin directory not present (NODE_PLUGINS_ROOT unset)");
    None
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let text = std::fs::read_to_string(root.join("hyprcoloc").join("manifest.toml")).unwrap();
    // Inline script_file the way the loader does.
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        if let Some(relative) = node.command.script_file.clone() {
            let source = std::fs::read_to_string(root.join("hyprcoloc").join(&relative)).unwrap();
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

/// Two traits matching the legacy wrapper test fixture: one continuous, one
/// binary. The legacy `Vec<TraitSpec>` travels as the encoded
/// `name,beta,se,binary` rows the script parses.
fn required_params() -> serde_json::Value {
    json!({
        "snp_column": "snp",
        "traits": "trait1,beta1,se1,0;trait2,beta2,se2,1",
    })
}

#[test]
fn hyprcoloc_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "hyprcoloc");

    let compiled =
        compile_container_spec(node, &manifest.image, &manifest.panels, &required_params())
            .unwrap();

    // Image unchanged from the legacy wrapper: same digest-pinned reference.
    assert_eq!(
        compiled.image,
        "ghcr.io/auto-nomics/autonomics/hyprcoloc@sha256:395a36903d3c3ed6c753aef85f5b0a57044f86968e4d79804e4cc85f36aa5617"
    );
    // Byte-exact command: bare interpreter, script carried alongside — the
    // legacy `command: ["Rscript"]` + generated script shape.
    assert_eq!(compiled.command, vec!["Rscript".to_string()]);
    let script = compiled
        .script
        .as_deref()
        .expect("hyprcoloc ships a script");

    // Semantic script markers — the same tokens the legacy wrapper's test
    // asserted on its generated template, plus the do.call splicing the
    // optional official arguments.
    assert!(script.contains("do.call(hyprcoloc::hyprcoloc, args)"));
    assert!(script.contains("effect.est = effect_est"));
    assert!(script.contains("effect.se = effect_se"));
    assert!(script.contains("binary.outcomes = binary_vector"));
    assert!(script.contains("trait.names = trait_names"));
    assert!(script.contains("snp.id = snp_id"));
    assert!(script.contains("args$trait.subset <- trait_subset"));
    assert!(script.contains("args$reg.thresh <- reg_thresh"));
    assert!(script.contains("args$align.thresh <- align_thresh"));
    assert!(script.contains("args$prior.12 <- prior_12"));
    assert!(script.contains("write.table"));
    assert!(script.contains("saveRDS"));

    assert_eq!(compiled.outputs.len(), 3);
    assert_eq!(compiled.outputs[0].path, "hyprcoloc_results.tsv");
    assert_eq!(
        compiled.outputs[0].format.as_deref(),
        Some("hyprcoloc_results_tsv")
    );
    assert_eq!(compiled.outputs[1].path, "hyprcoloc.RDS");
    assert_eq!(compiled.outputs[1].format.as_deref(), Some("r_rds"));
    assert_eq!(compiled.outputs[2].path, "hyprcoloc.log");
    assert_eq!(compiled.outputs[2].format.as_deref(), Some("hyprcoloc_log"));

    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert!(matches!(
        compiled.pull_policy,
        container_runtime::PullPolicy::Missing
    ));
    assert_eq!(compiled.timeout_secs, 900);
    // Deliberate delta: the legacy default was "/artifacts/hyprcoloc_container";
    // the plugin drops the `_container` suffix together with the kind rename
    // (the same rule the ldsc/deseq2 migrations applied).
    assert_eq!(
        compiled.artifact_prefix,
        Some("/artifacts/hyprcoloc".to_string())
    );
    assert_eq!(compiled.workdir, None);
    // The legacy wrapper bound no panels and left every resource knob unset.
    assert!(compiled.panels.is_empty());
    assert!(compiled.panel_bundles.is_empty());
    assert_eq!(compiled.cpus, None);
    assert_eq!(compiled.memory, None);
    assert_eq!(compiled.pids_limit, None);
    assert_eq!(compiled.shm_size, None);

    // Defaults render the legacy serde defaults (1e-4 → "0.0001", matching
    // the legacy template literal).
    assert_eq!(
        compiled.env.get("HYPRCOLOC_SNP_COLUMN").map(String::as_str),
        Some("snp")
    );
    assert_eq!(
        compiled.env.get("HYPRCOLOC_TRAITS").map(String::as_str),
        Some("trait1,beta1,se1,0;trait2,beta2,se2,1")
    );
    assert_eq!(
        compiled
            .env
            .get("HYPRCOLOC_TRAIT_SUBSET")
            .map(String::as_str),
        Some("")
    );
    assert_eq!(
        compiled.env.get("HYPRCOLOC_PRIOR_1").map(String::as_str),
        Some("0.0001")
    );
    assert_eq!(
        compiled.env.get("HYPRCOLOC_PRIOR_C").map(String::as_str),
        Some("0.02")
    );
    assert_eq!(
        compiled
            .env
            .get("HYPRCOLOC_UNIFORM_PRIORS")
            .map(String::as_str),
        Some("false")
    );
    assert_eq!(
        compiled.env.get("HYPRCOLOC_BB_ALG").map(String::as_str),
        Some("true")
    );
    assert_eq!(
        compiled
            .env
            .get("HYPRCOLOC_BB_SELECTION")
            .map(String::as_str),
        Some("regional")
    );
    assert_eq!(
        compiled.env.get("HYPRCOLOC_REG_STEPS").map(String::as_str),
        Some("1")
    );
    assert_eq!(
        compiled.env.get("HYPRCOLOC_SNPSCORES").map(String::as_str),
        Some("false")
    );

    // Optional numeric params render as the empty string the script's
    // optional_number folds into NULL (omit the official argument).
    assert_eq!(
        compiled.env.get("HYPRCOLOC_PRIOR_12").map(String::as_str),
        Some("")
    );
    assert_eq!(
        compiled.env.get("HYPRCOLOC_REG_THRESH").map(String::as_str),
        Some("")
    );
    assert_eq!(
        compiled
            .env
            .get("HYPRCOLOC_ALIGN_THRESH")
            .map(String::as_str),
        Some("")
    );

    // Fully-populated spec: optionals travel with their values, bools keep
    // the serde spelling the script's to_bool accepts.
    let full = json!({
        "snp_column": "snp",
        "traits": "trait1,beta1,se1,0;trait2,beta2,se2,1",
        "trait_subset": "trait2",
        "prior_1": 0.001,
        "prior_12": 0.05,
        "reg_thresh": 0.6,
        "align_thresh": 0.7,
        "uniform_priors": true,
        "bb_selection": "alignment",
        "reg_steps": 2,
        "snpscores": true,
    });
    let compiled_full =
        compile_container_spec(node, &manifest.image, &manifest.panels, &full).unwrap();
    assert_eq!(
        compiled_full
            .env
            .get("HYPRCOLOC_TRAIT_SUBSET")
            .map(String::as_str),
        Some("trait2")
    );
    assert_eq!(
        compiled_full
            .env
            .get("HYPRCOLOC_PRIOR_1")
            .map(String::as_str),
        Some("0.001")
    );
    assert_eq!(
        compiled_full
            .env
            .get("HYPRCOLOC_PRIOR_12")
            .map(String::as_str),
        Some("0.05")
    );
    assert_eq!(
        compiled_full
            .env
            .get("HYPRCOLOC_REG_THRESH")
            .map(String::as_str),
        Some("0.6")
    );
    assert_eq!(
        compiled_full
            .env
            .get("HYPRCOLOC_ALIGN_THRESH")
            .map(String::as_str),
        Some("0.7")
    );
    assert_eq!(
        compiled_full
            .env
            .get("HYPRCOLOC_UNIFORM_PRIORS")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        compiled_full
            .env
            .get("HYPRCOLOC_BB_SELECTION")
            .map(String::as_str),
        Some("alignment")
    );
    assert_eq!(
        compiled_full
            .env
            .get("HYPRCOLOC_REG_STEPS")
            .map(String::as_str),
        Some("2")
    );
    assert_eq!(
        compiled_full
            .env
            .get("HYPRCOLOC_SNPSCORES")
            .map(String::as_str),
        Some("true")
    );
}

/// The shipped script must keep the validation the manifest DSL cannot
/// express — the legacy validate() rules move here with the same error
/// semantics (failure point shifts from registry build to container start).
#[test]
fn runner_script_keeps_the_legacy_validation_markers() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let script =
        std::fs::read_to_string(root.join("hyprcoloc").join("scripts/hyprcoloc_runner.R")).unwrap();
    assert!(script.contains("hyprcoloc requires at least two traits"));
    assert!(script.contains("trait name cannot be empty"));
    assert!(script.contains("trait names must be unique"));
    assert!(script.contains("snp, beta, and se columns must be unique"));
    assert!(script.contains("unknown trait_subset entry"));
    assert!(script.contains("must be in (0, 1]"));
    assert!(script.contains("bb_alg thresholds must be at least"));
    assert!(script.contains("bb_selection must be regional or alignment"));
    assert!(script.contains("reg_steps must be greater than zero"));
}
