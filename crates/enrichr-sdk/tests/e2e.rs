//! Live end-to-end coverage through agent tools and DAG nodes. Run
//! explicitly: `cargo test -p enrichr-sdk --test e2e -- --ignored`
//!
//! The scenarios share one test function so every HTTP call is strictly
//! sequential: Enrichr's front nginx rejects bursts per IP, and DAG nodes
//! construct their own clients (hence their own pacers), so parallel tests
//! can exceed the burst window even with per-client one-second pacing.

use std::sync::Arc;

use agentik_core::tools::ToolFunction;
use arrow_array::{Array, Float64Array, RecordBatch, StringArray, UInt64Array};
use dag_core::dag::DagNode;
use dag_core::dag::node_event::NodeReporter;
use dag_core::registry::{NodeCtx, NodeRegistry};
use datafusion::execution::runtime_env::RuntimeEnv;

use enrichr_sdk::EnrichrClient;
use enrichr_sdk::nodes::Plugin;
use enrichr_sdk::tools::{AddListTool, EnrichTool, GeneMapTool, LibrariesTool};

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

async fn dag_nodes_fetch_and_materialize_enrichr_tables() {
    let runtime = Arc::new(RuntimeEnv::default());
    let mut registry = NodeRegistry::with_ingredients(runtime, None);
    registry.register_plugin(&Plugin);

    let mut enrichment = registry
        .build_node(
            "source_enrichr_enrich",
            serde_json::json!({
                "genes": ["TP53", "BRCA1", "EGFR", "MYC", "PTEN"],
                "background_type": "KEGG_2021_Human",
            }),
        )
        .unwrap();
    let outputs = enrichment
        .execute(&node_ctx(), &[], &NodeReporter::noop())
        .await
        .unwrap();
    let rows = batches(outputs.dataframe(0).unwrap()).await;
    assert!(!rows.is_empty() && rows[0].num_rows() > 0);
    let library = column::<StringArray>(&rows, "library");
    assert_eq!(library.value(0), "KEGG_2021_Human");
    let adj_p = column::<Float64Array>(&rows, "adjusted_p_value");
    assert!((0..adj_p.len()).all(|index| adj_p.value(index) >= 0.0));
    let genes = column::<StringArray>(&rows, "overlapping_genes");
    assert!(genes.value(0).contains(','));

    let mut libraries = registry
        .build_node(
            "source_enrichr_libraries",
            serde_json::json!({"query": "KEGG"}),
        )
        .unwrap();
    let outputs = libraries
        .execute(&node_ctx(), &[], &NodeReporter::noop())
        .await
        .unwrap();
    let rows = batches(outputs.dataframe(0).unwrap()).await;
    assert!(rows[0].num_rows() >= 1);
    let names = column::<StringArray>(&rows, "library_name");
    assert!((0..names.len()).all(|index| names.value(index).contains("KEGG")));

    let mut gene_map = registry
        .build_node(
            "source_enrichr_genemap",
            serde_json::json!({"gene": "TP53"}),
        )
        .unwrap();
    let outputs = gene_map
        .execute(&node_ctx(), &[], &NodeReporter::noop())
        .await
        .unwrap();
    let rows = batches(outputs.dataframe(0).unwrap()).await;
    assert!(rows[0].num_rows() > 100);
    let gene = column::<StringArray>(&rows, "gene");
    assert_eq!(gene.value(0), "TP53");
}

async fn agent_tools_submit_enrich_and_annotate(client: Arc<EnrichrClient>) {
    let libraries = LibrariesTool::new(client.clone());
    let result = libraries
        .execute(serde_json::json!({"query": "KEGG", "limit": 5}))
        .await
        .unwrap();
    let text = result.text_content();
    assert!(text.contains("Enrichr libraries"));
    assert!(text.contains("KEGG_2021_Human"));

    let add_list = AddListTool::new(client.clone());
    let result = add_list
        .execute(serde_json::json!({
            "genes": ["TP53", "BRCA1", "EGFR", "MYC", "PTEN"],
            "description": "e2e"
        }))
        .await
        .unwrap();
    let text = result.text_content();
    assert!(text.contains("User list ID"));

    let enrich = EnrichTool::new(client.clone());
    let result = enrich
        .execute(serde_json::json!({
            "genes": ["TP53", "BRCA1", "EGFR", "MYC", "PTEN"],
            "background_type": "GO_Biological_Process_2025",
            "limit": 5
        }))
        .await
        .unwrap();
    let text = result.text_content();
    assert!(text.contains("Enrichr enrichment — GO_Biological_Process_2025"));
    assert!(
        text.to_lowercase().contains("regulation"),
        "expected regulation-related GO terms in: {text}"
    );

    let gene_map = GeneMapTool::new(client);
    let result = gene_map
        .execute(serde_json::json!({"gene": "TP53", "limit": 5}))
        .await
        .unwrap();
    let text = result.text_content();
    assert!(text.contains("Enrichr gene map"));
}

async fn view_list_node_reads_back_submitted_list() {
    let client = EnrichrClient::new();
    let added = client
        .add_list(["TP53", "BRCA1", "EGFR"], "view e2e")
        .await
        .unwrap();

    let runtime = Arc::new(RuntimeEnv::default());
    let mut registry = NodeRegistry::with_ingredients(runtime, None);
    registry.register_plugin(&Plugin);
    let mut view = registry
        .build_node(
            "source_enrichr_view_list",
            serde_json::json!({"user_list_id": added.user_list_id}),
        )
        .unwrap();
    let outputs = view
        .execute(&node_ctx(), &[], &NodeReporter::noop())
        .await
        .unwrap();
    let rows = batches(outputs.dataframe(0).unwrap()).await;
    assert_eq!(rows[0].num_rows(), 3);
    let ids = column::<UInt64Array>(&rows, "user_list_id");
    assert_eq!(ids.value(0), added.user_list_id);
    let genes = column::<StringArray>(&rows, "gene");
    assert!((0..genes.len()).any(|index| genes.value(index) == "TP53"));
}

#[tokio::test]
#[ignore = "uses the public Enrichr API"]
async fn e2e_tools_and_nodes_against_public_enrichr() {
    dag_nodes_fetch_and_materialize_enrichr_tables().await;
    agent_tools_submit_enrich_and_annotate(Arc::new(EnrichrClient::new())).await;
    view_list_node_reads_back_submitted_list().await;
}
