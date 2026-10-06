//! Manifest node entries: the declarative per-node contract of a plugin
//! family.
//!
//! Everything here is plain data that round-trips through TOML. Runtime
//! objects ([`NodePorts`]) are only ever *produced* from these declarations
//! by [`compile_ports`]; they never appear inside a manifest.

use std::collections::BTreeMap;
use std::path::{Component, Path};

use container_runtime::{ContainerNetwork, DEFAULT_TIMEOUT_SECS, PullPolicy};
use dag_core::NodePorts;
use dag_core::value::PortType;
use serde::{Deserialize, Serialize};

/// One node kind exposed by a plugin family manifest. One `[[nodes]]` table
/// in the family TOML maps to one of these.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeDefinition {
    /// Registry kind, unique workspace-wide (e.g. `mtag_container`).
    pub kind: String,
    /// One-line description shown in node listings (`NodeFactory::desc`).
    pub desc: String,
    /// Long agent-facing documentation (`NodeFactory::doc`). This becomes
    /// what the agent reads when wiring the node — write it for them.
    pub doc: String,
    #[serde(default)]
    pub deprecated: bool,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Derived as `/artifacts/{kind}` when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_prefix: Option<String>,
    pub ports: PortLayout,
    /// Parameter DSL. The M2 compiler turns this into the node's JSON
    /// Schema, so `doc` and `default` here directly shape agent ergonomics.
    #[serde(default)]
    pub params: BTreeMap<String, ParamSpec>,
    pub command: CommandSpec,
    #[serde(default)]
    pub resources: Resources,
}

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

/// Declarative port layout as authored in the manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortLayout {
    #[serde(default)]
    pub inputs: Vec<PortSpec>,
    /// Declared outputs serve two uses from one declaration: the container's
    /// required output files and the node's output ports.
    pub outputs: Vec<OutputSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortSpec {
    /// Closed vocabulary: container nodes are File-to-File by policy, so v0
    /// accepts only `file`. Unknown values fail at parse time.
    pub r#type: PortKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Payload formats accepted on this input, in addition to any primary
    /// format implied by the node contract.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accepted_formats: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PortKind {
    File,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputSpec {
    /// Safe path relative to `/work`. Must exist after a successful run or
    /// the node fails.
    pub path: String,
    /// Format label passed downstream (e.g. `mtag_results`, `r_rds`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// One parameter of the node's spec DSL.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParamSpec {
    pub r#type: ParamType,
    /// JSON-typed default. `None` plus `optional = false` marks a required
    /// parameter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    /// May be absent without a default: resolves to null, which renders
    /// as an empty string on every surface. The canonical v0 pattern for
    /// optional tool flags: pass the param through `env`, let the script
    /// test `[ -n "$VAR" ]` and build its own flag list.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclusive_min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclusive_max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_len: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_len: Option<usize>,
    /// Boolean gates: when this parameter is set, each named bool parameter
    /// must be true (e.g. `equal_h2` requires `perfect_gencov`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
}

/// Parameter type vocabulary. Enum-typed params arrive with the panel
/// selection wave (`from_param`); v0 covers the wrapper shapes being
/// migrated first (bools, numbers, strings, string arrays).
///
/// Two boolean vocabularies: [`ParamType::Bool`] carries **value**
/// semantics (`false` renders `"false"` — for consumers that read the
/// value, e.g. R `as.logical()`), while [`ParamType::Flag`] carries
/// **presence** semantics (`true` renders `"true"`, `false` renders `""`
/// — for consumers that test `[ -n "$VAR" ]`). Flag is a rendering
/// contract, not a new JSON shape: both accept JSON booleans and compile
/// to the same `"type": "boolean"` schema node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamType {
    Bool,
    Flag,
    Int,
    Number,
    String,
    StringArray,
}

impl std::fmt::Display for ParamType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Bool => "boolean",
            Self::Flag => "boolean flag (presence)",
            Self::Int => "integer",
            Self::Number => "number",
            Self::String => "string",
            Self::StringArray => "array of strings",
        };
        f.write_str(name)
    }
}

/// How the container is invoked. `script`/`env`/`argv` values go through
/// the M2 template renderer with the params in scope (v0: `{{name}}`
/// substitution only, no control flow).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandSpec {
    /// `command[0]`: `sh`, `Rscript`, `python`, or a runner path.
    pub interpreter: String,
    /// Fixed arguments after the interpreter (baked-runner style).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub argv: Vec<String>,
    /// Inline script; inserted right after the interpreter, matching the
    /// `container_command` contract. Mutually exclusive with
    /// [`CommandSpec::script_file`]; the loader inlines `script_file`
    /// contents into this field before any validation runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
    /// Path to a script file inside the plugin directory (e.g.
    /// `scripts/h2.sh`), resolved by the loader relative to the plugin
    /// root. Authors prefer this over inline scripts for anything longer
    /// than a few lines: editors highlight it and diffs stay clean.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script_file: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Inline text files materialized under `/work/.autonomics/files`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, String>,
}

/// Resource and security profile. Reuses the runtime's own enums where
/// they exist; unknown TOML values fail at parse time.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resources {
    /// `None` keeps the runtime default (isolated, no network devices).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<ContainerNetwork>,
    #[serde(default = "default_true")]
    pub read_only_rootfs: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_policy: Option<PullPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpus: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pids_limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shm_size: Option<String>,
    /// GPU passthrough in `GpuRequest::parse` syntax (`all`, `2`,
    /// `device=0,2`); validated when compiled in M2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpus: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

fn default_true() -> bool {
    true
}

impl Default for Resources {
    /// A missing `[nodes.resources]` table must mean the hardened
    /// defaults, not the zero values: `#[serde(default)]` on the field
    /// goes through here, and `Default::derive` would flip
    /// `read_only_rootfs` to `false`.
    fn default() -> Self {
        Self {
            network: None,
            read_only_rootfs: true,
            pull_policy: None,
            cpus: None,
            memory: None,
            pids_limit: None,
            shm_size: None,
            gpus: None,
            user: None,
        }
    }
}

/// Compile the declarative layout into the runtime port contract. The only
/// place `NodePorts` is born from manifest data.
pub fn compile_ports(layout: &PortLayout) -> NodePorts {
    let mut ports = NodePorts::new();
    for input in &layout.inputs {
        // Three tiers, matching the builder surface: bare port, labeled
        // port, or a labeled port accepting alternate payload formats.
        ports = match (&input.label, input.accepted_formats.first()) {
            (None, _) => ports.add_input_port_of_type(None, PortType::File),
            (Some(label), None) => {
                ports.add_input_port_of_type_with_label(None, PortType::File, label.clone())
            }
            (Some(label), Some(primary)) => ports.add_input_port_of_type_with_accepted_formats(
                None,
                PortType::File,
                label.clone(),
                primary.clone(),
                input.accepted_formats.iter().skip(1).cloned(),
            ),
        };
    }
    for output in &layout.outputs {
        let label = output
            .label
            .clone()
            .unwrap_or_else(|| default_label(&output.path));
        ports = match &output.format {
            Some(format) => ports.add_output_port_of_type_with_label_and_format(
                None,
                PortType::File,
                label,
                format.clone(),
            ),
            None => ports.add_output_port_of_type(None, PortType::File),
        };
    }
    ports
}

fn default_label(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("output")
        .to_string()
}

/// Cross-field validation over one node entry. Pure: reused by the M4
/// loader and the `nodedev validate` subcommand.
pub fn validate(node: &NodeDefinition) -> Result<(), String> {
    if node.kind.is_empty()
        || !node
            .kind
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(format!(
            "node kind `{}` must be non-empty lowercase `[a-z0-9_]`",
            node.kind
        ));
    }
    if node.timeout_secs == 0 {
        return Err(format!("node `{}` has timeout_secs of zero", node.kind));
    }
    if let Some(prefix) = &node.artifact_prefix
        && !prefix.starts_with('/')
    {
        return Err(format!(
            "node `{}` artifact_prefix must be an absolute VFS path",
            node.kind
        ));
    }
    if node.ports.outputs.is_empty() {
        return Err(format!(
            "node `{}` must declare at least one output",
            node.kind
        ));
    }
    for output in &node.ports.outputs {
        validate_workspace_relative_path(&output.path)
            .map_err(|error| format!("node `{}` output `{}`: {error}", node.kind, output.path))?;
    }
    for (name, spec) in &node.params {
        // min_len/max_len measure arrays only (check_bounds reads them via
        // as_array); on any other type they would reject every value while
        // the compiled schema still advertises minLength/maxLength. Reject
        // that combination at load time instead.
        if spec.r#type != ParamType::StringArray
            && (spec.min_len.is_some() || spec.max_len.is_some())
        {
            return Err(format!(
                "node `{}` param `{name}`: min_len/max_len are only valid on string_array params",
                node.kind
            ));
        }
        for target in &spec.requires {
            let target_spec = node.params.get(target).ok_or_else(|| {
                format!(
                    "node `{}` param `{name}` requires unknown param `{target}`",
                    node.kind
                )
            })?;
            if !matches!(target_spec.r#type, ParamType::Bool | ParamType::Flag) {
                return Err(format!(
                    "node `{}` param `{name}` requires `{target}`, which is not a bool or flag",
                    node.kind
                ));
            }
        }
    }
    // Template closure: every {{param}} referenced by the command must be
    // declared. Undeclared refs would render as literal text at runtime and
    // fail deep inside the container instead of at load time.
    for reference in scan_template_refs(node) {
        if !node.params.contains_key(&reference) {
            return Err(format!(
                "node `{}` command references undeclared param `{{{{{reference}}}}}`",
                node.kind
            ));
        }
    }
    // Flag placement: presence semantics (false renders "") only exist on
    // the env surface. On argv an empty value would still occupy an
    // argument slot, and on the script surface the empty value arrives
    // quoted ('' / "") — both would change the tool invocation rather
    // than omit a flag. Reject the combination at load time.
    let outside_env = scan_template_refs_outside_env(node);
    for (name, spec) in &node.params {
        if spec.r#type == ParamType::Flag && outside_env.contains(name) {
            return Err(format!(
                "node `{}` param `{name}` has type flag; flag params may only be referenced from [command.env]",
                node.kind
            ));
        }
    }
    Ok(())
}

fn validate_workspace_relative_path(path: &str) -> Result<(), String> {
    if path.is_empty() || path.contains('\0') {
        return Err("path cannot be empty".into());
    }
    let candidate = Path::new(path);
    if candidate.is_absolute()
        || !candidate
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(format!("path `{path}` must be a safe relative path"));
    }
    Ok(())
}

/// Collect `{{param}}` references across every templatable command surface.
/// Byte-based scan: `{{` positions are always UTF-8 char boundaries because
/// multibyte sequences never contain ASCII bytes.
fn scan_template_refs(node: &NodeDefinition) -> Vec<String> {
    let mut refs = Vec::new();
    for arg in &node.command.argv {
        scan_text_for_refs(arg, &mut refs);
    }
    if let Some(script) = &node.command.script {
        scan_text_for_refs(script, &mut refs);
    }
    for value in node.command.env.values() {
        scan_text_for_refs(value, &mut refs);
    }
    for value in node.command.files.values() {
        scan_text_for_refs(value, &mut refs);
    }
    finish_refs(refs)
}

/// References on every templatable surface **except** `command.env`. The
/// audience of the flag placement rule: presence semantics only exist on
/// env, so a flag referenced anywhere else is a load-time error.
fn scan_template_refs_outside_env(node: &NodeDefinition) -> Vec<String> {
    let mut refs = Vec::new();
    for arg in &node.command.argv {
        scan_text_for_refs(arg, &mut refs);
    }
    if let Some(script) = &node.command.script {
        scan_text_for_refs(script, &mut refs);
    }
    for value in node.command.files.values() {
        scan_text_for_refs(value, &mut refs);
    }
    finish_refs(refs)
}

fn finish_refs(mut refs: Vec<String>) -> Vec<String> {
    refs.sort();
    refs.dedup();
    refs
}

/// Scan one text for `{{token}}` references (see [`scan_template_refs`]
/// for the byte-level boundary argument).
fn scan_text_for_refs(text: &str, refs: &mut Vec<String>) {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'{' && bytes[i + 1] == b'{' && text[i + 2..].find("}}").is_some() {
            let end = text[i + 2..].find("}}").unwrap();
            let token = text[i + 2..i + 2 + end].trim();
            if !token.is_empty() && token.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                refs.push(token.to_string());
            }
            i = i + 2 + end + 2;
            continue;
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MTAG_ENTRY_TOML: &str = r#"
kind = "mtag_container"
desc = "Runs original MTAG 1.0.8 on two GWAS sumstats."
doc = "Runs the official Python MTAG implementation on two trait files."
timeout_secs = 3600

[ports]
inputs = [{ type = "file" }, { type = "file" }]
outputs = [
    { path = "mtag_trait_1.txt", format = "mtag_results" },
    { path = "mtag.log", format = "mtag_log" },
]

[params]
time_limit_hours = { type = "number", default = 1.0, exclusive_min = 0.0, doc = "Optimizer time limit" }
force = { type = "bool", default = false }
equal_h2 = { type = "bool", default = false, requires = ["force"] }

[command]
interpreter = "sh"
script = """
set -eu
mtag --time_limit {{ time_limit_hours }} --out "$AUTONOMICS_OUTPUT2"
"""
"#;

    fn mtag_entry() -> NodeDefinition {
        toml::from_str(MTAG_ENTRY_TOML).unwrap()
    }

    #[test]
    fn parses_a_complete_node_entry() {
        let node = mtag_entry();
        assert_eq!(node.kind, "mtag_container");
        assert_eq!(node.ports.inputs.len(), 2);
        assert_eq!(node.ports.outputs.len(), 2);
        assert_eq!(node.ports.outputs[0].path, "mtag_trait_1.txt");
        assert_eq!(
            node.ports.outputs[0].format.as_deref(),
            Some("mtag_results")
        );
        assert_eq!(node.params["force"].r#type, ParamType::Bool);
        assert_eq!(
            node.params["time_limit_hours"].default,
            Some(serde_json::json!(1.0))
        );
        assert_eq!(node.command.interpreter, "sh");
        assert!(
            node.command
                .script
                .as_deref()
                .unwrap()
                .contains("{{ time_limit_hours }}")
        );
    }

    #[test]
    fn unknown_fields_fail_at_parse_time() {
        let broken = MTAG_ENTRY_TOML.replacen("desc =", "description =", 1);
        assert!(
            toml::from_str::<NodeDefinition>(&broken).is_err(),
            "misspelled fields must be rejected with a field path"
        );
    }

    #[test]
    fn unknown_port_kinds_fail_at_parse_time() {
        let broken = MTAG_ENTRY_TOML.replacen("type = \"file\" }", "type = \"dataframe\" }", 1);
        assert!(toml::from_str::<NodeDefinition>(&broken).is_err());
    }

    #[test]
    fn validation_rejects_structural_defects() {
        let mut node = mtag_entry();

        node.ports.outputs.clear();
        assert!(validate(&node).unwrap_err().contains("at least one output"));

        let mut node = mtag_entry();
        node.ports.outputs[0].path = "../escape.txt".into();
        assert!(validate(&node).unwrap_err().contains("safe relative path"));

        let mut node = mtag_entry();
        node.kind = "Mtag-Container".into();
        assert!(validate(&node).unwrap_err().contains("[a-z0-9_]"));

        let mut node = mtag_entry();
        node.timeout_secs = 0;
        assert!(validate(&node).unwrap_err().contains("timeout_secs"));

        let mut node = mtag_entry();
        node.params.get_mut("equal_h2").unwrap().requires = vec!["missing_param".into()];
        assert!(validate(&node).unwrap_err().contains("unknown param"));
    }

    #[test]
    fn validation_rejects_undeclared_template_refs() {
        let broken =
            MTAG_ENTRY_TOML.replacen("{{ time_limit_hours }}", "{{ undeclared_param }}", 1);
        let node: NodeDefinition = toml::from_str(&broken).unwrap();
        assert!(
            validate(&node).unwrap_err().contains("undeclared param"),
            "undeclared template refs must fail at load, not inside the container"
        );
    }

    const FLAG_ENTRY_TOML: &str = r#"
kind = "flag_kind"
desc = "Flag surface rules"
doc = "Flag surface rules"
timeout_secs = 60

[ports]
outputs = [{ path = "out.txt" }]

[params]
verbose = { type = "flag", default = false, doc = "Emit progress lines" }

[command]
interpreter = "sh"
script = "echo hi"

[command.env]
VERBOSE = "{{ verbose }}"
"#;

    #[test]
    fn flag_params_parse_and_are_env_only() {
        // `type = "flag"` is the presence-semantics boolean (F02): JSON
        // booleans in, "true"/"" out through env. Referenced from env it
        // validates; referenced from the script or argv surface it is
        // rejected at load time — an empty value there would occupy an
        // argument slot / arrive quoted instead of omitting a flag.
        let node: NodeDefinition = toml::from_str(FLAG_ENTRY_TOML).unwrap();
        assert_eq!(node.params["verbose"].r#type, ParamType::Flag);
        assert!(validate(&node).is_ok());

        let broken = FLAG_ENTRY_TOML.replacen("echo hi", "echo {{ verbose }}", 1);
        let node: NodeDefinition = toml::from_str(&broken).unwrap();
        let error = validate(&node).unwrap_err();
        assert!(error.contains("type flag"), "{error}");
        assert!(error.contains("[command.env]"), "{error}");

        let broken = FLAG_ENTRY_TOML.replacen(
            "script = \"echo hi\"",
            "argv = [\"--verbose\", \"{{ verbose }}\"]\nscript = \"echo hi\"",
            1,
        );
        let node: NodeDefinition = toml::from_str(&broken).unwrap();
        assert!(validate(&node).unwrap_err().contains("type flag"));
    }

    #[test]
    fn requires_accepts_bool_and_flag_targets() {
        // `requires` gates fire on true, which both boolean vocabularies
        // carry; non-boolean targets stay rejected (message now says
        // "not a bool or flag").
        let base = FLAG_ENTRY_TOML
            .replacen(
                "verbose = { type = \"flag\", default = false, doc = \"Emit progress lines\" }",
                "verbose = { type = \"flag\", default = false }\n\
                 mode = { type = \"bool\", default = false }\n\
                 count = { type = \"int\", default = 1 }",
                1,
            )
            .replacen("kind = \"flag_kind\"", "kind = \"gate_kind\"", 1);

        let bool_target = base.replacen(
            "count = { type = \"int\", default = 1 }",
            "count = { type = \"int\", default = 1, requires = [\"mode\"] }",
            1,
        );
        let node: NodeDefinition = toml::from_str(&bool_target).unwrap();
        assert!(validate(&node).is_ok(), "bool targets remain legal gates");

        let flag_target = base.replacen(
            "count = { type = \"int\", default = 1 }",
            "count = { type = \"int\", default = 1, requires = [\"verbose\"] }",
            1,
        );
        let node: NodeDefinition = toml::from_str(&flag_target).unwrap();
        assert!(validate(&node).is_ok(), "flag targets are legal gates");

        let int_target = base.replacen(
            "mode = { type = \"bool\", default = false }",
            "mode = { type = \"bool\", default = false, requires = [\"count\"] }",
            1,
        );
        let node: NodeDefinition = toml::from_str(&int_target).unwrap();
        assert!(validate(&node).unwrap_err().contains("not a bool or flag"));
    }

    #[test]
    fn compile_ports_builds_the_runtime_contract() {
        let ports = compile_ports(&mtag_entry().ports);
        assert_eq!(ports.input_ports().len(), 2);
        assert_eq!(ports.output_ports().len(), 2);
    }
}
