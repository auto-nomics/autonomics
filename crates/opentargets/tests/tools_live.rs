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
    assert_eq!(tools.len(), 6, "expected 6 Open Targets tools");
    assert!(
        !tools
            .iter()
            .any(|tool| tool.definition.name == "opentargets_search"),
        "opentargets_search must not be registered; use source_opentargets_search"
    );
}

// NOTE: `opentargets_associated_diseases` / `opentargets_associated_targets`
// tool tests were removed with the tools themselves — the ranked association
// tables are served by the DAG nodes `source_opentargets_associated_diseases`
// / `source_opentargets_associated_targets` (see data-engine live tests).

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

#[tokio::test]
#[ignore = "hits the live Open Targets API"]
async fn variant_tool_runs() {
    let text = run_tool(
        "opentargets_variant",
        json!({ "variant_id": "2_655222_G_A" }),
    )
    .await;
    assert!(text.contains("2_655222_G_A"));
    assert!(text.contains("rs13029479"));
}

#[tokio::test]
#[ignore = "hits the live Open Targets API"]
async fn variant_associations_tool_runs() {
    let text = run_tool(
        "opentargets_variant_associations",
        json!({ "variant_id": "2_655222_G_A" }),
    )
    .await;
    assert!(text.contains("trait associations"));
    // Known BMI/weight signal at this locus (Galánin-like peptide region).
    assert!(
        text.to_lowercase().contains("body mass index") || text.to_lowercase().contains("weight"),
        "expected a BMI/weight association in: {text}"
    );
}

#[tokio::test]
#[ignore = "hits the live Open Targets API"]
async fn variant_tool_unknown_id_is_error_result() {
    let reg = registrations()
        .into_iter()
        .find(|r| r.definition.name == "opentargets_variant")
        .unwrap();
    let res = reg
        .implementation
        .execute(json!({ "variant_id": "22_0_IG_NO_RE" }))
        .await
        .expect("tool call failed");
    assert!(
        res.is_error.unwrap_or(false),
        "unknown ID must be an error result"
    );
}
