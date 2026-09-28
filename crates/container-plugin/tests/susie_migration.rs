//! Golden parity tests for the susie plugin: the `[[nodes]]` entry must
//! compile to the same `Container` spec the legacy Rust wrapper
//! (`crates/node-bundles/nodes-io/src/susie_rss_container.rs`) produced.
//! The plugin directory lives outside this repository
//! (`/mnt/projects/node-plugins/susie` by default, overridable via
//! `NODE_PLUGINS_ROOT`); the test is skipped when absent so CI without
//! the plugin checkout stays green.

use std::path::PathBuf;

use container_plugin::compile::spec_compile::compile_container_spec;
use container_plugin::manifest::PluginManifest;
use serde_json::json;

fn plugin_root() -> Option<PathBuf> {
    let root = std::env::var_os("NODE_PLUGINS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/projects/node-plugins"));
    let manifest = root.join("susie").join("manifest.toml");
    manifest.is_file().then_some(root)
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let text = std::fs::read_to_string(root.join("susie").join("manifest.toml")).unwrap();
    // Inline script_file the way the loader does.
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        if let Some(relative) = node.command.script_file.clone() {
            let source = std::fs::read_to_string(root.join("susie").join(&relative)).unwrap();
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
fn susie_rss_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "susie_rss");

    let compiled =
        compile_container_spec(node, &manifest.image, &manifest.panels, &json!({})).unwrap();

    assert_eq!(
        compiled.image,
        "ghcr.io/auto-nomics/autonomics/susie@sha256:8a72a443461add5c94c4907f9a1d6106b989850e93217a95587d9095542febe8"
    );
    assert_eq!(compiled.outputs.len(), 3);
    assert_eq!(compiled.outputs[0].path, "susie_rss.tsv");
    assert_eq!(compiled.outputs[0].format.as_deref(), Some("susie_rss"));
    assert_eq!(compiled.outputs[1].path, "susie_rss.RDS");
    assert_eq!(compiled.outputs[1].format.as_deref(), Some("r_rds"));
    assert_eq!(compiled.outputs[2].path, "susie_rss.log");
    assert_eq!(compiled.outputs[2].format.as_deref(), Some("susie_rss_log"));
    // The legacy wrapper built `["Rscript", "/opt/susie/bin/run_susie_rss.R"]`
    // with no inline script; the plugin owns the runner and stages it, so the
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
    assert_eq!(compiled.timeout_secs, 1800);
    // The legacy DEFAULT_ARTIFACT_PREFIX is kept verbatim; the manifest-level
    // default would have derived `/artifacts/susie_rss` (kind minus the
    // `_container` suffix).
    assert_eq!(compiled.artifact_prefix, "/artifacts/susie_rss_container");
    assert_eq!(compiled.workdir, None);
    assert!(compiled.panels.is_empty());
    // The signed-LD panel binding: the legacy wrapper mounted
    // SUSIE_REF_PANEL at /panels/mixer_ref and the runner hard-reads the
    // stage_flat/ld_mixer paths under it.
    assert_eq!(compiled.panel_bundles.len(), 1);
    assert_eq!(
        compiled.panel_bundles[0].panel_id,
        "wjixiang/catalog-mixer-g1000-eur"
    );
    assert_eq!(compiled.panel_bundles[0].mount_path, "/panels/mixer_ref");
    // The legacy spec left every resource override unset; the manifest omits
    // [nodes.resources], so the hardened defaults apply unchanged.
    assert_eq!(compiled.cpus, None);
    assert_eq!(compiled.memory, None);
    assert_eq!(compiled.pids_limit, None);
    assert_eq!(compiled.shm_size, None);
    assert_eq!(compiled.gpus, None);
    assert_eq!(compiled.user, None);

    // Env defaults, the legacy container_spec() contract. serde_json renders
    // the f64 zeros with a trailing `.0` (`0.0`) where the legacy
    // `f64::to_string` produced `"0"`; values are equal after R's
    // as.numeric() (documented pitfall 8). SUSIE_N renders as the empty
    // string where the legacy wrapper omitted the key entirely; the runner's
    // `nzchar(Sys.getenv("SUSIE_N", unset = ""))` treats both as "read the n
    // column".
    assert_eq!(compiled.env.get("SUSIE_L").unwrap(), "10");
    assert_eq!(
        compiled.env.get("SUSIE_ESTIMATE_PRIOR_METHOD").unwrap(),
        "optim"
    );
    assert_eq!(
        compiled
            .env
            .get("SUSIE_ESTIMATE_RESIDUAL_VARIANCE")
            .unwrap(),
        "false"
    );
    assert_eq!(
        compiled.env.get("SUSIE_ESTIMATE_PRIOR_VARIANCE").unwrap(),
        "true"
    );
    assert_eq!(compiled.env.get("SUSIE_COVERAGE").unwrap(), "0.95");
    assert_eq!(compiled.env.get("SUSIE_MIN_ABS_CORR").unwrap(), "0.5");
    assert_eq!(
        compiled.env.get("SUSIE_SCALED_PRIOR_VARIANCE").unwrap(),
        "0.2"
    );
    assert_eq!(compiled.env.get("SUSIE_Z_METHOD").unwrap(), "wald");
    assert_eq!(compiled.env.get("SUSIE_R2_MIN").unwrap(), "0.0");
    assert_eq!(
        compiled.env.get("SUSIE_CHECK_NULL_THRESHOLD").unwrap(),
        "0.0"
    );
    assert_eq!(compiled.env.get("SUSIE_MAX_ITER").unwrap(), "100");
    assert_eq!(compiled.env.get("SUSIE_N").unwrap(), "");

    // Semantic script markers, not byte equality: the plugin runner is the
    // legacy image-baked run_susie_rss.R adapted, so the susie_rss() call
    // and the signed-LD panel mount paths are the load-bearing tokens.
    let script = compiled.script.as_deref().unwrap();
    assert!(script.contains("susieR::susie_rss("));
    assert!(script.contains("min_abs_corr = min_abs_corr"));
    assert!(script.contains("coverage = coverage"));
    assert!(script.contains("/panels/mixer_ref/stage_flat/chr@.bim"));
    assert!(script.contains("/panels/mixer_ref/ld_mixer/1000G.EUR.chr@"));
}

#[test]
fn susie_rss_plugin_runner_keeps_the_legacy_call_semantics() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "susie_rss");

    let compiled =
        compile_container_spec(node, &manifest.image, &manifest.panels, &json!({})).unwrap();

    // The full susie_rss(...) argument list is token-identical to the baked
    // runner: same parameters, same order, same env-var sources.
    let script = compiled.script.as_deref().unwrap();
    assert!(script.contains("z = z,"));
    assert!(script.contains("R = r_matrix,"));
    assert!(script.contains("n = n_value,"));
    assert!(script.contains("L = l,"));
    assert!(script.contains("scaled_prior_variance = scaled_prior_variance,"));
    assert!(script.contains("estimate_residual_variance = estimate_residual_variance,"));
    assert!(script.contains("estimate_prior_variance = estimate_prior_variance,"));
    assert!(script.contains("estimate_prior_method = estimate_prior_method,"));
    assert!(script.contains("z_method = z_method,"));
    assert!(script.contains("check_null_threshold = check_null_threshold,"));
    assert!(script.contains("max_iter = max_iter"));

    // The LD query helper stays image-baked; the runner still invokes it with
    // the legacy --bim-file/--ld-file panel paths.
    assert!(script.contains("/opt/susie/bin/susie_ld_query.py"));
    assert!(script.contains("--engine-home"));
    assert!(script.contains("/opt/mixer/lib/libbgmg.so"));
    assert!(script.contains("--r2-min"));

    // The AUTONOMICS_* channel: one input, three outputs, and the staged
    // query/pairs scratch files under AUTONOMICS_WORKDIR.
    assert!(script.contains("Sys.getenv(\"AUTONOMICS_INPUT0\")"));
    assert!(script.contains("Sys.getenv(\"AUTONOMICS_OUTPUT0\")"));
    assert!(script.contains("Sys.getenv(\"AUTONOMICS_OUTPUT1\")"));
    assert!(script.contains("Sys.getenv(\"AUTONOMICS_OUTPUT2\")"));
    assert!(script.contains("Sys.getenv(\"AUTONOMICS_WORKDIR\")"));
    assert!(script.contains("saveRDS(fit, Sys.getenv(\"AUTONOMICS_OUTPUT1\"))"));

    // The legacy validate() membership rules the v0 DSL cannot express as
    // bounds are enforced by the runner with the byte-identical error
    // messages.
    assert!(script.contains("estimate_prior_method must be one of: optim, EM, simple"));
    assert!(script.contains("z_method must be one of: wald, score"));
}

#[test]
fn susie_rss_plugin_renders_submitted_values_into_env() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "susie_rss");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({
            "l": 4,
            "max_iter": 500,
            "n": 987,
            "estimate_prior_method": "EM",
            "estimate_residual_variance": true,
            "z_method": "score",
            "coverage": 0.9,
            "r2_min": 0.5
        }),
    )
    .unwrap();

    // Integer 4 renders "4", matching the legacy usize 4.to_string().
    assert_eq!(compiled.env.get("SUSIE_L").unwrap(), "4");
    assert_eq!(compiled.env.get("SUSIE_MAX_ITER").unwrap(), "500");
    assert_eq!(compiled.env.get("SUSIE_N").unwrap(), "987");
    assert_eq!(
        compiled.env.get("SUSIE_ESTIMATE_PRIOR_METHOD").unwrap(),
        "EM"
    );
    assert_eq!(
        compiled
            .env
            .get("SUSIE_ESTIMATE_RESIDUAL_VARIANCE")
            .unwrap(),
        "true"
    );
    assert_eq!(compiled.env.get("SUSIE_Z_METHOD").unwrap(), "score");
    assert_eq!(compiled.env.get("SUSIE_COVERAGE").unwrap(), "0.9");
    assert_eq!(compiled.env.get("SUSIE_R2_MIN").unwrap(), "0.5");

    // A second L variation: the minimal single-effect fit, with n left
    // absent so SUSIE_N renders empty ("read the n column").
    let compiled =
        compile_container_spec(node, &manifest.image, &manifest.panels, &json!({"l": 1})).unwrap();
    assert_eq!(compiled.env.get("SUSIE_L").unwrap(), "1");
    assert_eq!(compiled.env.get("SUSIE_N").unwrap(), "");
}

#[test]
fn susie_rss_plugin_rejects_values_the_legacy_validate_rejected() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "susie_rss");

    // The numeric/integer rules of SusieRssContainerSpec::validate are
    // expressed as manifest bounds and fail at compile time, exactly where
    // the legacy wrapper failed at registry build.
    for broken in [
        json!({"l": 0}),
        json!({"coverage": 1.5}),
        json!({"min_abs_corr": -0.1}),
        json!({"r2_min": -0.1}),
        json!({"scaled_prior_variance": 0.0}),
        json!({"max_iter": 0}),
    ] {
        assert!(
            compile_container_spec(node, &manifest.image, &manifest.panels, &broken).is_err(),
            "expected {broken} to be rejected"
        );
    }

    // The string-membership rules moved into the runner (v0 has no enum
    // param type): a bad method compiles and fails at container start with
    // the legacy message instead of at registry build.
    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({"estimate_prior_method": "unknown"}),
    )
    .unwrap();
    assert_eq!(
        compiled.env.get("SUSIE_ESTIMATE_PRIOR_METHOD").unwrap(),
        "unknown"
    );
}

#[test]
fn susie_rss_plugin_schema_marks_optionals_and_bounds() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "susie_rss");

    let schema =
        serde_json::to_value(container_plugin::compile::compile_schema(&node.params)).unwrap();
    // Every param has a default or is optional (n): nothing is required.
    let required = schema["required"].as_array().unwrap();
    assert!(required.is_empty());
    assert_eq!(schema["properties"]["l"]["type"], "integer");
    assert_eq!(schema["properties"]["l"]["default"], 10);
    assert_eq!(schema["properties"]["l"]["minimum"], 1.0);
    assert_eq!(schema["properties"]["coverage"]["type"], "number");
    assert_eq!(schema["properties"]["coverage"]["default"], 0.95);
    assert_eq!(schema["properties"]["coverage"]["minimum"], 0.0);
    assert_eq!(schema["properties"]["coverage"]["maximum"], 1.0);
    assert_eq!(
        schema["properties"]["scaled_prior_variance"]["exclusiveMinimum"],
        0.0
    );
    assert_eq!(schema["properties"]["n"]["type"], "number");
    assert!(schema["properties"]["n"].get("default").is_none());
    assert_eq!(
        schema["properties"]["estimate_residual_variance"]["type"],
        "boolean"
    );
}

#[test]
fn susie_rss_plugin_keeps_the_three_output_port_layout() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "susie_rss");

    // One File input, three File outputs (tsv / RDS / log): the plugin
    // layout must match the legacy port_layout() exactly.
    let ports = container_plugin::node_definition::compile_ports(&node.ports);
    assert_eq!(ports.input_ports().len(), 1);
    assert_eq!(ports.output_ports().len(), 3);
}

#[test]
fn susie_rss_plugin_doc_covers_operational_blind_spots() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "susie_rss");

    // Mirrors the legacy wrapper's factory_doc test over FACTORY_DOC: the
    // agent-facing doc must keep the operational warnings.
    for required in [
        "chr:pos:allele1:allele2",
        "rsIDs do not align",
        "format=\"tsv\"",
        "cs=k>=1",
        ".snps extraction lists",
        "timeout_secs=1800",
    ] {
        assert!(
            node.doc.contains(required),
            "manifest doc is missing `{required}`"
        );
    }
}
