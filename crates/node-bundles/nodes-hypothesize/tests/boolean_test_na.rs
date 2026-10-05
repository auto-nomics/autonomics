//! F03 regression: `hypothesize.boolean_test` NA propagation.
//!
//! Bug: the node extracted the p-value column with a null-skipping helper,
//! so an input of `[0.001, NA]` produced an intersection p of 0.001 — the
//! joint test silently dropped the non-estimable component and overstated
//! the evidence. Fix: null-aware extraction; any null component makes the
//! output p null (R `min`/`max` default NA rule); rows are never dropped.

use arrow_array::{Float64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use dag_core::dag::node_event::NodeReporter;
use dag_core::node::{DagNode, NodeInput};
use dag_core::registry::{NodeCtx, NodeFactory};
use datafusion::prelude::SessionContext;
use nodes_hypothesize::combine_nodes::BooleanNodeFactory;
use std::sync::Arc;

fn pcol_batch(p_values: &[Option<f64>]) -> RecordBatch {
    let schema = Arc::new(Schema::new(vec![Field::new(
        "p_value",
        DataType::Float64,
        true,
    )]));
    RecordBatch::try_new(schema, vec![Arc::new(Float64Array::from(p_values.to_vec()))]).unwrap()
}

async fn run_boolean(op: &str, batches: Vec<RecordBatch>) -> RecordBatch {
    let session = SessionContext::new();
    let node_ctx = NodeCtx::new(session.runtime_env(), None);
    let mut node = BooleanNodeFactory {}
        .build(
            serde_json::json!({"op": op}),
            node_ctx.clone(),
        )
        .expect("valid spec");
    // One input dataframe carrying all batches — collect_input reads only the
    // first input port, so multi-batch coverage must come from read_batches.
    let df = session.read_batches(batches).unwrap();
    let outputs = node
        .execute(
            &node_ctx,
            &[NodeInput::new_dataframe(0, df)],
            &NodeReporter::noop(),
        )
        .await
        .unwrap();
    let mut collected = outputs
        .dataframe(0)
        .unwrap()
        .clone()
        .collect()
        .await
        .unwrap();
    assert_eq!(collected.len(), 1);
    collected.remove(0)
}

/// The p-value slot of the emitted test row: `None` for a null p-value.
fn p_of(batch: &RecordBatch) -> Option<f64> {
    batch
        .column_by_name("p_value")
        .unwrap()
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap()
        .iter()
        .next()
        .flatten()
}

#[tokio::test]
async fn intersection_with_null_component_is_null() {
    // Audit counter-example: (0.001, NA) must NOT resolve to 0.001.
    let batch = run_boolean("intersection", vec![pcol_batch(&[Some(0.001), None])]).await;
    assert!(p_of(&batch).is_none(), "expected null p, got {:?}", p_of(&batch));
}

#[tokio::test]
async fn intersection_all_null_is_null() {
    let batch = run_boolean("intersection", vec![pcol_batch(&[None, None])]).await;
    assert!(p_of(&batch).is_none());
}

#[tokio::test]
async fn intersection_without_nulls_is_max_p() {
    let batch = run_boolean("intersection", vec![pcol_batch(&[Some(0.1), Some(0.2)])]).await;
    let p = p_of(&batch).unwrap();
    assert!((p - 0.2).abs() < 1e-15);
}

#[tokio::test]
async fn union_with_null_component_is_null() {
    // Union NA rule (decided): any NA → NA, matching R min(c(0.05, NA)) = NA.
    let batch = run_boolean("union", vec![pcol_batch(&[Some(0.05), None])]).await;
    assert!(p_of(&batch).is_none());
}

#[tokio::test]
async fn union_without_nulls_is_min_p() {
    let batch = run_boolean("union", vec![pcol_batch(&[Some(0.05), Some(0.3)])]).await;
    let p = p_of(&batch).unwrap();
    assert!((p - 0.05).abs() < 1e-15);
}

#[tokio::test]
async fn complement_of_null_is_null() {
    let batch = run_boolean("complement", vec![pcol_batch(&[None])]).await;
    assert!(p_of(&batch).is_none());
}

#[tokio::test]
async fn complement_of_value_is_one_minus() {
    let batch = run_boolean("complement", vec![pcol_batch(&[Some(0.3)])]).await;
    let p = p_of(&batch).unwrap();
    assert!((p - 0.7).abs() < 1e-15);
}

#[tokio::test]
async fn complement_requires_exactly_one_pvalue() {
    let session = SessionContext::new();
    let node_ctx = NodeCtx::new(session.runtime_env(), None);
    let mut node = BooleanNodeFactory {}
        .build(serde_json::json!({"op": "complement"}), node_ctx.clone())
        .unwrap();
    let df = session.read_batch(pcol_batch(&[Some(0.1), Some(0.2)])).unwrap();
    let err = node
        .execute(
            &node_ctx,
            &[NodeInput::new_dataframe(0, df)],
            &NodeReporter::noop(),
        )
        .await;
    assert!(err.is_err());
}

/// Two input batches, each containing a null — the family is the ordered
/// concatenation `[0.001, NA | 0.2, NA]`, so both intersection and union
/// propagate null. The no-null cross-batch case must still compute over all
/// rows in order.
#[tokio::test]
async fn cross_batch_nulls_propagate_and_order_preserved() {
    let two_batches = vec![pcol_batch(&[Some(0.001), None]), pcol_batch(&[Some(0.2), None])];
    let batch = run_boolean("intersection", two_batches.clone()).await;
    assert!(p_of(&batch).is_none());
    let batch = run_boolean("union", two_batches).await;
    assert!(p_of(&batch).is_none());

    // No nulls anywhere: family = [0.001, 0.01 | 0.2, 0.3] → max 0.3, min 0.001.
    let clean = vec![
        pcol_batch(&[Some(0.001), Some(0.01)]),
        pcol_batch(&[Some(0.2), Some(0.3)]),
    ];
    let batch = run_boolean("intersection", clean.clone()).await;
    assert!((p_of(&batch).unwrap() - 0.3).abs() < 1e-15);
    let batch = run_boolean("union", clean).await;
    assert!((p_of(&batch).unwrap() - 0.001).abs() < 1e-15);
}

#[tokio::test]
async fn n_column_counts_all_components() {
    let batch = run_boolean(
        "intersection",
        vec![pcol_batch(&[Some(0.001), None]), pcol_batch(&[Some(0.2)])],
    )
    .await;
    let n = batch
        .column_by_name("n")
        .unwrap()
        .as_any()
        .downcast_ref::<arrow_array::Int32Array>()
        .unwrap()
        .value(0);
    assert_eq!(n, 3);
}
