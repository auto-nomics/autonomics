use std::sync::Arc;

mod common;

use agentik_core::tools::{ToolError, ToolRegistration, ToolResult};
use agentik_sdk::types::ToolResultContent;
use plugin_rsi::{
    AgentProfile, Environment, EnvironmentCatalog, EnvironmentRegistry, GhPublisher,
    GhPublisherConfig, PluginDevelopmentToolsetRegistry, PluginInfra,
};
use serde_json::json;

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

async fn execute(
    tools: &[ToolRegistration],
    name: &str,
    input: serde_json::Value,
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
async fn agent_can_create_an_empty_greenfield_plugin_workspace() {
    let state = tempfile::tempdir().unwrap();
    common::configure_plugin_vfs(state.path());
    let publisher = Arc::new(GhPublisher::new(GhPublisherConfig {
        enabled: false,
        ..Default::default()
    }));
    let environments = EnvironmentRegistry::ephemeral(catalog());
    let infra = Arc::new(
        PluginInfra::open_with_environments(
            state.path(),
            "main",
            "Autonomics RSI",
            "rsi@example.com",
            environments.clone(),
            publisher.clone(),
            publisher,
        )
        .unwrap(),
    );
    PluginDevelopmentToolsetRegistry::global()
        .configure_infra(Arc::clone(&infra))
        .unwrap();
    PluginDevelopmentToolsetRegistry::global()
        .configure_environments(environments)
        .unwrap();

    let profile = AgentProfile::new("greenfield-agent").unwrap();
    let result = execute(
        &profile.tool_registrations(),
        "plugin_create",
        json!({
            "plugin_name": "greenfield-plugin",
            "environment_id": "alpine",
            "purpose": "Provide a deterministic file adapter."
        }),
    )
    .await
    .unwrap();
    let ToolResultContent::Json(output) = result.content else {
        panic!("plugin_create must return JSON");
    };

    assert_eq!(output["plugin_name"], "greenfield-plugin");
    assert_eq!(output["plugin_vfs_path"], "/plugins/dev/greenfield-plugin");
    assert_eq!(output["environment_id"], "alpine");
    assert_eq!(output["environment_reference"], ENVIRONMENT_REFERENCE);
    assert_eq!(output["status"], "draft");

    let store = infra.store();
    let workspace = store
        .develop("greenfield-plugin")
        .unwrap()
        .expect("created workspace");
    assert_eq!(workspace.status(), plugin_rsi::PluginStatus::Draft);
    assert!(workspace.manifest().nodes.is_empty());
    assert_eq!(
        workspace.manifest().lifecycle.rationale.as_deref(),
        Some("Provide a deterministic file adapter.")
    );
    assert!(workspace.manifest().lifecycle.request_ids.is_empty());
}
