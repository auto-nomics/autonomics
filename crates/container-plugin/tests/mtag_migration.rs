//! Golden parity tests for the mtag plugin: the `[[nodes]]` entry must
//! compile to the same `Container` spec the legacy Rust wrapper
//! (`nodes-io/src/mtag_container.rs`) produced. The plugin directory
//! lives outside this repository (`/mnt/projects/node-plugins/mtag` by
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
    let manifest = root.join("mtag").join("manifest.toml");
    if manifest.is_file() {
        return Some(root);
    }
    if explicit.is_some() {
        // fix-review R04 (2026-10-05): an explicitly set NODE_PLUGINS_ROOT
        // means migration parity was requested. A missing family must fail
        // loudly — early-returns here used to count as *passed* tests, so a
        // green summary claimed coverage that never ran.
        panic!(
            "NODE_PLUGINS_ROOT is set but the mtag family is not deployed \
             under it ({}) — migration parity cannot run. Deploy the family \
             or unset NODE_PLUGINS_ROOT to skip these tests deliberately.",
            manifest.display()
        );
    }
    eprintln!("skipping: mtag plugin directory not present (NODE_PLUGINS_ROOT unset)");
    None
}

fn load_manifest(root: &PathBuf) -> PluginManifest {
    let text = std::fs::read_to_string(root.join("mtag").join("manifest.toml")).unwrap();
    // Inline script_file the way the loader does.
    let mut manifest: PluginManifest = toml::from_str(&text).unwrap();
    for node in manifest.nodes.iter_mut() {
        if let Some(relative) = node.command.script_file.clone() {
            let source = std::fs::read_to_string(root.join("mtag").join(&relative)).unwrap();
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
fn mtag_plugin_compiles_to_the_legacy_wrapper_contract() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "mtag");

    let compiled =
        compile_container_spec(node, &manifest.image, &manifest.panels, &json!({})).unwrap();

    // Byte-exact contract with the legacy wrapper.
    assert_eq!(
        compiled.image,
        "ghcr.io/auto-nomics/autonomics/mtag@sha256:28ac0a0a0ee741340390b7588bf8ba36e6b316d62adc0cab7fd4494f5dd6a90c"
    );
    assert_eq!(compiled.outputs.len(), 3);
    assert_eq!(compiled.outputs[0].path, "mtag_trait_1.txt");
    assert_eq!(compiled.outputs[0].format.as_deref(), Some("mtag_results"));
    assert_eq!(compiled.outputs[1].path, "mtag_trait_2.txt");
    assert_eq!(compiled.outputs[1].format.as_deref(), Some("mtag_results"));
    assert_eq!(compiled.outputs[2].path, "mtag.log");
    assert_eq!(compiled.outputs[2].format.as_deref(), Some("mtag_log"));
    assert_eq!(compiled.network, "isolated");
    assert!(compiled.read_only_rootfs);
    assert!(matches!(
        compiled.pull_policy,
        container_runtime::PullPolicy::Missing
    ));
    // Legacy DEFAULT_TIMEOUT_SECS.
    assert_eq!(compiled.timeout_secs, 3600);
    // Deliberate delta: the legacy default was "/artifacts/mtag_container";
    // the plugin follows the kind rename (`_container` suffix dropped), the
    // same rule the ldsc/mrpresso family migrations applied.
    assert_eq!(compiled.artifact_prefix, "/artifacts/mtag");
    assert_eq!(compiled.workdir, None);
    assert!(compiled.panels.is_empty());
    assert!(compiled.files.is_empty());
    assert_eq!(compiled.command, vec!["sh".to_string()]);

    // The single LD panel: catalog id and mount must match the legacy
    // panel-bundle spec byte for byte.
    assert_eq!(compiled.panel_bundles.len(), 1);
    assert_eq!(
        compiled.panel_bundles[0].panel_id,
        "wjixiang/catalog-mtag-ld-ref-1000g-eur-w-ld"
    );
    assert_eq!(compiled.panel_bundles[0].mount_path, "/panels/ld_ref");

    // Defaults render through the env channel (the legacy spec built its
    // env as an empty map and interpolated params straight into the script;
    // the deliberate plugin delta routes every param through env). Numbers
    // carry the serde_json spelling: f64 1.0 renders "1.0" where the
    // legacy `format!` produced "1", and 1e-6 renders "1e-6" where it
    // produced "0.000001" — equal after float parsing (documented
    // pitfall 8).
    assert_eq!(compiled.env.len(), 8);
    assert_eq!(compiled.env.get("MTAG_TIME_LIMIT_HOURS").unwrap(), "1.0");
    assert_eq!(compiled.env.get("MTAG_TOL").unwrap(), "1e-6");
    // Optional-and-absent bools render as empty strings, so the script
    // `[ -n … ]` guards omit the official flags exactly like the legacy
    // `if spec.force { push("--force") }` building.
    for name in [
        "MTAG_FORCE",
        "MTAG_NO_OVERLAP",
        "MTAG_PERFECT_GENCOV",
        "MTAG_EQUAL_H2",
        "MTAG_STD_BETAS",
        "MTAG_NUMERICAL_OMEGA",
    ] {
        assert_eq!(compiled.env.get(name).unwrap(), "", "{name}");
    }

    // Script parity is semantic, not byte-exact: the plugin drives flags
    // through env + `[ -n … ]` guards instead of Rust string building.
    let script = compiled.script.as_deref().unwrap();
    // Both sumstats inputs feed the single official --sumstats token, in
    // order, byte-identical to the legacy invocation.
    assert!(script.contains("--sumstats \"$AUTONOMICS_INPUT0\",\"$AUTONOMICS_INPUT1\""));
    // Official MTAG CLI tokens are identical to the legacy script.
    assert!(script.contains("--snp_name snpid"));
    assert!(script.contains("--z_name z"));
    assert!(script.contains("--n_name n"));
    assert!(script.contains("--eaf_name freq"));
    assert!(script.contains("--chr_name chr"));
    assert!(script.contains("--bpos_name bpos"));
    assert!(script.contains("--a1_name a1"));
    assert!(script.contains("--a2_name a2"));
    // The LD panel mount: trailing slash preserved from the legacy script.
    assert!(script.contains("--ld_ref_panel /panels/ld_ref/"));
    assert!(script.contains("--out \"$AUTONOMICS_WORKDIR/mtag\""));
    assert!(script.contains("--make_full_path"));
    // Always-on optimizer settings, in the legacy flag position.
    assert!(script.contains("--time_limit \"$MTAG_TIME_LIMIT_HOURS\""));
    assert!(script.contains("--tol \"$MTAG_TOL\""));
    // One `[ -n … ]` guard per optional official flag.
    assert!(script.contains("[ -n \"$MTAG_FORCE\" ]"));
    assert!(script.contains("[ -n \"$MTAG_NO_OVERLAP\" ]"));
    assert!(script.contains("[ -n \"$MTAG_PERFECT_GENCOV\" ]"));
    assert!(script.contains("[ -n \"$MTAG_EQUAL_H2\" ]"));
    assert!(script.contains("[ -n \"$MTAG_STD_BETAS\" ]"));
    assert!(script.contains("[ -n \"$MTAG_NUMERICAL_OMEGA\" ]"));
    assert!(script.contains("EXTRA=\"$EXTRA --force\""));
    assert!(script.contains("EXTRA=\"$EXTRA --no_overlap\""));
    assert!(script.contains("EXTRA=\"$EXTRA --perfect_gencov\""));
    assert!(script.contains("EXTRA=\"$EXTRA --equal_h2\""));
    assert!(script.contains("EXTRA=\"$EXTRA --std_betas\""));
    assert!(script.contains("EXTRA=\"$EXTRA --numerical_omega\""));
    // Parity with the legacy wrapper, unlike ldsc: the legacy mtag script
    // passed both inputs straight through (no decompress_gzip_inputs
    // helper), so the plugin script must not grow a gzip case stanza.
    assert!(!script.contains("gzip"));
}

#[test]
fn mtag_plugin_renders_submitted_values_into_env() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "mtag");

    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({
            "time_limit_hours": 2.5,
            "tol": 0.001,
            "force": true,
            "perfect_gencov": true,
            "equal_h2": true,
            "numerical_omega": true
        }),
    )
    .unwrap();

    // Submitted numbers render with the serde_json spelling (2.5 → "2.5").
    assert_eq!(compiled.env.get("MTAG_TIME_LIMIT_HOURS").unwrap(), "2.5");
    assert_eq!(compiled.env.get("MTAG_TOL").unwrap(), "0.001");
    // Submitted-true booleans render "true", which is non-empty, so the
    // script guards append the official flags — the plugin equivalent of
    // the legacy `--force --perfect_gencov --equal_h2 --numerical_omega`.
    assert_eq!(compiled.env.get("MTAG_FORCE").unwrap(), "true");
    assert_eq!(compiled.env.get("MTAG_PERFECT_GENCOV").unwrap(), "true");
    assert_eq!(compiled.env.get("MTAG_EQUAL_H2").unwrap(), "true");
    assert_eq!(compiled.env.get("MTAG_NUMERICAL_OMEGA").unwrap(), "true");
    // Flags left out stay empty: they are absent from the submission and
    // the params are optional, so they resolve to null.
    assert_eq!(compiled.env.get("MTAG_NO_OVERLAP").unwrap(), "");
    assert_eq!(compiled.env.get("MTAG_STD_BETAS").unwrap(), "");
}

#[test]
fn mtag_plugin_rejects_equal_h2_without_perfect_gencov() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "mtag");

    // The legacy validate() cross-field rule (`equal_h2 requires
    // perfect_gencov`) is expressed in the manifest DSL itself via
    // `requires = ["perfect_gencov"]` and enforced at compile time — the
    // first family to use the gate, so no script-side guard is needed.
    let error = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({"equal_h2": true}),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("requires `perfect_gencov`"),
        "unexpected error: {error}"
    );

    // Positive control: the satisfied gate compiles.
    let compiled = compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &json!({"equal_h2": true, "perfect_gencov": true}),
    )
    .unwrap();
    assert_eq!(compiled.env.get("MTAG_EQUAL_H2").unwrap(), "true");
}

#[test]
fn mtag_plugin_schema_marks_optionals_not_required() {
    let Some(root) = plugin_root() else {
        eprintln!("skipping: plugin directory not present");
        return;
    };
    let manifest = load_manifest(&root);
    let node = node_by_kind(&manifest, "mtag");

    let schema =
        serde_json::to_value(container_plugin::compile::compile_schema(&node.params)).unwrap();
    // Every legacy spec field had a default or was serde-defaulted, so
    // nothing is required.
    let required = schema["required"].as_array().unwrap();
    assert!(required.is_empty());
    let time_limit = &schema["properties"]["time_limit_hours"];
    assert_eq!(time_limit["type"], "number");
    assert_eq!(time_limit["default"], 1.0);
    // validate(): finite and greater than zero → exclusive minimum.
    assert_eq!(time_limit["exclusiveMinimum"], 0.0);
    let tol = &schema["properties"]["tol"];
    assert_eq!(tol["type"], "number");
    assert_eq!(tol["default"], 1e-6);
    assert_eq!(tol["exclusiveMinimum"], 0.0);
    for name in [
        "force",
        "no_overlap",
        "perfect_gencov",
        "equal_h2",
        "std_betas",
        "numerical_omega",
    ] {
        let flag = &schema["properties"][name];
        assert_eq!(flag["type"], "boolean", "{name}");
        assert!(flag.get("default").is_none(), "{name}");
    }
}
