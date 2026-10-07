use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolRegistration, ToolResult};
use agentik_sdk::types::ToolResultContent;
mod common;

use plugin_rsi::GitRepo;
use plugin_rsi::{
    AgentProfile, Environment, EnvironmentCatalog, EnvironmentRegistry, GhPublisher,
    GhPublisherConfig, InstalledPluginSource, PluginLifecycle, PluginStatus, PluginStore,
    RequestIntent, RequestRecord, RequestSource, RequestStatus, RequestStore, RsiInfra,
    ValidationOutcome,
};
use serde_json::{Value, json};

const ENVIRONMENT_REFERENCE: &str = "docker.io/library/alpine@sha256:0123456789012345678901234567890123456789012345678901234567890123";
const PYTHON_REFERENCE: &str = "docker.io/library/python@sha256:1234567890123456789012345678901234567890123456789012345678901234";

fn catalog() -> EnvironmentCatalog {
    let mut catalog = EnvironmentCatalog::default();
    catalog.insert(
        "alpine",
        Environment {
            reference: ENVIRONMENT_REFERENCE.into(),
            interpreters: vec!["sh".into()],
        },
    );
    catalog.insert(
        "python",
        Environment {
            reference: PYTHON_REFERENCE.into(),
            interpreters: vec!["sh".into(), "python3".into()],
        },
    );
    catalog
}

fn request(plugin_name: &str) -> RequestRecord {
    RequestRecord {
        id: String::new(),
        created_at: 0,
        source: RequestSource::User,
        intent: RequestIntent::NewNode,
        summary: "Fork the reference adapter".into(),
        body: "Fork the installed adapter and give it a new node kind.".into(),
        plugin_name: Some(plugin_name.into()),
        evidence_ids: Vec::new(),
        status: RequestStatus::Open,
    }
}

fn node_json(kind: &str, script_file: &str) -> Value {
    json!({
        "kind": kind,
        "desc": "Copy one file",
        "doc": "Copies input 0 to output 0.",
        "ports": {
            "inputs": [{ "type": "file", "label": "input" }],
            "outputs": [{ "path": "result.txt", "format": "txt" }]
        },
        "command": {
            "interpreter": "sh",
            "argv": [],
            "script_file": script_file,
            "env": {},
            "files": {}
        }
    })
}

fn updated_node_json() -> Value {
    json!({
        "kind": "forked_adapter",
        "desc": "Copy two files",
        "doc": "Copies each input to the matching output.",
        "ports": {
            "inputs": [
                { "type": "file", "label": "left" },
                { "type": "file", "label": "right" }
            ],
            "outputs": [
                { "path": "result_0.txt", "format": "txt", "label": "left_result" },
                { "path": "result_1.txt", "format": "txt", "label": "right_result" }
            ]
        },
        "params": {
            "threshold": {
                "type": "number",
                "default": 0.5,
                "doc": "Threshold recorded by the adapter."
            }
        },
        "command": {
            "interpreter": "sh",
            "argv": [],
            "script_file": "scripts/forked.sh",
            "env": {},
            "files": {}
        }
    })
}

struct NoopRegistry;

impl plugin_rsi::PluginRegistryControl for NoopRegistry {
    fn installed_node_kinds(&self) -> plugin_rsi::Result<Vec<String>> {
        Ok(Vec::new())
    }

    fn reload_plugin(&self, _plugin_name: &str) -> plugin_rsi::Result<()> {
        Ok(())
    }
}

async fn execute(
    tools: &[ToolRegistration],
    name: &str,
    input: Value,
) -> Result<ToolResult, ToolError> {
    tools
        .iter()
        .find(|tool| tool.definition.name == name)
        .unwrap()
        .implementation
        .execute(input)
        .await
}

#[tokio::test]
async fn an_installed_reference_can_be_forked_and_locally_activated() {
    let state = tempfile::tempdir().unwrap();
    let requests = RequestStore::open(state.path());
    let store = PluginStore::open(state.path(), "main", "Autonomics RSI", "rsi@example.com");
    common::configure_plugin_vfs(state.path());
    let first_request = requests.record(request("reference-plugin")).unwrap();
    let mut reference = store
        .create(
            "reference-plugin",
            "alpine",
            &[first_request.id],
            "Create the reference adapter.",
            &requests,
            &catalog(),
        )
        .unwrap();

    let profile = AgentProfile::new("reference-agent").unwrap();
    let tools = profile.tool_registrations();
    execute(
        &tools,
        "plugin_node_create",
        json!({
            "plugin_path": "/plugins/dev/reference-plugin",
            "node": node_json("reference_adapter", "scripts/reference.sh")
        }),
    )
    .await
    .unwrap();
    reference
        .workspace()
        .write_text(
            "scripts/reference.sh",
            "#!/bin/sh\nset -eu\ncp \"$AUTONOMICS_INPUT0\" \"$AUTONOMICS_OUTPUT0\"\n",
        )
        .unwrap();
    reference
        .workspace()
        .write_text(
            "README.md",
            "# reference-plugin\n\nA deterministic adapter.\n",
        )
        .unwrap();
    let validation_catalog = catalog();
    let mut reference_lifecycle = PluginLifecycle::new(&mut reference, &validation_catalog, &[]);
    match reference_lifecycle.validate_and_submit().unwrap() {
        ValidationOutcome::Submitted(_) => store.install_local("reference-plugin").unwrap(),
        ValidationOutcome::Passed(_) => panic!("review submission unexpectedly stayed local"),
        ValidationOutcome::NeedsFix(report) => panic!("reference validation failed: {report:?}"),
    };

    let skills = skills::SkillManager::init(skills::SkillManager::new(state.path()));
    let publisher = Arc::new(GhPublisher::new(GhPublisherConfig {
        enabled: false,
        ..Default::default()
    }));
    let infra = RsiInfra::open(
        state.path(),
        "main",
        "Autonomics RSI",
        "rsi@example.com",
        skills,
        catalog(),
        publisher.clone(),
        publisher,
    )
    .unwrap();
    infra.configure_registry(Arc::new(NoopRegistry));
    plugin_rsi::PluginDevelopmentToolsetRegistry::global()
        .configure_infra(Arc::new(infra.clone()))
        .unwrap();
    plugin_rsi::PluginDevelopmentToolsetRegistry::global()
        .configure_environments(EnvironmentRegistry::ephemeral(catalog()))
        .unwrap();
    let profile = AgentProfile::new("fork-agent").unwrap();
    let tools = profile.tool_registrations();
    let fork_result = execute(
        &tools,
        "plugin_fork",
        json!({
            "reference_plugin_path": "/plugins/dev/reference-plugin",
            "plugin_name": "forked-plugin",
            "intent": "new_node",
            "summary": "Fork the local active reference.",
            "body": "Fork the installed adapter and give it a new node kind."
        }),
    )
    .await
    .unwrap();
    let ToolResultContent::Json(fork_output) = fork_result.content else {
        panic!("plugin_fork must return JSON");
    };
    assert_eq!(fork_output["reference_plugin"], "reference-plugin");
    assert_eq!(fork_output["target_plugin"], "forked-plugin");
    assert_eq!(fork_output["target_vfs_path"], "/plugins/dev/forked-plugin");
    let request_id = fork_output["request_id"].as_str().unwrap();
    let fork_request = infra.requests().find(request_id).unwrap().unwrap();
    assert_eq!(fork_request.status, RequestStatus::Working);

    let environment_result = execute(
        &tools,
        "plugin_environment_bind",
        json!({
            "plugin_path": "/plugins/dev/forked-plugin",
            "environment_id": "python"
        }),
    )
    .await
    .unwrap();
    let ToolResultContent::Json(environment_output) = environment_result.content else {
        panic!("plugin_environment_bind must return JSON");
    };
    assert_eq!(environment_output["environment_id"], "python");
    assert_eq!(
        environment_output["environment_reference"],
        PYTHON_REFERENCE
    );

    let mut forked = store.develop("forked-plugin").unwrap().unwrap();
    assert_eq!(forked.status(), PluginStatus::Draft);
    assert_eq!(
        forked.manifest().lifecycle.source_plugin.as_deref(),
        Some("reference-plugin")
    );
    let fork_path = forked.development_vfs_path().unwrap();
    assert_eq!(fork_path, "/plugins/dev/forked-plugin");
    let repository = GitRepo::open(store.root().join("forked-plugin"));
    assert!(repository.remote_url("origin").unwrap().is_none());

    let node_update_result = execute(
        &tools,
        "plugin_node_update",
        json!({
            "plugin_path": "/plugins/dev/forked-plugin",
            "node_kind": "reference_adapter",
            "node": updated_node_json()
        }),
    )
    .await
    .unwrap();
    let ToolResultContent::Json(node_update_output) = node_update_result.content else {
        panic!("plugin_node_update must return JSON");
    };
    assert_eq!(node_update_output["previous_kind"], "reference_adapter");
    assert_eq!(node_update_output["kind"], "forked_adapter");
    forked.refresh().unwrap();
    assert_eq!(forked.owned_node_kinds(), ["forked_adapter"]);
    let updated_node = &forked.manifest().nodes[0];
    assert_eq!(updated_node.ports.inputs.len(), 2);
    assert_eq!(updated_node.ports.outputs.len(), 2);
    assert!(updated_node.params.contains_key("threshold"));
    forked
        .workspace()
        .write_text(
            "scripts/forked.sh",
            "#!/bin/sh\nset -eu\ncp \"$AUTONOMICS_INPUT0\" \"$AUTONOMICS_OUTPUT0\"\ncp \"$AUTONOMICS_INPUT1\" \"$AUTONOMICS_OUTPUT1\"\n",
        )
        .unwrap();
    forked.snapshot("plugin: rename forked node").unwrap();

    let install_result = execute(
        &tools,
        "plugin_install",
        json!({"plugin_path": "/plugins/dev/forked-plugin"}),
    )
    .await
    .unwrap();
    let ToolResultContent::Json(install_output) = install_result.content else {
        panic!("plugin_install must return JSON");
    };
    assert_eq!(install_output["activated"], true);
    assert_eq!(install_output["status"], "draft");
    assert_eq!(install_output["node_kinds"], json!(["forked_adapter"]));
    assert_eq!(install_output["validation_report"]["overall"], "pass");
    assert!(install_output["local_commit"].is_string());
    assert!(install_output["local_digest"].is_string());
    forked.refresh().unwrap();
    assert_eq!(forked.status(), PluginStatus::Draft);
    assert!(forked.manifest().lifecycle.publication_pending);

    let mut revised_node = updated_node_json();
    revised_node["params"]["threshold"]["default"] = json!(0.75);
    execute(
        &tools,
        "plugin_node_update",
        json!({
            "plugin_path": "/plugins/dev/forked-plugin",
            "node_kind": "forked_adapter",
            "node": revised_node
        }),
    )
    .await
    .unwrap();
    forked.refresh().unwrap();
    assert!(!forked.manifest().lifecycle.publication_pending);
    let reinstall_result = execute(
        &tools,
        "plugin_install",
        json!({"plugin_path": "/plugins/dev/forked-plugin"}),
    )
    .await
    .unwrap();
    let ToolResultContent::Json(reinstall_output) = reinstall_result.content else {
        panic!("second plugin_install must return JSON");
    };
    assert_eq!(reinstall_output["activated"], true);
    assert_eq!(reinstall_output["status"], "draft");
    forked.refresh().unwrap();

    let runtime_root = plugin_rsi::PluginStateLayout::v2(state.path()).runtime_root();
    assert!(runtime_root.join("forked-plugin").is_dir());
    assert!(matches!(
        plugin_rsi::read_installed_plugin_source(
            &state.path().join("plugins.toml"),
            "forked-plugin"
        )
        .unwrap(),
        InstalledPluginSource::Local(_)
    ));
    assert!(repository.is_clean().unwrap());
    assert_eq!(forked.status(), PluginStatus::Draft);

    let update_request = requests.record(request("forked-plugin")).unwrap();
    let installed_source = store.installed_source("forked-plugin").unwrap();
    let mut update = store
        .create_update(
            "forked-plugin",
            &[update_request.id],
            "Continue the fast local feedback loop.",
            &requests,
            &installed_source,
            &validation_catalog,
            &plugin_rsi::GitPluginSourceFetcher,
        )
        .unwrap();
    assert_eq!(update.status(), PluginStatus::Updating);
    update
        .workspace()
        .write_text(
            "scripts/forked.sh",
            "#!/bin/sh\nset -eu\ntest -s \"$AUTONOMICS_INPUT0\"\ncp \"$AUTONOMICS_INPUT0\" \"$AUTONOMICS_OUTPUT0\"\n",
        )
        .unwrap();
    update.snapshot("plugin: apply local feedback").unwrap();
    let active_kinds = ["forked_adapter".to_string()];
    let mut update_lifecycle =
        PluginLifecycle::new(&mut update, &validation_catalog, &active_kinds);
    match update_lifecycle.validate_and_submit().unwrap() {
        ValidationOutcome::Submitted(report) => assert!(report.passed(), "{report:?}"),
        ValidationOutcome::Passed(_) => panic!("review submission unexpectedly stayed local"),
        ValidationOutcome::NeedsFix(report) => panic!("update validation failed: {report:?}"),
    };
    store.install_local("forked-plugin").unwrap();
    assert!(
        store
            .pending_distillation()
            .unwrap()
            .iter()
            .any(|manifest| manifest.plugin_name == "forked-plugin")
    );
}
