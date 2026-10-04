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
    workspace::ProposalWorkspace,
};

#[derive(Debug, Clone)]
pub struct ApprovedImage {
    pub reference: String,
    pub interpreters: Vec<String>,
}

#[derive(Debug, Default)]
pub struct ImageCatalog {
    images: BTreeMap<String, ApprovedImage>,
}

impl ImageCatalog {
    pub fn insert(&mut self, id: impl Into<String>, image: ApprovedImage) {
        self.images.insert(id.into(), image);
    }

    pub fn get(&self, id: &str) -> Option<&ApprovedImage> {
        self.images.get(id)
    }
}

pub fn validate_workspace(
    proposal: &crate::Proposal,
    workspace: &ProposalWorkspace,
    catalog: &ImageCatalog,
    installed_kinds: &[String],
    attempt: u32,
) -> ValidationReport {
    let mut gates = Vec::new();
    let manifest = gate(&mut gates, "manifest", || {
        load_manifest(workspace).map_err(|errors| errors.join("; "))
    });
    let manifest = match manifest {
        Some(value) => value,
        None => return blocked_rest(proposal, attempt, gates, "manifest"),
    };

    let policy = validate_policy(proposal, workspace, &manifest, catalog);
    match policy {
        Ok(()) => gates.push(GateResult::pass("policy")),
        Err(error) => {
            gates.push(GateResult::fail("policy", error));
            return blocked_rest(proposal, attempt, gates, "policy");
        }
    }

    let collision = installed_kinds.contains(&proposal.node_kind);
    if collision {
        gates.push(GateResult::fail(
            "kind_collision",
            format!("node kind `{}` is already registered", proposal.node_kind),
        ));
        return blocked_rest(proposal, attempt, gates, "kind_collision");
    }
    gates.push(GateResult::pass("kind_collision"));

    let script = manifest.nodes[0].command.script.clone().unwrap_or_default();
    match static_script_review(&manifest.nodes[0], &script) {
        Ok(()) => gates.push(GateResult::pass("script_static")),
        Err(error) => {
            gates.push(GateResult::fail("script_static", error));
            return blocked_rest(proposal, attempt, gates, "script_static");
        }
    }

    match registry_compile(&manifest) {
        Ok(()) => gates.push(GateResult::pass("registry_compile")),
        Err(error) => {
            gates.push(GateResult::fail("registry_compile", error));
            return blocked_rest(proposal, attempt, gates, "registry_compile");
        }
    }

    match secret_scan(workspace) {
        Ok(()) => gates.push(GateResult::pass("secret_scan")),
        Err(error) => gates.push(GateResult::fail("secret_scan", error)),
    }

    ValidationReport::new(&proposal.proposal_id, attempt, gates)
}

fn blocked_rest(
    proposal: &crate::Proposal,
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
    ValidationReport::new(&proposal.proposal_id, attempt, gates)
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

fn load_manifest(
    workspace: &ProposalWorkspace,
) -> std::result::Result<PluginManifest, Vec<String>> {
    let text = workspace
        .read_text("manifest.toml")
        .map_err(|error| vec![error.to_string()])?;
    let mut manifest: PluginManifest =
        toml::from_str(&text).map_err(|error| vec![error.to_string()])?;
    let Some(node) = manifest.nodes.first_mut() else {
        return Err(vec!["manifest has no nodes".into()]);
    };
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
    for node in &manifest.nodes {
        node_definition::validate(node).map_err(|error| vec![error])?;
    }
    Ok(manifest)
}

fn validate_policy(
    proposal: &crate::Proposal,
    workspace: &ProposalWorkspace,
    manifest: &PluginManifest,
    catalog: &ImageCatalog,
) -> std::result::Result<(), String> {
    if manifest.nodes.len() != 1 {
        return Err("MVP plugins must declare exactly one node".into());
    }
    if manifest.plugin_name != proposal.plugin_name {
        return Err(format!(
            "manifest plugin_name `{}` does not match proposal `{}`",
            manifest.plugin_name, proposal.plugin_name
        ));
    }
    let node = &manifest.nodes[0];
    if node.kind != proposal.node_kind {
        return Err(format!(
            "manifest kind `{}` does not match derived kind `{}`",
            node.kind, proposal.node_kind
        ));
    }
    if !manifest.panels.is_empty() {
        return Err("panels are not allowed in the greenfield MVP".into());
    }
    let Some(image_id) = proposal.image_id.as_deref() else {
        return Err("proposal has no bound image".into());
    };
    let Some(expected_reference) = proposal.image_reference.as_deref() else {
        return Err("proposal has no digest-pinned image reference".into());
    };
    if manifest.image.reference.as_str() != expected_reference {
        return Err("manifest image differs from proposal image".into());
    }
    let Some(image) = catalog.get(image_id) else {
        return Err(format!("image `{image_id}` is not approved"));
    };
    if image.reference != expected_reference {
        return Err("proposal image differs from approved image catalog".into());
    }
    if !image
        .interpreters
        .iter()
        .any(|item| item == &node.command.interpreter)
    {
        return Err(format!(
            "interpreter `{}` is not approved for image `{}`",
            node.command.interpreter, image_id
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
    let readme = workspace
        .read_text("README.md")
        .map_err(|error| format!("README.md is required: {error}"))?;
    if readme.contains("TODO") || proposal.rationale.contains("TODO") {
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
    let node = &manifest.nodes[0];
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
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn secret_scan(workspace: &ProposalWorkspace) -> std::result::Result<(), String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ProposalAction, ProposalStatus};

    const IMAGE: &str = "docker.io/library/hello-world@sha256:2dad70a9583f93db1dcc9a560b7d5b309af4a5151dfaf615f80d059a0925d78c";

    fn setup(tmp: &Path) -> (crate::Proposal, ProposalWorkspace, ImageCatalog) {
        let workspace = ProposalWorkspace::new(tmp.join("repo"));
        workspace
            .write_text(
                "manifest.toml",
                &format!(
                    r#"
schema_version = 1
plugin_name = "demo-plugin"

[image]
reference = "{IMAGE}"

[[nodes]]
kind = "demo_plugin"
desc = "Demo adapter"
doc = "Input 0 becomes output 0."

[nodes.ports]
inputs = [{{ type = "file", label = "input" }}]
outputs = [{{ path = "out.txt", format = "txt" }}]

[nodes.command]
interpreter = "sh"
script_file = "scripts/adapter.sh"
"#
                ),
            )
            .unwrap();
        workspace
            .write_text(
                "scripts/adapter.sh",
                "set -eu\ncp \"$AUTONOMICS_INPUT0\" \"$AUTONOMICS_OUTPUT0\"\n",
            )
            .unwrap();
        workspace.write_text("README.md", "# demo\n").unwrap();

        let proposal = crate::Proposal {
            schema_version: 1,
            proposal_id: "P-test".into(),
            plugin_name: "demo-plugin".into(),
            node_kind: "demo_plugin".into(),
            action: ProposalAction::NewPlugin,
            status: ProposalStatus::Draft,
            authored_by: "agent".into(),
            request_ids: vec!["R-test".into()],
            image_id: Some("demo".into()),
            image_reference: Some(IMAGE.into()),
            source_commit: None,
            remote: None,
            pushed_commit: None,
            latest_report: None,
            rationale: "test".into(),
            created_at: 0,
            updated_at: 0,
        };
        let mut catalog = ImageCatalog::default();
        catalog.insert(
            "demo",
            ApprovedImage {
                reference: IMAGE.into(),
                interpreters: vec!["sh".into()],
            },
        );
        (proposal, workspace, catalog)
    }

    #[test]
    fn minimal_plugin_passes_deterministic_gates() {
        let tmp = tempfile::tempdir().unwrap();
        let (proposal, workspace, catalog) = setup(tmp.path());
        let report = validate_workspace(&proposal, &workspace, &catalog, &[], 1);
        assert_eq!(report.overall, GateStatus::Pass, "{report:?}");
    }

    #[test]
    fn collisions_and_host_access_fail_closed() {
        let tmp = tempfile::tempdir().unwrap();
        let (proposal, workspace, catalog) = setup(tmp.path());
        let report = validate_workspace(
            &proposal,
            &workspace,
            &catalog,
            &["demo_plugin".to_string()],
            1,
        );
        assert_eq!(report.overall, GateStatus::Fail);

        let tmp2 = tempfile::tempdir().unwrap();
        let (proposal2, workspace2, catalog2) = setup(tmp2.path());
        workspace2
            .write_text(
                "scripts/adapter.sh",
                "set -eu\ncurl http://example.com > \"$AUTONOMICS_OUTPUT0\"\n",
            )
            .unwrap();
        let report = validate_workspace(&proposal2, &workspace2, &catalog2, &[], 1);
        assert_eq!(report.overall, GateStatus::Fail);
        assert!(report.gates.iter().any(|gate| gate.name == "script_static"));
    }
}
