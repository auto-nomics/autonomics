//! Registry smoke and live tests for the Reactome DAG source nodes.
//!
//! The first two tests need no network: they confirm the four factories
//! are registered in the default registry and that their specs
//! deserialize. The live tests hit the real API:
//!
//! ```sh
//! cargo test -p data-engine --test reactome_node_live -- --ignored
//! ```

use data_engine::node_registry::NodeCtx;
use datafusion::prelude::SessionContext;

fn build_node(kind: &str, spec: serde_json::Value) -> Box<dyn data_engine::dag::DagNode> {
    let registry = data_engine::default_registry::build_default_registry(
        SessionContext::new().runtime_env(),
        None,
        std::sync::Arc::new(dag_core::DataBundleCatalog::new()),
    );
    registry.build_node(kind, spec).expect("build_node failed")
}

#[test]
fn factories_are_registered() {
    let registry = data_engine::default_registry::build_default_registry(
        SessionContext::new().runtime_env(),
        None,
        std::sync::Arc::new(dag_core::DataBundleCatalog::new()),
    );
    let kinds: Vec<_> = registry.list_nodes().into_iter().map(|n| n.kind).collect();
    for kind in [
        "source_reactome_pathways",
        "source_reactome_mapping",
        "source_reactome_analysis",
        "source_reactome_participants",
    ] {
        assert!(kinds.contains(&kind.to_string()), "missing {kind}");
    }
}

#[test]
fn build_node_smoke() {
    // No network: just confirms the specs deserialize and the nodes build.
    let node = build_node(
        "source_reactome_pathways",
        serde_json::json!({"species": "Homo sapiens"}),
    );
    assert_eq!(node.kind(), "source_reactome_pathways");

    let node = build_node(
        "source_reactome_mapping",
        serde_json::json!({"resource": "UniProt", "identifier": "P04637"}),
    );
    assert_eq!(node.kind(), "source_reactome_mapping");

    let node = build_node(
        "source_reactome_analysis",
        serde_json::json!({"identifiers": ["TP53", "BRCA1"]}),
    );
    assert_eq!(node.kind(), "source_reactome_analysis");

    let node = build_node(
        "source_reactome_participants",
        serde_json::json!({"id": "R-HSA-1640170"}),
    );
    assert_eq!(node.kind(), "source_reactome_participants");
}

#[tokio::test]
#[ignore = "live Reactome API test"]
async fn pathways_node_executes_from_full_registry() {
    let ctx = NodeCtx::new(SessionContext::new().runtime_env(), None);
    let mut node = build_node(
        "source_reactome_pathways",
        serde_json::json!({"species": "Homo sapiens"}),
    );
    let reporter = dag_core::dag::node_event::NodeReporter::noop();
    let outputs = node
        .execute(&ctx, &[], &reporter)
        .await
        .expect("node execute");
    let df = outputs.dataframe(0).expect("dataframe output");
    let batches = df.clone().collect().await.expect("collect");
    assert!(batches[0].num_rows() > 0, "Homo sapiens should have pathways");
}

#[tokio::test]
#[ignore = "live Reactome API test"]
async fn analysis_node_executes_from_full_registry() {
    let ctx = NodeCtx::new(SessionContext::new().runtime_env(), None);
    let mut node = build_node(
        "source_reactome_analysis",
        serde_json::json!({"identifiers": ["TP53", "BRCA1", "EGFR"]}),
    );
    let reporter = dag_core::dag::node_event::NodeReporter::noop();
    let outputs = node
        .execute(&ctx, &[], &reporter)
        .await
        .expect("node execute");
    let df = outputs.dataframe(0).expect("dataframe output");
    let batches = df.clone().collect().await.expect("collect");
    assert!(batches[0].num_rows() > 0, "analysis should return enriched pathways");
}
