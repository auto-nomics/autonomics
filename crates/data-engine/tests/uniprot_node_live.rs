//! Registry smoke tests for the UniProt DAG source nodes.
//!
//! The first two tests need no network: they confirm the three factories are
//! registered in the default registry and that their specs deserialize. The
//! live test hits the real API:
//!
//! ```sh
//! cargo test -p data-engine --test uniprot_node_live -- --ignored
//! ```

use data_engine::node_registry::NodeCtx;
use datafusion::prelude::SessionContext;

fn build_node(kind: &str, spec: serde_json::Value) -> Box<dyn data_engine::dag::DagNode> {
    let registry = data_engine::default_registry::build_default_registry(
        SessionContext::new().runtime_env(),
        None,
        std::sync::Arc::new(dag_core::BundleRegistry::new()),
    );
    registry.build_node(kind, spec).expect("build_node failed")
}

#[test]
fn factories_are_registered() {
    let registry = data_engine::default_registry::build_default_registry(
        SessionContext::new().runtime_env(),
        None,
        std::sync::Arc::new(dag_core::BundleRegistry::new()),
    );
    let kinds: Vec<_> = registry.list_nodes().into_iter().map(|n| n.kind).collect();
    for kind in [
        "source_uniprot_search",
        "source_uniprot_stream",
        "source_uniprot_idmap",
    ] {
        assert!(kinds.contains(&kind.to_string()), "missing {kind}");
    }
}

#[test]
fn build_node_smoke() {
    // No network: just confirms the specs deserialize and the nodes build.
    let node = build_node(
        "source_uniprot_search",
        serde_json::json!({"gene": ["INS"], "organism_id": 9606, "size": 5}),
    );
    assert_eq!(node.kind(), "source_uniprot_search");

    let node = build_node(
        "source_uniprot_stream",
        serde_json::json!({
            "query": "organism_id:9606 AND reviewed:true",
            "format": "fasta",
            "path": "/tmp/uniprot_smoke.fasta",
        }),
    );
    assert_eq!(node.kind(), "source_uniprot_stream");
    assert_eq!(node.sink_path(), Some("/tmp/uniprot_smoke.fasta"));

    let node = build_node(
        "source_uniprot_idmap",
        serde_json::json!({"from_db": "Gene_Name", "to_db": "UniProtKB", "ids": ["INS"]}),
    );
    assert_eq!(node.kind(), "source_uniprot_idmap");
}

#[tokio::test]
#[ignore = "live UniProt API test"]
async fn search_node_executes_from_full_registry() {
    // End-to-end through the real registry: build by kind and execute once.
    let ctx = NodeCtx::new(SessionContext::new().runtime_env(), None);
    let mut node = build_node(
        "source_uniprot_search",
        serde_json::json!({"accessions": ["P01308"], "size": 1}),
    );
    let reporter = dag_core::dag::node_event::NodeReporter::noop();
    let outputs = node
        .execute(&ctx, &[], &reporter)
        .await
        .expect("node execute");
    let df = outputs.dataframe(0).expect("dataframe output");
    let batches = df.clone().collect().await.expect("collect");
    assert_eq!(batches[0].num_rows(), 1);
}
