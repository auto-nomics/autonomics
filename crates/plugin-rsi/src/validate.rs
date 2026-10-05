use std::{collections::BTreeMap, path::Path};

use container_plugin::{
    compile::{compile_schema, spec_compile::compile_container_spec},
    manifest::PluginManifest,
    node_definition::{self, NodeDefinition},
};
use container_runtime::ContainerNetwork;

use crate::{
    Error, Result,
    report::{GateResult, GateStatus, ValidationReport},
    workspace::PluginWorkspace,
};

/// A reusable runtime environment selected by a plugin.
///
/// This is not a plugin-owned image: ordinary plugin repositories contain
/// source and tests, while environment images are managed as shared assets.
#[derive(Debug, Clone)]
pub struct Environment {
    pub reference: String,
    pub interpreters: Vec<String>,
}

/// Approved digest-pinned runtime environments available to plugins.
#[derive(Debug, Default)]
pub struct EnvironmentCatalog {
    environments: BTreeMap<String, Environment>,
}

impl EnvironmentCatalog {
    pub fn insert(&mut self, id: impl Into<String>, environment: Environment) {
        self.environments.insert(id.into(), environment);
    }

    pub fn get(&self, id: &str) -> Option<&Environment> {
        self.environments.get(id)
    }

    /// Return the catalog id owning one digest-pinned reference.
    pub fn find_reference(&self, reference: &str) -> Option<&str> {
        self.environments
            .iter()
            .find(|(_, environment)| environment.reference == reference)
            .map(|(id, _)| id.as_str())
    }
}

pub fn validate_workspace(
    plugin_name: &str,
    workspace: &PluginWorkspace,
    catalog: &EnvironmentCatalog,
    installed_kinds: &[String],
    owned_kinds: &[String],
    attempt: u32,
) -> ValidationReport {
    let mut gates = Vec::new();
    let manifest = gate(&mut gates, "manifest", || {
        load_manifest(workspace).map_err(|errors| errors.join("; "))
    });
    let manifest = match manifest {
        Some(value) => value,
        None => return blocked_rest(plugin_name, attempt, gates, "manifest"),
    };

    let policy = validate_policy(plugin_name, workspace, &manifest, catalog);
    match policy {
        Ok(()) => gates.push(GateResult::pass("policy")),
        Err(error) => {
            gates.push(GateResult::fail("policy", error));
            return blocked_rest(plugin_name, attempt, gates, "policy");
        }
    }

    let collisions = manifest
        .nodes
        .iter()
        .map(|node| node.kind.clone())
        .filter(|kind| installed_kinds.contains(kind) && !owned_kinds.contains(kind))
        .collect::<Vec<_>>();
    if !collisions.is_empty() {
        gates.push(GateResult::fail(
            "kind_collision",
            format!("node kinds already registered: {}", collisions.join(", ")),
        ));
        return blocked_rest(plugin_name, attempt, gates, "kind_collision");
    }
    gates.push(GateResult::pass("kind_collision"));

    let script_results = manifest
        .nodes
        .iter()
        .map(|node| {
            let script = node.command.script.clone().unwrap_or_default();
            static_script_review(node, &script)
        })
        .collect::<Vec<_>>();
    if let Some(Err(error)) = script_results.into_iter().find(|result| result.is_err()) {
        gates.push(GateResult::fail("script_static", error));
        return blocked_rest(plugin_name, attempt, gates, "script_static");
    }
    gates.push(GateResult::pass("script_static"));

    match registry_compile(&manifest) {
        Ok(()) => gates.push(GateResult::pass("registry_compile")),
        Err(error) => {
            gates.push(GateResult::fail("registry_compile", error));
            return blocked_rest(plugin_name, attempt, gates, "registry_compile");
        }
    }

    match secret_scan(workspace) {
        Ok(()) => gates.push(GateResult::pass("secret_scan")),
        Err(error) => gates.push(GateResult::fail("secret_scan", error)),
    }

    ValidationReport::new(plugin_name, attempt, gates)
}

fn blocked_rest(
    plugin_name: &str,
    attempt: u32,
    mut gates: Vec<GateResult>,
    failed_gate: &str,
) -> ValidationReport {
    for name in ["script_static", "registry_compile", "secret_scan"] {
        if gates.iter().any(|gate| gate.name == name) {
            continue;
        }
        gates.push(GateResult::blocked(
            name,
            format!("not run after `{failed_gate}` failed"),
        ));
    }
    ValidationReport::new(plugin_name, attempt, gates)
}

fn gate<T, F>(gates: &mut Vec<GateResult>, name: &str, operation: F) -> Option<T>
where
    F: FnOnce() -> std::result::Result<T, String>,
{
    match operation() {
        Ok(value) => {
            gates.push(GateResult::pass(name));
            Some(value)
        }
        Err(error) => {
            gates.push(GateResult::fail(name, error));
            None
        }
    }
}

fn load_manifest(workspace: &PluginWorkspace) -> std::result::Result<PluginManifest, Vec<String>> {
    let text = workspace
        .read_text("manifest.toml")
        .map_err(|error| vec![error.to_string()])?;
    let mut manifest: PluginManifest =
        toml::from_str(&text).map_err(|error| vec![error.to_string()])?;
    if manifest.nodes.is_empty() {
        return Err(vec!["manifest has no nodes".into()]);
    }
    for node in manifest.nodes.iter_mut() {
        match (&node.command.script, &node.command.script_file) {
            (Some(_), Some(_)) => return Err(vec!["script and script_file are both set".into()]),
            (None, Some(relative)) => {
                let script = workspace
                    .read_text(relative)
                    .map_err(|error| vec![error.to_string()])?;
                node.command.script = Some(script);
                node.command.script_file = None;
            }
            (None, None) => return Err(vec!["command must declare script_file".into()]),
            (Some(_), None) => return Err(vec!["RSI plugins must use script_file".into()]),
        }
    }
    for node in &manifest.nodes {
        node_definition::validate(node).map_err(|error| vec![error])?;
    }
    Ok(manifest)
}

fn validate_policy(
    plugin_name: &str,
    workspace: &PluginWorkspace,
    manifest: &PluginManifest,
    catalog: &EnvironmentCatalog,
) -> std::result::Result<(), String> {
    if manifest.nodes.is_empty() {
        return Err("manifest must declare at least one node".into());
    }
    if manifest.plugin_name != plugin_name {
        return Err(format!(
            "manifest plugin_name `{}` does not match workspace `{plugin_name}`",
            manifest.plugin_name
        ));
    }
    if !manifest.panels.is_empty() {
        return Err("panels are not allowed in the greenfield MVP".into());
    }
    let files = workspace.list_files().map_err(|error| error.to_string())?;
    if files.iter().any(|path| {
        Path::new(path)
            .file_name()
            .is_some_and(|name| name == "Dockerfile")
    }) {
        return Err(
            "ordinary plugins cannot contain a Dockerfile; use an approved environment".into(),
        );
    }
    let expected_reference = manifest.image.reference.as_str();
    let Some(environment_id) = catalog.find_reference(&expected_reference) else {
        return Err(format!(
            "environment `{expected_reference}` is not approved for RSI"
        ));
    };
    let Some(environment) = catalog.get(environment_id) else {
        return Err(format!("environment `{environment_id}` is not approved"));
    };
    for node in &manifest.nodes {
        if !environment
            .interpreters
            .iter()
            .any(|item| item == &node.command.interpreter)
        {
            return Err(format!(
                "interpreter `{}` is not approved for environment `{}`",
                node.command.interpreter, environment_id
            ));
        }
        if !matches!(
            node.resources.network,
            None | Some(ContainerNetwork::Isolated)
        ) {
            return Err("network egress requires a later reviewed release class".into());
        }
        if !node.resources.read_only_rootfs {
            return Err("read_only_rootfs cannot be disabled in the MVP".into());
        }
        if node.resources.gpus.is_some() || node.resources.user.is_some() {
            return Err("GPU and user overrides are not allowed in the MVP".into());
        }
    }
    let readme = workspace
        .read_text("README.md")
        .map_err(|error| format!("README.md is required: {error}"))?;
    if readme.contains("TODO") {
        return Err("TODO placeholders are not reviewable".into());
    }
    Ok(())
}

fn static_script_review(node: &NodeDefinition, script: &str) -> std::result::Result<(), String> {
    if script.trim().is_empty() {
        return Err("adapter script is empty".into());
    }
    if node.command.interpreter.contains("sh") && !script.contains("set -eu") {
        return Err("shell adapters must start with strict `set -eu` behavior".into());
    }
    if node.ports.inputs.is_empty() {
        return Err("MVP nodes must have at least one file input".into());
    }
    if !script.contains("AUTONOMICS_INPUT0") {
        return Err("script must validate/read AUTONOMICS_INPUT0".into());
    }
    if !script.contains("AUTONOMICS_OUTPUT0") {
        return Err("script must write AUTONOMICS_OUTPUT0".into());
    }
    for forbidden in [
        "curl ",
        "wget ",
        "ssh ",
        "scp ",
        "/home/",
        "/Users/",
        "/mnt/",
        "AUTONOMICS_PLUGIN_ROOT",
    ] {
        if script.contains(forbidden) {
            return Err(format!(
                "forbidden host/network token in script: {forbidden:?}"
            ));
        }
    }
    Ok(())
}

fn registry_compile(manifest: &PluginManifest) -> std::result::Result<(), String> {
    for node in &manifest.nodes {
        compile_one_node(manifest, node)?;
    }
    Ok(())
}

fn compile_one_node(
    manifest: &PluginManifest,
    node: &NodeDefinition,
) -> std::result::Result<(), String> {
    let _schema = compile_schema(&node.params);
    let ports = node_definition::compile_ports(&node.ports);
    if ports.input_ports().len() != node.ports.inputs.len()
        || ports.output_ports().len() != node.ports.outputs.len()
    {
        return Err("compiled port count does not match manifest".into());
    }

    let mut values = serde_json::Map::new();
    for (name, param) in &node.params {
        let value = param
            .default
            .clone()
            .or_else(|| param.optional.then_some(serde_json::Value::Null))
            .ok_or_else(|| format!("required MVP parameter `{name}` must provide a default"))?;
        values.insert(name.clone(), value);
    }
    compile_container_spec(
        node,
        &manifest.image,
        &manifest.panels,
        &serde_json::Value::Object(values),
    )
    .map_err(|error| format!("node `{}`: {error}", node.kind))?;
    Ok(())
}

fn secret_scan(workspace: &PluginWorkspace) -> std::result::Result<(), String> {
    for path in workspace.list_files().map_err(|error| error.to_string())? {
        let text = workspace
            .read_text(&path)
            .map_err(|error| error.to_string())?;
        for marker in ["ghp_", "github_pat_", "AKIA", "BEGIN PRIVATE KEY"] {
            if text.contains(marker) {
                return Err(format!("possible credential in {path}"));
            }
        }
    }
    Ok(())
}
