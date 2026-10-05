//! Unit-level proof that the manifest bool rendering round-trips through
//! the real consumer idiom of the MVMR plugin script: R's `as.logical()`.
//!
//! F02 regression: `Value::Bool(false)` used to render as "" on the env
//! surface, and `as.logical("")` is **NA** in R — the MVMR default spec
//! (`qhet = false`) fed NA into the qhet branch gate and blocked the
//! default run. The plugin contract (mvmr.R header: "booleans render as
//! 'true'/'false' and convert with as.logical()") is value semantics.
//!
//! The test compiles an mvmr-shaped node through the real pipeline
//! (manifest → resolve → render → env), asserts the rendered values, then
//! feeds them to an R snippet mirroring mvmr.R's parse chain (lines
//! 24-27) and requires every flag to parse to the intended logical.
//!
//! The R part skips when `Rscript` is not on PATH; the value assertions
//! run everywhere.

use container_plugin::compile::spec_compile::compile_container_spec;
use container_plugin::manifest::PluginManifest;
use serde_json::json;

/// The mvmr node shape: the four booleans carry the exact defaults of the
/// deployed family (`qhet` defaults to false — the F02 blocking case) and
/// flow through MVMR_* env names, exactly as the family manifest declares
/// them.
const MVMR_SHAPED_NODE: &str = r#"
kind = "mvmr"
desc = "MVMR shape"
doc = "MVMR shape"
timeout_secs = 900

[nodes.ports]
outputs = [
    { path = "mvmr.RDS", format = "r_rds" },
    { path = "mvmr.log", format = "mvmr_log" },
]

[nodes.params]
beta_yg = { type = "string" }
sebeta_yg = { type = "string" }
beta_xg = { type = "string_array" }
sebeta_xg = { type = "string_array" }
strength = { type = "bool", default = true, doc = "Include strength_mvmr results" }
strhet = { type = "bool", default = true, doc = "Include strhet_mvmr results" }
pleiotropy = { type = "bool", default = true, doc = "Include pleiotropy_mvmr results" }
qhet = { type = "bool", default = false, doc = "Include qhet_mvmr results; requires pcor" }
pcor = { type = "string", optional = true, doc = "Optional correlation matrix" }

[nodes.command]
interpreter = "Rscript"
script = "x <- 1"

[nodes.command.env]
MVMR_BETA_YG = "{{ beta_yg }}"
MVMR_SEBETA_YG = "{{ sebeta_yg }}"
MVMR_BETA_XG = "{{ beta_xg }}"
MVMR_SEBETA_XG = "{{ sebeta_xg }}"
MVMR_STRENGTH = "{{ strength }}"
MVMR_STRHET = "{{ strhet }}"
MVMR_PLEIOTROPY = "{{ pleiotropy }}"
MVMR_QHET = "{{ qhet }}"
MVMR_PCOR = "{{ pcor }}"
"#;

/// The four params the family marks required (mirrors mvmr_migration).
fn required_columns() -> serde_json::Value {
    json!({
        "beta_yg": "SBP_beta",
        "sebeta_yg": "SBP_se",
        "beta_xg": ["LDL_beta", "HDL_beta"],
        "sebeta_xg": ["LDL_se", "HDL_se"],
    })
}

fn compiled_env(submitted: serde_json::Value) -> std::collections::BTreeMap<String, String> {
    let manifest: PluginManifest = toml::from_str(&format!(
        "schema_version = 1\nplugin_name = \"mvmr_test\"\n\n[image]\nreference = \
         \"ghcr.io/auto-nomics/autonomics/mvmr@sha256:\
         ce9ad3f46cf8b74a95c194fe791b6a529d9168676c5d936a05ba440569260eb3\"\n\n[[nodes]]\n\
         {MVMR_SHAPED_NODE}"
    ))
    .unwrap();
    let node = &manifest.nodes[0];
    container_plugin::node_definition::validate(node).unwrap();
    compile_container_spec(node, &manifest.image, &manifest.panels, &submitted)
        .unwrap()
        .env
}

#[test]
fn mvmr_shaped_bools_render_value_semantics() {
    let env = compiled_env(required_columns());
    assert_eq!(env.get("MVMR_STRENGTH").unwrap(), "true");
    assert_eq!(env.get("MVMR_STRHET").unwrap(), "true");
    assert_eq!(env.get("MVMR_PLEIOTROPY").unwrap(), "true");
    // The F02 case: qhet defaults to false and must render the literal —
    // R's as.logical("false") is FALSE while as.logical("") is NA.
    assert_eq!(env.get("MVMR_QHET").unwrap(), "false");
    // Optional-and-absent keeps the presence channel (empty string).
    assert_eq!(env.get("MVMR_PCOR").unwrap(), "");

    // Explicit submissions render their value, both directions.
    let submitted = json!({
        "beta_yg": "SBP_beta",
        "sebeta_yg": "SBP_se",
        "beta_xg": ["LDL_beta", "HDL_beta"],
        "sebeta_xg": ["LDL_se", "HDL_se"],
        "qhet": true,
        "strength": false,
    });
    let env = compiled_env(submitted);
    assert_eq!(env.get("MVMR_QHET").unwrap(), "true");
    assert_eq!(env.get("MVMR_STRENGTH").unwrap(), "false");
}

#[test]
fn rendered_bool_env_values_parse_in_r_as_logical() {
    let Some(rscript) = which_rscript() else {
        eprintln!("skipping: Rscript is not on PATH");
        return;
    };
    let env = compiled_env(required_columns());

    // Mirror of the mvmr.R parse chain: as.logical(Sys.getenv(...)).
    // stopifnot fails (non-zero exit) when any value parses to NA or to
    // the wrong logical.
    let program = r#"
strength <- as.logical(Sys.getenv("MVMR_STRENGTH"))
strhet <- as.logical(Sys.getenv("MVMR_STRHET"))
pleiotropy <- as.logical(Sys.getenv("MVMR_PLEIOTROPY"))
qhet <- as.logical(Sys.getenv("MVMR_QHET"))
stopifnot(
  identical(strength, TRUE),
  identical(strhet, TRUE),
  identical(pleiotropy, TRUE),
  identical(qhet, FALSE),
  !anyNA(c(strength, strhet, pleiotropy, qhet))
)
cat("mvmr bool parse chain ok\n")
"#;
    let status = std::process::Command::new(&rscript)
        .args(["--vanilla", "-e", program])
        .envs(&env)
        .status()
        .unwrap();
    assert!(
        status.success(),
        "R as.logical parse chain must read the rendered values; exit {status}"
    );
}

/// Locate `Rscript` on PATH without pulling in a `which` crate.
fn which_rscript() -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("Rscript"))
        .find(|candidate| candidate.is_file())
}
