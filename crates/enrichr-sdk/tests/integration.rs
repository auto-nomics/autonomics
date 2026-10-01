//! Offline integration tests: registration shape, input validation, and DAG
//! plugin wiring without network access.

use std::sync::Arc;

use agentik_core::tools::ToolFunction;
use dag_core::dag::DagNode;
use dag_core::dag::node_event::NodeReporter;
use dag_core::{NodePlugin, NodeRegistry};
use datafusion::execution::runtime_env::RuntimeEnv;

use enrichr_sdk::EnrichrClient;
use enrichr_sdk::nodes::Plugin;
use enrichr_sdk::tools::{AddListTool, BackgroundEnrichTool, EnrichTool};

fn client() -> Arc<EnrichrClient> {
    Arc::new(
        EnrichrClient::builder()
            .disable_rate_limit()
            .build()
            .unwrap(),
    )
}

#[test]
fn tool_registrations_expose_unique_definitions() {
    let registrations = enrichr_sdk::enrichr_registrations(client());
    let names: Vec<_> = registrations
        .iter()
        .map(|registration| registration.definition.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec![
            "enrichr_libraries",
            "enrichr_add_list",
            "enrichr_enrich",
            "enrichr_view_list",
            "enrichr_background_enrich",
            "enrichr_gene_map",
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

    let add_list = AddListTool::new(client.clone());
    let error = add_list
        .execute(serde_json::json!({"genes": []}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("at least one"));

    let enrich = EnrichTool::new(client.clone());
    let error = enrich
        .execute(serde_json::json!({"background_type": "KEGG_2021_Human"}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("either genes or user_list_id"));
    let error = enrich
        .execute(serde_json::json!({
            "genes": ["TP53"],
            "user_list_id": 123,
            "background_type": "KEGG_2021_Human"
        }))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("not both"));

    let background = BackgroundEnrichTool::new(client);
    let error = background
        .execute(serde_json::json!({
            "genes": ["TP53"],
            "background_genes": [],
            "background_type": "KEGG_2021_Human"
        }))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("background_genes"));
}

#[tokio::test]
async fn background_node_validates_genes_before_network_calls() {
    let runtime = Arc::new(RuntimeEnv::default());
    let mut registry = NodeRegistry::with_ingredients(runtime, None);
    registry.register_plugin(&Plugin);

    let mut node = registry
        .build_node(
            "source_enrichr_background_enrich",
            serde_json::json!({
                "genes": [],
                "background_genes": ["TP53"],
                "background_type": "KEGG_2021_Human",
            }),
        )
        .unwrap();
    let ctx = dag_core::registry::NodeCtx::new(Arc::new(RuntimeEnv::default()), None);
    let error = node
        .execute(&ctx, &[], &NodeReporter::noop())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("genes"));

    let mut node = registry
        .build_node(
            "source_enrichr_background_enrich",
            serde_json::json!({
                "genes": ["TP53"],
                "background_genes": [],
                "background_type": "KEGG_2021_Human",
            }),
        )
        .unwrap();
    let error = node
        .execute(&ctx, &[], &NodeReporter::noop())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("background_genes"));

    let mut node = registry
        .build_node(
            "source_enrichr_background_enrich",
            serde_json::json!({
                "genes": ["TP53"],
                "background_genes": ["TP53", "BRCA1"],
                "background_type": " "
            }),
        )
        .unwrap();
    let error = node
        .execute(&ctx, &[], &NodeReporter::noop())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("background_type"));
}

#[test]
fn node_plugin_registers_and_builds_all_sources() {
    let runtime = Arc::new(RuntimeEnv::default());
    let mut registry = NodeRegistry::with_ingredients(runtime, None);
    registry.register_plugin(&Plugin);

    let spec = serde_json::json!({
        "genes": ["TP53", "BRCA1", "EGFR"],
        "background_type": "KEGG_2021_Human",
    });
    for kind in [
        "source_enrichr_enrich",
        "source_enrichr_libraries",
        "source_enrichr_view_list",
        "source_enrichr_genemap",
        "source_enrichr_background_enrich",
    ] {
        let spec = match kind {
            "source_enrichr_view_list" => serde_json::json!({"user_list_id": 1}),
            "source_enrichr_genemap" => serde_json::json!({"gene": "TP53"}),
            "source_enrichr_libraries" => serde_json::json!({"query": "KEGG"}),
            "source_enrichr_background_enrich" => serde_json::json!({
                "genes": ["TP53", "BRCA1", "EGFR"],
                "background_genes": [
                    "TP53", "BRCA1", "EGFR", "MYC", "PTEN", "AKT1", "KRAS"
                ],
                "background_type": "KEGG_2021_Human",
            }),
            _ => spec.clone(),
        };
        let node = registry
            .build_node(kind, spec)
            .unwrap_or_else(|error| panic!("failed to build {kind}: {error}"));
        assert_eq!(node.kind(), kind);
        assert_eq!(node.ports().output_ports().len(), 1);
    }
}
