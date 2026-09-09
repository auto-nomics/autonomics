//! Live API end-to-end coverage. Run explicitly:
//! `cargo test -p string-sdk --test e2e -- --ignored`

use std::sync::Arc;

use agentik_core::tools::ToolFunction;
use arrow_array::{Array, Float64Array, RecordBatch, StringArray, UInt64Array};
use dag_core::dag::DagNode;
use dag_core::dag::node_event::NodeReporter;
use dag_core::registry::{NodeCtx, NodeRegistry};
use datafusion::execution::runtime_env::RuntimeEnv;

use string_sdk::StringDbClient;
use string_sdk::nodes::Plugin;
use string_sdk::tools::{NetworkImageTool, NetworkSummaryTool, ResolveIdentifiersTool};

fn node_ctx() -> NodeCtx {
    NodeCtx::new(Arc::new(RuntimeEnv::default()), None)
}

async fn batches(df: &datafusion::dataframe::DataFrame) -> Vec<RecordBatch> {
    df.clone().collect().await.unwrap()
}

fn column<'a, A: Array + 'static>(batches: &'a [RecordBatch], name: &str) -> &'a A {
    batches[0].schema().field_with_name(name).unwrap();
    batches[0]
        .column_by_name(name)
        .unwrap()
        .as_any()
        .downcast_ref::<A>()
        .unwrap()
}

#[tokio::test]
#[ignore = "uses the public STRING API"]
async fn dag_nodes_fetch_and_materialize_string_tables() {
    let runtime = Arc::new(RuntimeEnv::default());
    let mut registry = NodeRegistry::with_ingredients(runtime, None);
    registry.register_plugin(&Plugin);
    let spec = serde_json::json!({
        "identifiers": ["TP53", "CDK2"],
        "species": "9606",
    });

    let mut mapping = registry
        .build_node("source_string_id_map", spec.clone())
        .unwrap();
    let outputs = mapping
        .execute(&node_ctx(), &[], &NodeReporter::noop())
        .await
        .unwrap();
    let rows = batches(outputs.dataframe(0).unwrap()).await;
    let string_ids = column::<StringArray>(&rows, "string_id");
    assert_eq!(rows[0].num_rows(), 2);
    for index in 0..2 {
        assert!(string_ids.value(index).starts_with("9606."));
    }

    let mut network = registry
        .build_node("source_string_network", spec.clone())
        .unwrap();
    let outputs = network
        .execute(&node_ctx(), &[], &NodeReporter::noop())
        .await
        .unwrap();
    let rows = batches(outputs.dataframe(0).unwrap()).await;
    assert!(!rows.is_empty() && rows[0].num_rows() > 0);
    let scores = column::<Float64Array>(&rows, "score");
    assert!((0..scores.len()).any(|index| scores.value(index) > 0.0));

    let mut enrichment = registry
        .build_node("source_string_enrichment", spec.clone())
        .unwrap();
    let outputs = enrichment
        .execute(&node_ctx(), &[], &NodeReporter::noop())
        .await
        .unwrap();
    let rows = batches(outputs.dataframe(0).unwrap()).await;
    assert!(!rows.is_empty() && rows[0].num_rows() > 0);
    let fdr = column::<Float64Array>(&rows, "fdr");
    assert!((0..fdr.len()).all(|index| fdr.value(index) >= 0.0));

    let mut ppi = registry
        .build_node("source_string_ppi_enrichment", spec)
        .unwrap();
    let outputs = ppi
        .execute(&node_ctx(), &[], &NodeReporter::noop())
        .await
        .unwrap();
    let rows = batches(outputs.dataframe(0).unwrap()).await;
    assert_eq!(rows[0].num_rows(), 1);
    let nodes = column::<UInt64Array>(&rows, "nodes");
    assert_eq!(nodes.value(0), 2);
}

#[tokio::test]
#[ignore = "uses the public STRING API"]
async fn agent_tools_resolve_summarize_and_preview() {
    let client = Arc::new(
        StringDbClient::builder()
            .caller_identity("string-sdk-e2e")
            .build()
            .unwrap(),
    );

    let resolver = ResolveIdentifiersTool::new(client.clone());
    let result = resolver
        .execute(serde_json::json!({
            "identifiers": ["TP53", "CDK2"],
            "species": "9606"
        }))
        .await
        .unwrap();
    assert!(result.text_content().contains("9606.ENSP"));

    let summary = NetworkSummaryTool::new(client.clone());
    let result = summary
        .execute(serde_json::json!({
            "identifiers": ["TP53", "CDK2"],
            "species": "9606"
        }))
        .await
        .unwrap();
    let summary = result.text_content();
    assert!(summary.contains("STRING network summary"));
    assert!(summary.contains("Top functional terms"));

    let image = NetworkImageTool::new(client);
    let result = image
        .execute(serde_json::json!({
            "identifiers": ["TP53", "CDK2"],
            "species": "9606",
            "image_format": "png"
        }))
        .await
        .unwrap();
    let agentik_sdk::types::ToolResultContent::Blocks(blocks) = result.content else {
        panic!("network image tool should return image blocks");
    };
    assert!(blocks.iter().any(|block| {
        matches!(
            block,
            agentik_sdk::types::ToolResultBlock::Image { source: _ }
        )
    }));
}
