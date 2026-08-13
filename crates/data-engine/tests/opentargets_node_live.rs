//! Live tests for the Open Targets DAG source nodes.
//!
//! These hit the real Open Targets API and are `#[ignore]` by default:
//!
//! ```sh
//! cargo test -p data-engine --test opentargets_node_live -- --ignored
//! ```

use data_engine::node_registry::{NodeCtx, NodeFactory};
use datafusion::prelude::SessionContext;
use nodes_io::source_opentargets::{
    OpentargetsAssociationsNodeFactory, OpentargetsSearchNodeFactory,
};
use std::sync::Arc;

fn node_ctx() -> NodeCtx {
    NodeCtx {
        runtime_env: SessionContext::new().runtime_env(),
        iceberg_catalog: None,
        datalake: Arc::new(datalake::Datalake::default()),
        opendal: None,
        resources: std::sync::Arc::new(dag_core::resource_catalog::ResourceCatalog::new(
            std::path::PathBuf::from("."),
        )),
        global_sem: None,
    }
}

/// Build the full registry and return a built node by kind + spec.
fn build_node(kind: &str, spec: serde_json::Value) -> Box<dyn data_engine::dag::DagNode> {
    let registry = data_engine::default_registry::build_default_registry(
        SessionContext::new().runtime_env(),
        None,
        Arc::new(datalake::Datalake::default()),
        None,
        std::sync::Arc::new(dag_core::resource_catalog::ResourceCatalog::new(
            std::path::PathBuf::from("."),
        )),
    );
    registry.build_node(kind, spec).expect("build_node failed")
}

#[test]
fn factories_are_registered() {
    // The two factories must be reachable through the registry (kind-agnostic
    // tool layer depends on this).
    let registry = data_engine::default_registry::build_default_registry(
        SessionContext::new().runtime_env(),
        None,
        Arc::new(datalake::Datalake::default()),
        None,
        std::sync::Arc::new(dag_core::resource_catalog::ResourceCatalog::new(
            std::path::PathBuf::from("."),
        )),
    );
    let kinds: Vec<_> = registry.list_nodes().into_iter().map(|n| n.kind).collect();
    assert!(kinds.contains(&"source_opentargets_associations".to_string()));
    assert!(kinds.contains(&"source_opentargets_search".to_string()));
    // Sanity: factory kind strings match.
    assert_eq!(
        OpentargetsAssociationsNodeFactory {}.kind(),
        "source_opentargets_associations"
    );
    assert_eq!(
        OpentargetsSearchNodeFactory {}.kind(),
        "source_opentargets_search"
    );
}

#[test]
fn build_node_smoke() {
    // No network: just confirms the spec deserializes and the node builds.
    let node = build_node(
        "source_opentargets_associations",
        serde_json::json!({"id": "ENSG00000012048", "fetch_all": false, "size": 1}),
    );
    assert_eq!(node.kind(), "source_opentargets_associations");
    let node = build_node(
        "source_opentargets_search",
        serde_json::json!({"query": "BRCA1"}),
    );
    assert_eq!(node.kind(), "source_opentargets_search");
}

async fn run_node(
    kind: &str,
    spec: serde_json::Value,
) -> datafusion::common::Result<Vec<arrow_array::RecordBatch>> {
    let mut node = build_node(kind, spec);
    let mut outputs = node
        .execute(
            &node_ctx(),
            &[],
            &data_engine::dag::node_event::NodeReporter::noop(),
        )
        .await
        .expect("node execute failed");
    let df = outputs.remove(&0).expect("missing output port 0");
    df.collect().await
}

#[tokio::test]
#[ignore = "hits the live Open Targets API"]
async fn associations_target_to_disease() {
    let batches = run_node(
        "source_opentargets_associations",
        serde_json::json!({
            "id": "ENSG00000012048",
            "b_filter": "breast",
            "min_score": 0.1,
            "fetch_all": true
        }),
    )
    .await
    .unwrap();
    let total: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert!(total > 0, "expected association rows for BRCA1");
    let schema = batches[0].schema();
    let cols: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert_eq!(
        cols,
        vec![
            "target_id",
            "disease_id",
            "disease_name",
            "score",
            "novelty"
        ]
    );
}

#[tokio::test]
#[ignore = "hits the live Open Targets API"]
async fn associations_disease_to_target() {
    let batches = run_node(
        "source_opentargets_associations",
        serde_json::json!({
            "direction": "disease_to_target",
            "id": "MONDO_0004975",
            "fetch_all": false,
            "size": 5
        }),
    )
    .await
    .unwrap();
    let total: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert!(total > 0);
    let schema = batches[0].schema();
    let cols: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert_eq!(
        cols,
        vec![
            "disease_id",
            "target_id",
            "symbol",
            "approved_name",
            "biotype",
            "score",
            "novelty"
        ]
    );
}

#[tokio::test]
#[ignore = "hits the live Open Targets API"]
async fn search_node_runs() {
    let batches = run_node(
        "source_opentargets_search",
        serde_json::json!({"query": "BRCA1", "entity": ["target"], "size": 5}),
    )
    .await
    .unwrap();
    let total: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert!(total > 0);
}
