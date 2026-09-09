//! Registry and live integration tests for RCSB PDB DAG nodes.

use std::sync::Arc;

use arrow_array::{Array, StringArray, UInt64Array};
use data_engine::data_engine::DataEngine;
use datafusion::prelude::SessionContext;

use dag_core::DataBundleCatalog;
use dag_core::dag::node_event::NodeReporter;
use dag_core::registry::NodeCtx;

fn registry() -> data_engine::node_registry::NodeRegistry {
    data_engine::default_registry::build_default_registry(
        SessionContext::new().runtime_env(),
        None,
        Arc::new(DataBundleCatalog::new()),
    )
}

fn build_node(kind: &str, spec: serde_json::Value) -> Box<dyn dag_core::dag::DagNode> {
    registry()
        .build_node(kind, spec)
        .unwrap_or_else(|error| panic!("build `{kind}` through default registry: {error}"))
}

fn node_ctx() -> NodeCtx {
    NodeCtx::new(SessionContext::new().runtime_env(), None)
}

async fn run_node(kind: &str, spec: serde_json::Value) -> Vec<arrow_array::RecordBatch> {
    let ctx = node_ctx();
    let mut node = build_node(kind, spec);
    let outputs = node
        .execute(&ctx, &[], &NodeReporter::noop())
        .await
        .unwrap_or_else(|error| panic!("execute `{kind}`: {error}"));
    outputs
        .dataframe(0)
        .expect("RCSB node DataFrame output")
        .clone()
        .collect()
        .await
        .expect("collect RCSB node output")
}

#[test]
fn all_rcsb_node_factories_are_registered() {
    let registry = registry();
    let kinds = registry
        .list_nodes()
        .into_iter()
        .map(|node| node.kind)
        .collect::<Vec<_>>();

    for kind in [
        "source_rcsb_search",
        "source_rcsb_entry",
        "source_rcsb_polymer_entity",
        "source_rcsb_assembly",
        "source_rcsb_structure",
    ] {
        assert!(kinds.contains(&kind.to_string()), "missing {kind}");
    }
}

#[test]
fn all_rcsb_nodes_build_and_expose_port_contracts() {
    let registry = registry();
    let cases = [
        (
            "source_rcsb_search",
            serde_json::json!({ "query": "hemoglobin", "rows": 2 }),
            0,
        ),
        (
            "source_rcsb_entry",
            serde_json::json!({ "entry_ids": ["4HHB"] }),
            1,
        ),
        (
            "source_rcsb_polymer_entity",
            serde_json::json!({ "entry_ids": ["4HHB"], "entity_ids": [1] }),
            0,
        ),
        (
            "source_rcsb_assembly",
            serde_json::json!({ "entry_id": "4HHB", "assembly_ids": [1] }),
            1,
        ),
        (
            "source_rcsb_structure",
            serde_json::json!({ "entry_id": "4HHB", "path": "/tmp/4HHB.cif" }),
            0,
        ),
    ];

    for (kind, spec, input_count) in cases {
        let node = registry
            .build_node(kind, spec)
            .unwrap_or_else(|error| panic!("build `{kind}`: {error}"));
        assert_eq!(node.kind(), kind);

        let ports = registry
            .get_node_ports(kind)
            .unwrap_or_else(|error| panic!("ports `{kind}`: {error}"));
        assert_eq!(
            ports.input_ports().len(),
            input_count,
            "unexpected input count for `{kind}`"
        );
        assert_eq!(
            ports.output_ports().len(),
            1,
            "unexpected output count for `{kind}`"
        );

        let schema = registry
            .get_node_spec(kind)
            .unwrap_or_else(|error| panic!("schema `{kind}`: {error}"));
        assert!(
            serde_json::to_value(&schema).is_ok(),
            "schema for `{kind}` should serialize"
        );
    }
}

#[tokio::test]
#[ignore = "live RCSB API test"]
async fn default_engine_executes_search_to_entry_dag() {
    let mut engine = DataEngine::builder().build();
    engine
        .add_node_from_registry(
            "search",
            "source_rcsb_search",
            serde_json::json!({ "query": "deoxyhaemoglobin", "rows": 2 }),
        )
        .expect("add RCSB search node");
    engine
        .add_node_from_registry("entries", "source_rcsb_entry", serde_json::json!({}))
        .expect("add RCSB entry node");
    engine
        .add_edge("search", "entries", 0, 0)
        .expect("connect search to entries");

    let report = engine.run().await.expect("run RCSB DAG");
    assert!(
        report
            .nodes
            .iter()
            .all(|node| node.status == dag_core::dag::runtime::RuntimeStatus::Success),
        "RCSB DAG node failed: {report:?}"
    );

    let outputs = engine
        .get_output("entries")
        .await
        .expect("entry node output should be retained");
    let batches = outputs
        .dataframe(0)
        .expect("entry DataFrame")
        .clone()
        .collect()
        .await
        .expect("collect entry output");
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].num_rows(), 2);

    let entry_ids = batches[0]
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("entry IDs");
    assert!(
        entry_ids
            .iter()
            .all(|id| id.is_some_and(|id| id.len() == 4))
    );
}

#[tokio::test]
#[ignore = "live RCSB API test"]
async fn default_registry_executes_polymer_and_assembly_nodes() {
    let polymer = run_node(
        "source_rcsb_polymer_entity",
        serde_json::json!({ "entry_ids": ["4HHB"], "entity_ids": [1] }),
    )
    .await;
    assert_eq!(polymer.len(), 1);
    assert_eq!(polymer[0].num_rows(), 1);
    let sequences = polymer[0]
        .column(3)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("sequences");
    assert_eq!(sequences.value(0).len(), 141);

    let assembly = run_node(
        "source_rcsb_assembly",
        serde_json::json!({ "entry_id": "4HHB", "assembly_ids": [1] }),
    )
    .await;
    assert_eq!(assembly.len(), 1);
    assert_eq!(assembly[0].num_rows(), 1);
    let atoms = assembly[0]
        .column(4)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .expect("assembly atom counts");
    assert_eq!(atoms.value(0), 4779);
}

#[tokio::test]
#[ignore = "live RCSB API test"]
async fn default_registry_executes_structure_file_node() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("4HHB.cif");
    let path_str = path.to_str().expect("UTF-8 path").to_owned();

    let ctx = node_ctx();
    let mut node = build_node(
        "source_rcsb_structure",
        serde_json::json!({ "entry_id": "4HHB", "format": "cif", "path": path_str }),
    );
    let outputs = node
        .execute(&ctx, &[], &NodeReporter::noop())
        .await
        .expect("execute structure node");
    let file = outputs
        .get(&0)
        .and_then(|value| value.as_file().ok())
        .expect("structure FileRef");
    assert_eq!(file.path, path_str);
    assert_eq!(file.format.as_deref(), Some("cif"));

    let bytes = std::fs::read(&path).expect("downloaded mmCIF");
    assert!(bytes.starts_with(b"data_4HHB"));
}
