//! Tests for the Open Targets **agent tool** layer.
//!
//! Registration tests run offline. The remaining live tests exercise JSON
//! deserialization → SDK call → Markdown formatting. They hit the real API
//! and are `#[ignore]` by default:
//!
//! ```sh
//! cargo test -p opentargets -- --ignored --test-threads=1
//! ```

use std::sync::Arc;

use agentik_core::tools::ToolRegistration;
use opentargets::{OpenTargetsClient, opentargets_registrations};
use serde_json::json;

fn registrations() -> Vec<ToolRegistration> {
    opentargets_registrations(Arc::new(OpenTargetsClient::new()))
}

/// Find a registered tool by name and run it with a JSON input.
async fn run_tool(name: &str, input: serde_json::Value) -> String {
    let reg = registrations()
        .into_iter()
        .find(|r| r.definition.name == name)
        .unwrap_or_else(|| panic!("tool '{name}' not registered"));
    let res = reg
        .implementation
        .execute(input)
        .await
        .expect("tool failed");
    assert!(
        !res.is_error.unwrap_or(false),
        "tool {name} returned an error: {}",
        res.text_content()
    );
    res.text_content()
}

#[test]
fn registrations_exclude_migrated_search() {
    let tools = registrations();
    assert_eq!(tools.len(), 5, "expected 5 Open Targets tools");
    assert!(
        !tools
            .iter()
            .any(|tool| tool.definition.name == "opentargets_search"),
        "opentargets_search must not be registered; use source_opentargets_search"
    );
}

#[tokio::test]
#[ignore = "hits the live Open Targets API"]
async fn associated_diseases_tool_runs() {
    let text = run_tool(
        "opentargets_associated_diseases",
        json!({
            "ensembl_id": "ENSG00000012048",
            "size": 5,
            "b_filter": "breast",
            "min_score": 0.1
        }),
    )
    .await;
    assert!(text.contains("Score"));
    assert!(text.to_lowercase().contains("breast"));
}

#[tokio::test]
#[ignore = "hits the live Open Targets API"]
async fn associated_diseases_tool_fetch_all() {
    let text = run_tool(
        "opentargets_associated_diseases",
        json!({
            "ensembl_id": "ENSG00000012048",
            "b_filter": "cancer",
            "min_score": 0.4,
            "fetch_all": true
        }),
    )
    .await;
    assert!(text.contains("associated diseases"));
}

#[tokio::test]
#[ignore = "hits the live Open Targets API"]
async fn target_tool_runs() {
    let text = run_tool(
        "opentargets_target",
        json!({ "ensembl_id": "ENSG00000012048" }),
    )
    .await;
    assert!(text.contains("BRCA1"));
}

#[tokio::test]
#[ignore = "hits the live Open Targets API"]
async fn disease_tool_runs() {
    let text = run_tool("opentargets_disease", json!({ "efo_id": "MONDO_0004975" })).await;
    assert!(text.to_lowercase().contains("alzheimer"));
}
