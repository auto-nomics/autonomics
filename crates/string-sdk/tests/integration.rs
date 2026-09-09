use std::sync::Arc;

use agentik_core::tools::ToolFunction;
use dag_core::{NodePlugin, NodeRegistry};
use datafusion::execution::runtime_env::RuntimeEnv;

use string_sdk::StringDbClient;
use string_sdk::nodes::Plugin;
use string_sdk::tools::{
    FunctionalEnrichmentTool, NetworkImageTool, NetworkInteractionsTool, NetworkSummaryTool,
    ResolveIdentifiersTool,
};

fn client() -> Arc<StringDbClient> {
    Arc::new(
        StringDbClient::builder()
            .caller_identity("string-sdk-tests")
            .disable_rate_limit()
            .build()
            .unwrap(),
    )
}

#[test]
fn tool_registrations_expose_unique_definitions() {
    let registrations = string_sdk::string_registrations(client());
    let names: Vec<_> = registrations
        .iter()
        .map(|registration| registration.definition.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec![
            "string_resolve_identifiers",
            "string_network_interactions",
            "string_functional_enrichment",
            "string_network_summary",
            "string_network_image",
        ]
    );
    for registration in registrations {
        assert!(!registration.definition.description.is_empty());
        assert_eq!(registration.definition.input_schema.schema_type, "object");
    }
}

#[tokio::test]
async fn tools_validate_inputs_before_network_calls() {
    let client = client();
    let resolve = ResolveIdentifiersTool::new(client.clone());
    let error = resolve
        .execute(serde_json::json!({"identifiers": []}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("at least one"));

    let network = NetworkInteractionsTool::new(client.clone());
    let error = network
        .execute(serde_json::json!({
            "identifiers": ["TP53"],
            "network_type": "metabolic"
        }))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("network_type"));

    let enrichment = FunctionalEnrichmentTool::new(client.clone());
    let error = enrichment
        .execute(serde_json::json!({"identifiers": []}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("at least one"));

    let summary = NetworkSummaryTool::new(client.clone());
    let error = summary
        .execute(serde_json::json!({"identifiers": ["TP53"]}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("at least two"));

    let image = NetworkImageTool::new(client);
    let error = image
        .execute(serde_json::json!({
            "identifiers": ["TP53"],
            "image_format": "pdf"
        }))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("image_format"));
}

#[test]
fn node_plugin_registers_and_builds_all_sources() {
    let runtime = RuntimeEnv::default();
    let mut registry = NodeRegistry::with_ingredients(Arc::new(runtime), None);
    registry.register_plugin(&Plugin);

    let fixture = Plugin.fixture_spec("source_string_id_map").unwrap();
    for kind in [
        "source_string_id_map",
        "source_string_network",
        "source_string_enrichment",
        "source_string_ppi_enrichment",
    ] {
        let node = registry
            .build_node(kind, fixture.clone())
            .unwrap_or_else(|error| panic!("failed to build {kind}: {error}"));
        assert_eq!(node.kind(), kind);
        assert_eq!(node.ports().output_ports().len(), 1);
    }
}
