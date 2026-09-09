//! Tests for the ChEMBL DAG source nodes.
//!
//! The two live tests hit the public ChEMBL API and are ignored by default:
//!
//! ```sh
//! cargo test -p data-engine --test chembl_node_live -- --ignored
//! ```

use data_engine::node_registry::{NodeCtx, NodeFactory};
use datafusion::prelude::SessionContext;
use nodes_io::source_chembl::{ChemblActivitiesNodeFactory, ChemblMoleculesNodeFactory};

fn node_ctx() -> NodeCtx {
    NodeCtx::new(SessionContext::new().runtime_env(), None)
}

fn build_node(kind: &str, spec: serde_json::Value) -> Box<dyn data_engine::dag::DagNode> {
    let registry = data_engine::default_registry::build_default_registry(
        SessionContext::new().runtime_env(),
        None,
        std::sync::Arc::new(dag_core::DataBundleCatalog::new()),
    );
    registry.build_node(kind, spec).expect("build_node failed")
}

#[test]
fn chembl_factories_are_registered() {
    let registry = data_engine::default_registry::build_default_registry(
        SessionContext::new().runtime_env(),
        None,
        std::sync::Arc::new(dag_core::DataBundleCatalog::new()),
    );
    let kinds: Vec<_> = registry
        .list_nodes()
        .into_iter()
        .map(|node| node.kind)
        .collect();
    assert!(kinds.contains(&"source_chembl_activities".to_string()));
    assert!(kinds.contains(&"source_chembl_molecules".to_string()));
    assert_eq!(
        ChemblActivitiesNodeFactory.kind(),
        "source_chembl_activities"
    );
    assert_eq!(ChemblMoleculesNodeFactory.kind(), "source_chembl_molecules");
}

#[test]
fn chembl_nodes_build_from_specs() {
    let activities = build_node(
        "source_chembl_activities",
        serde_json::json!({
            "molecule_chembl_id": "CHEMBL25",
            "fetch_all": false,
            "size": 10
        }),
    );
    assert_eq!(activities.kind(), "source_chembl_activities");

    let molecules = build_node(
        "source_chembl_molecules",
        serde_json::json!({"query": "aspirin"}),
    );
    assert_eq!(molecules.kind(), "source_chembl_molecules");
}

async fn run_node(
    kind: &str,
    spec: serde_json::Value,
) -> datafusion::common::Result<Vec<arrow_array::RecordBatch>> {
    let mut node = build_node(kind, spec);
    let outputs = node
        .execute(
            &node_ctx(),
            &[],
            &data_engine::dag::node_event::NodeReporter::noop(),
        )
        .await
        .expect("node execute failed");
    let dataframe = outputs.dataframe(0).expect("missing output port 0").clone();
    dataframe.collect().await
}

#[tokio::test]
#[ignore = "hits the live ChEMBL API"]
async fn activities_node_runs() {
    let batches = run_node(
        "source_chembl_activities",
        serde_json::json!({
            "molecule_chembl_id": "CHEMBL25",
            "fetch_all": false,
            "size": 10
        }),
    )
    .await
    .unwrap();
    let total: usize = batches.iter().map(|batch| batch.num_rows()).sum();
    assert!(total > 0, "expected activities for aspirin");
    let schema = batches[0].schema();
    let columns: Vec<_> = schema
        .fields()
        .iter()
        .map(|field| field.name().as_str())
        .collect();
    assert!(columns.contains(&"molecule_id"));
    assert!(columns.contains(&"standard_value"));
    assert!(columns.contains(&"pchembl_value"));
}

#[tokio::test]
#[ignore = "hits the live ChEMBL API"]
async fn molecules_node_runs() {
    let batches = run_node(
        "source_chembl_molecules",
        serde_json::json!({"query": "aspirin", "size": 10}),
    )
    .await
    .unwrap();
    let total: usize = batches.iter().map(|batch| batch.num_rows()).sum();
    assert!(total > 0, "expected molecules matching aspirin");
    let schema = batches[0].schema();
    let columns: Vec<_> = schema
        .fields()
        .iter()
        .map(|field| field.name().as_str())
        .collect();
    assert!(columns.contains(&"molecule_id"));
    assert!(columns.contains(&"canonical_smiles"));
}
