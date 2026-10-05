use agentik_core::tools::{ToolError, ToolRegistration};
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
    let store = PluginStore::open(state.path(), "main", "Autonomics RSI", "rsi@example.com");
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

    assert_eq!(operator.status(), PluginStatus::Draft);
    assert_eq!(store.list().unwrap().len(), 1);

    let profile = AgentProfile::new("direct-plugin-agent").unwrap();
    profile
        .bind_direct_plugin(&mut operator, "direct-run-1")
        .unwrap();
    let tools = profile.tool_registrations();
    execute(&tools, "plugin_node_create", json!({ "node": node_json() }))
        .await
        .unwrap();
    execute(
        &tools,
        "plugin_node_write_script",
        json!({
            "node_kind": "direct_adapter",
            "contents": "#!/bin/sh\ncp \"$AUTONOMICS_INPUT0\" \"$AUTONOMICS_OUTPUT0\"\n"
        }),
    )
    .await
    .unwrap();

    operator.refresh().unwrap();
    assert_eq!(operator.manifest().nodes.len(), 1);
    assert!(operator.workspace().read_text("scripts/adapter.sh").is_ok());

    operator.transition(PluginStatus::Validating).unwrap();
    execute(
        &tools,
        "plugin_node_spec",
        json!({ "node_kind": "direct_adapter" }),
    )
    .await
    .unwrap();
    assert!(
        execute(
            &tools,
            "plugin_workspace_write",
            json!({
                "path": "README.md",
                "contents": "# direct-plugin\n"
            }),
        )
        .await
        .is_err()
    );

    let commit = operator
        .snapshot("plugin: direct development snapshot")
        .unwrap()
        .unwrap();
    assert_eq!(commit.len(), 40);
}
