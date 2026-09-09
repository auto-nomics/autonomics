//! Live integration tests for RCSB DAG source nodes.

use arrow_array::{Array, StringArray, UInt64Array};
use datafusion::prelude::SessionContext;

use dag_core::dag::node_event::NodeReporter;
use dag_core::dag::{DagNode, NodeInput};
use dag_core::registry::{NodeCtx, NodeFactory};
use rcsb::nodes::entry::{RcsbEntryNodeFactory, RcsbEntrySpec};
use rcsb::nodes::search::{RcsbSearchNodeFactory, RcsbSearchSpec};

fn node_ctx() -> NodeCtx {
    NodeCtx::new(SessionContext::new().runtime_env(), None)
}

#[tokio::test]
#[ignore = "live RCSB API test"]
async fn search_node_emits_identifiers_and_scores() {
    let ctx = node_ctx();
    let spec = serde_json::json!({
        "query": "deoxyhaemoglobin",
        "rows": 2
    });
    let mut node = RcsbSearchNodeFactory {}
        .build(spec, dag_core::registry::NodeCtx::clone(&ctx))
        .expect("build search node");
    let outputs = node
        .execute(&ctx, &[], &NodeReporter::noop())
        .await
        .expect("execute search node");
    let batches = outputs
        .dataframe(0)
        .expect("dataframe output")
        .clone()
        .collect()
        .await
        .expect("collect search output");

    assert!(!batches.is_empty());
    let identifiers = batches[0]
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("identifier strings");
    assert_eq!(identifiers.len(), 2);
    assert!(identifiers.iter().all(|id| {
        id.is_some_and(|id| id.len() == 4 && id.bytes().all(|byte| byte.is_ascii_alphanumeric()))
    }));
}

#[tokio::test]
#[ignore = "live RCSB API test"]
async fn entry_node_emits_structured_metadata() {
    let ctx = node_ctx();
    let spec = serde_json::json!({ "entry_ids": ["4HHB"] });
    let mut node = RcsbEntryNodeFactory {}
        .build(spec, dag_core::registry::NodeCtx::clone(&ctx))
        .expect("build entry node");
    let outputs = node
        .execute(&ctx, &[], &NodeReporter::noop())
        .await
        .expect("execute entry node");
    let batches = outputs
        .dataframe(0)
        .expect("dataframe output")
        .clone()
        .collect()
        .await
        .expect("collect entry output");

    assert_eq!(batches.len(), 1);
    let entry_ids = batches[0]
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("entry IDs");
    assert_eq!(entry_ids.len(), 1);
    assert_eq!(entry_ids.value(0), "4HHB");

    let atoms = batches[0]
        .column(4)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .expect("atom counts");
    assert_eq!(atoms.value(0), 4779);
}

#[test]
fn specs_deserialize_defaults() {
    let search: RcsbSearchSpec = serde_json::from_str("{}").unwrap();
    assert_eq!(search.query, "");
    let entry: RcsbEntrySpec = serde_json::from_str("{}").unwrap();
    assert!(entry.entry_ids.is_empty());
}
