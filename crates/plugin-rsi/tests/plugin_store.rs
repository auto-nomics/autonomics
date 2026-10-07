use agentik_core::tools::{ToolError, ToolRegistration};
mod common;

use plugin_rsi::{
    AgentProfile, Environment, EnvironmentCatalog, PluginStatus, PluginStore, RequestIntent,
    RequestRecord, RequestSource, RequestStatus, RequestStore,
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

fn request() -> RequestRecord {
    RequestRecord {
        id: String::new(),
        created_at: 0,
        source: RequestSource::User,
        intent: RequestIntent::NewNode,
        summary: "Create a direct-development adapter".into(),
        body: "Copy the input file to the declared output.".into(),
        plugin_name: Some("direct-plugin".into()),
        evidence_ids: Vec::new(),
        status: RequestStatus::Open,
    }
}

fn node_json() -> Value {
    json!({
        "kind": "direct_adapter",
        "desc": "Copy one file",
        "doc": "Copies input 0 to output 0.",
        "ports": {
            "inputs": [{ "type": "file", "label": "input" }],
            "outputs": [{ "path": "result.txt", "format": "txt" }]
        },
        "command": {
            "interpreter": "sh",
            "argv": [],
            "script_file": "scripts/adapter.sh",
            "env": {},
            "files": {}
        }
    })
}

async fn execute(
    tools: &[ToolRegistration],
    name: &str,
    input: Value,
) -> Result<agentik_sdk::types::ToolResult, ToolError> {
    tools
        .iter()
        .find(|tool| tool.definition.name == name)
        .unwrap()
        .implementation
        .execute(input)
        .await
}

#[tokio::test]
async fn plugin_store_develops_one_repository_in_place() {
    let state = tempfile::tempdir().unwrap();
    let requests = RequestStore::open(state.path());
    let request = requests.record(request()).unwrap();
    let layout = plugin_rsi::PluginStateLayout::v2(state.path());
    let workspace_root = layout.workspace_root();
    let store = PluginStore::open_with_layout(layout, "main", "Autonomics RSI", "rsi@example.com");
    common::configure_plugin_vfs(state.path());
    let mut operator = store
        .create(
            "direct-plugin",
            "alpine",
            &[request.id],
            "Create the first direct-workspace plugin.",
            &requests,
            &catalog(),
        )
        .unwrap();

    let valid_manifest =
        std::fs::read_to_string(workspace_root.join("direct-plugin/manifest.toml")).unwrap();
    std::fs::create_dir_all(workspace_root.join("orphan-name")).unwrap();
    std::fs::write(
        workspace_root.join("orphan-name/manifest.toml"),
        valid_manifest.replace(
            "plugin_name = \"direct-plugin\"",
            "plugin_name = \"other-plugin\"",
        ),
    )
    .unwrap();
    std::fs::create_dir_all(workspace_root.join("broken-plugin")).unwrap();
    std::fs::write(
        workspace_root.join("broken-plugin/manifest.toml"),
        "not valid toml",
    )
    .unwrap();

    assert_eq!(operator.status(), PluginStatus::Draft);
    assert_eq!(
        store
            .list()
            .unwrap()
            .into_iter()
            .map(|manifest| manifest.plugin_name)
            .collect::<Vec<_>>(),
        vec!["direct-plugin".to_string()]
    );
    let orphan_error = store.develop("orphan-name").unwrap_err();
    assert!(
        orphan_error
            .to_string()
            .contains("does not match workspace"),
        "{orphan_error}"
    );
    assert_eq!(
        store.development_vfs_path("direct-plugin").unwrap(),
        "/plugins/dev/direct-plugin"
    );

    let profile = AgentProfile::new("direct-plugin-agent").unwrap();
    let tools = profile.tool_registrations();
    execute(
        &tools,
        "plugin_node_create",
        json!({
            "plugin_path": "/plugins/dev/direct-plugin",
            "node": node_json()
        }),
    )
    .await
    .unwrap();
    operator
        .workspace()
        .write_text(
            "scripts/adapter.sh",
            "#!/bin/sh\ncp \"$AUTONOMICS_INPUT0\" \"$AUTONOMICS_OUTPUT0\"\n",
        )
        .unwrap();

    operator.refresh().unwrap();
    assert_eq!(operator.manifest().nodes.len(), 1);
    assert!(operator.workspace().read_text("scripts/adapter.sh").is_ok());

    let commit = operator
        .snapshot("plugin: direct development snapshot")
        .unwrap()
        .unwrap();
    assert_eq!(commit.len(), 40);
}
