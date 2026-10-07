use agentik_core::tools::{ToolError, ToolRegistration, ToolResult};
mod common;

use plugin_rsi::GitRepo;
use plugin_rsi::{
    AgentProfile, Environment, EnvironmentCatalog, InstalledPluginSource, PluginLifecycle,
    PluginStatus, PluginStore, RequestIntent, RequestRecord, RequestSource, RequestStatus,
    RequestStore, ValidationOutcome,
};
use serde_json::{Value, json};

const ENVIRONMENT_REFERENCE: &str = "docker.io/library/alpine@sha256:0123456789012345678901234567890123456789012345678901234567890123";

fn catalog() -> EnvironmentCatalog {
    let mut catalog = EnvironmentCatalog::default();
    catalog.insert(
        "alpine",
        Environment {
            reference: ENVIRONMENT_REFERENCE.into(),
            interpreters: vec!["sh".into()],
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
    execute(
        &tools,
        "plugin_node_write_script",
        json!({
            "plugin_path": "/plugins/dev/reference-plugin",
            "node_kind": "reference_adapter",
            "contents": "#!/bin/sh\nset -eu\ncp \"$AUTONOMICS_INPUT0\" \"$AUTONOMICS_OUTPUT0\"\n"
        }),
    )
    .await
    .unwrap();
    execute(
        &tools,
        "plugin_workspace_write",
        json!({
            "plugin_path": "/plugins/dev/reference-plugin",
            "path": "README.md",
            "contents": "# reference-plugin\n\nA deterministic adapter.\n"
        }),
    )
    .await
    .unwrap();
    let validation_catalog = catalog();
    let mut reference_lifecycle = PluginLifecycle::new(&mut reference, &validation_catalog, &[]);
    match reference_lifecycle.validate_and_submit().unwrap() {
        ValidationOutcome::Submitted(_) => store.install_local("reference-plugin").unwrap(),
        ValidationOutcome::NeedsFix(report) => panic!("reference validation failed: {report:?}"),
    };

    let fork_request = requests.record(request("forked-plugin")).unwrap();
    let mut forked = store
        .fork(
            "reference-plugin",
            "forked-plugin",
            &[fork_request.id],
            "Fork the local active reference.",
            &requests,
            &catalog(),
        )
        .unwrap();
    assert_eq!(forked.status(), PluginStatus::Draft);
    assert_eq!(
        forked.manifest().lifecycle.source_plugin.as_deref(),
        Some("reference-plugin")
    );
    let fork_path = forked.development_vfs_path().unwrap();
    assert_eq!(fork_path, "/plugins/dev/forked-plugin");
    let first_fork_agent = AgentProfile::new("fork-agent-1").unwrap();
    let second_fork_agent = AgentProfile::new("fork-agent-2").unwrap();
    for profile in [first_fork_agent, second_fork_agent] {
        let tools = profile.tool_registrations();
        execute(
            &tools,
            "plugin_development_status",
            json!({ "plugin_path": fork_path }),
        )
        .await
        .unwrap();
    }
    let repository = GitRepo::open(store.root().join("forked-plugin"));
    assert!(repository.remote_url("origin").unwrap().is_none());

    let manifest_path = store.root().join("forked-plugin/manifest.toml");
    let manifest_text = std::fs::read_to_string(&manifest_path).unwrap();
    let manifest_text = manifest_text
        .replace("reference_adapter", "forked_adapter")
        .replace("scripts/reference.sh", "scripts/forked.sh");
    std::fs::write(&manifest_path, manifest_text).unwrap();
    forked
        .workspace()
        .write_text(
            "scripts/forked.sh",
            "#!/bin/sh\nset -eu\ncp \"$AUTONOMICS_INPUT0\" \"$AUTONOMICS_OUTPUT0\"\n",
        )
        .unwrap();
    forked.snapshot("plugin: rename forked node").unwrap();

    let installed_kinds = ["reference_adapter".to_string()];
    let mut fork_lifecycle =
        PluginLifecycle::new(&mut forked, &validation_catalog, &installed_kinds);
    match fork_lifecycle.validate_and_submit().unwrap() {
        ValidationOutcome::Submitted(report) => assert!(report.passed(), "{report:?}"),
        ValidationOutcome::NeedsFix(report) => panic!("fork validation failed: {report:?}"),
    };
    store.install_local("forked-plugin").unwrap();

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
    assert_eq!(forked.status(), PluginStatus::PendingReview);

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
