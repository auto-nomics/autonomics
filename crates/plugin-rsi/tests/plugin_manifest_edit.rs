//! plugin_node_update / plugin_node_create manifest-editing guarantees: a
//! valid edit lands (and resets a pending local activation); every rejected
//! edit — not-found, ambiguity, invalid TOML, daemon-owned field tampering,
//! node-set changes, duplicate kinds, manifest that no longer compiles —
//! leaves manifest.toml byte-identical.

use std::sync::Mutex;

mod common;

use agentik_core::tools::{ToolError, ToolRegistration, ToolResult};
use agentik_sdk::types::ToolResultContent;
use plugin_rsi::{
    AgentProfile, Environment, EnvironmentCatalog, PluginStateLayout, PluginStore, RequestIntent,
    RequestRecord, RequestSource, RequestStatus, RequestStore,
};
use serde_json::{Value, json};

const ENVIRONMENT_REFERENCE: &str = "docker.io/library/alpine@sha256:0123456789012345678901234567890123456789012345678901234567890123";

/// The process-global toolset registry is shared state; reconfigure per test
/// and run this binary's tests sequentially.
static SEQUENTIAL: Mutex<()> = Mutex::new(());

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

fn request(plugin_name: &str, summary: &str) -> RequestRecord {
    RequestRecord {
        id: String::new(),
        created_at: 0,
        source: RequestSource::User,
        intent: RequestIntent::NewNode,
        summary: summary.into(),
        body: "Provide the plugin with the requested node.".into(),
        plugin_name: Some(plugin_name.into()),
        evidence_ids: Vec::new(),
        status: RequestStatus::Open,
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

async fn json_result(tools: &[ToolRegistration], name: &str, input: Value) -> Value {
    let result = execute(tools, name, input).await.unwrap();
    let ToolResultContent::Json(value) = result.content else {
        panic!("tool `{name}` must return JSON");
    };
    value
}

fn read_manifest(operator: &plugin_rsi::PluginOperator<'_>) -> String {
    operator.workspace().read_text("manifest.toml").unwrap()
}

const SAMPLE_ADAPTER_TOML: &str = "\
kind = \"sample_adapter\"
desc = \"Copy one file\"
doc = \"Copies input 0 to output 0.\"

[ports]
inputs = [{ type = \"file\", label = \"input\" }]
outputs = [{ path = \"result.txt\", format = \"txt\" }]

[params]
alpha = { type = \"string\", default = \"x\", doc = \"Shared note.\" }
beta = { type = \"string\", default = \"y\", doc = \"Shared note.\" }

[command]
interpreter = \"sh\"
argv = []
script_file = \"scripts/adapter.sh\"
";

const OTHER_ADAPTER_TOML: &str = "\
kind = \"other_adapter\"
desc = \"Copy another file\"
doc = \"Copies input 0 to output 0.\"

[ports]
inputs = []
outputs = [{ path = \"out.txt\", format = \"txt\" }]

[params]
gamma = { type = \"number\", default = 0.5 }

[command]
interpreter = \"sh\"
argv = []
script_file = \"scripts/other.sh\"
";

#[tokio::test]
async fn manifest_edits_are_validated_before_they_land() {
    let _guard = SEQUENTIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let state = tempfile::tempdir().unwrap();
    let requests = RequestStore::open(state.path());
    let recorded = requests
        .record(request(
            "manifest-plugin",
            "Create a manifest editing playground",
        ))
        .unwrap();
    common::configure_plugin_vfs(state.path());
    let layout = PluginStateLayout::v2(state.path());
    let store = PluginStore::open_with_layout(layout, "main", "Autonomics RSI", "rsi@example.com");
    let mut operator = store
        .create(
            "manifest-plugin",
            "alpine",
            std::slice::from_ref(&recorded.id),
            "Create the manifest editing playground plugin.",
            &requests,
            &catalog(),
        )
        .unwrap();

    let profile = AgentProfile::new("manifest-edit-agent").unwrap();
    let tools = profile.tool_registrations();
    let plugin_path = "/plugins/dev/manifest-plugin";
    json_result(
        &tools,
        "plugin_node_create",
        json!({ "plugin_path": plugin_path, "node_toml": SAMPLE_ADAPTER_TOML }),
    )
    .await;
    json_result(
        &tools,
        "plugin_node_create",
        json!({ "plugin_path": plugin_path, "node_toml": OTHER_ADAPTER_TOML }),
    )
    .await;
    operator.refresh().unwrap();
    assert_eq!(
        operator
            .manifest()
            .nodes
            .iter()
            .map(|node| node.kind.as_str())
            .collect::<Vec<_>>(),
        ["sample_adapter", "other_adapter"]
    );

    // Simulate a pending local activation; any successful edit must reset it.
    let pending = read_manifest(&operator)
        .replace("publication_pending = false", "publication_pending = true");
    assert_ne!(pending, read_manifest(&operator));
    operator
        .workspace()
        .write_text("manifest.toml", &pending)
        .unwrap();

    // Repeated identical doc lines are ambiguous without replace_all...
    let ambiguous = execute(
        &tools,
        "plugin_node_update",
        json!({
            "plugin_path": plugin_path,
            "old_string": "doc = \"Shared note.\"",
            "new_string": "doc = \"Reviewed note.\"",
        }),
    )
    .await
    .unwrap_err();
    match ambiguous {
        ToolError::ValidationFailed { message } => {
            assert!(
                message.contains('2'),
                "must report the match count: {message}"
            );
        }
        other => panic!("repeated pattern must be ambiguous: {other:?}"),
    }
    assert_eq!(
        read_manifest(&operator),
        pending,
        "ambiguous edit must not land"
    );

    // ...and replace_all rewrites every match.
    let value = json_result(
        &tools,
        "plugin_node_update",
        json!({
            "plugin_path": plugin_path,
            "old_string": "doc = \"Shared note.\"",
            "new_string": "doc = \"Reviewed note.\"",
            "replace_all": true,
        }),
    )
    .await;
    assert_eq!(value["node_count"], json!(2), "{value:?}");
    assert_eq!(
        read_manifest(&operator).matches("Reviewed note.").count(),
        2
    );
    operator.refresh().unwrap();
    assert!(
        !operator.manifest().lifecycle.publication_pending,
        "a manifest edit must invalidate pending local activation"
    );

    // Rejections leave the file byte-identical.
    let original = read_manifest(&operator);
    let sample_block = {
        let text = read_manifest(&operator);
        let start = text.find("[[nodes]]").unwrap();
        let end = text[start + 1..].find("[[nodes]]").unwrap() + start + 1;
        text[start..end].trim_end().to_string()
    };
    assert!(sample_block.contains("kind = \"sample_adapter\""));
    let rejects: Vec<(&str, Value)> = vec![
        (
            "not found",
            json!({
                "plugin_path": plugin_path,
                "old_string": "NONSENSE-NOT-IN-FILE",
                "new_string": "x",
            }),
        ),
        (
            "invalid toml",
            json!({
                "plugin_path": plugin_path,
                "old_string": "kind = \"sample_adapter\"",
                "new_string": "kind = ",
            }),
        ),
        (
            "status tamper",
            json!({
                "plugin_path": plugin_path,
                "old_string": "status = \"draft\"",
                "new_string": "status = \"approved\"",
            }),
        ),
        (
            "lifecycle tamper",
            json!({
                "plugin_path": plugin_path,
                "old_string": "rationale = \"Create the manifest editing playground plugin.\"",
                "new_string": "rationale = \"tampered\"",
            }),
        ),
        (
            "node removal",
            json!({
                "plugin_path": plugin_path,
                "old_string": sample_block,
                "new_string": "",
            }),
        ),
        (
            "kind duplicate",
            json!({
                "plugin_path": plugin_path,
                "old_string": "kind = \"other_adapter\"",
                "new_string": "kind = \"sample_adapter\"",
            }),
        ),
        (
            "manifest stops compiling",
            json!({
                "plugin_path": plugin_path,
                "old_string": "default = 0.5",
                "new_string": "optional = false",
            }),
        ),
    ];
    for (hint, input) in rejects {
        let error = execute(&tools, "plugin_node_update", input)
            .await
            .unwrap_err();
        assert!(
            matches!(error, ToolError::ValidationFailed { .. }),
            "{hint} must be a validation rejection: {error:?}"
        );
        assert_eq!(
            read_manifest(&operator),
            original,
            "{hint} rejection must leave manifest.toml untouched"
        );
    }

    // A collision-free rename lands.
    let value = json_result(
        &tools,
        "plugin_node_update",
        json!({
            "plugin_path": plugin_path,
            "old_string": "kind = \"other_adapter\"",
            "new_string": "kind = \"renamed_adapter\"",
        }),
    )
    .await;
    assert_eq!(
        value["node_kinds"],
        json!(["sample_adapter", "renamed_adapter"]),
        "{value:?}"
    );

    // plugin_node_create rejects an existing kind.
    let error = execute(
        &tools,
        "plugin_node_create",
        json!({
            "plugin_path": plugin_path,
            "node_toml": SAMPLE_ADAPTER_TOML,
        }),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(error, ToolError::ValidationFailed { .. }),
        "duplicate kind must be rejected: {error:?}"
    );
}
