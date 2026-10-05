use arrow_array::{Float64Array, Int32Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use dag_core::dag::node_event::NodeReporter;
use dag_core::node::{DagNode, NodeInput};
use dag_core::registry::{NodeCtx, NodeFactory};
use datafusion::prelude::SessionContext;
use nodes_hypothesize::combine_nodes::AdjustNodeFactory;
use std::sync::Arc;

fn input_batch(p_values: &[f64]) -> RecordBatch {
    let schema = Arc::new(Schema::new(vec![Field::new(
        "p_value",
        DataType::Float64,
        false,
    )]));
    RecordBatch::try_new(
        schema,
        vec![Arc::new(Float64Array::from(p_values.to_vec()))],
    )
    .unwrap()
}

async fn run_adjust(spec: serde_json::Value) -> RecordBatch {
    let session = SessionContext::new();
    let node_ctx = NodeCtx::new(session.runtime_env(), None);
    let mut node = AdjustNodeFactory {}
        .build(spec, node_ctx.clone())
        .expect("valid spec");
    let input = session
        .read_batch(input_batch(&[0.001, 0.01, 0.02, 0.5]))
        .unwrap();
    let outputs = node
        .execute(
            &node_ctx,
            &[NodeInput::new_dataframe(0, input)],
            &NodeReporter::noop(),
        )
        .await
        .unwrap();

    let mut batches = outputs
        .dataframe(0)
        .unwrap()
        .clone()
        .collect()
        .await
        .unwrap();
    assert_eq!(batches.len(), 1);
    batches.remove(0)
}

#[tokio::test]
async fn adjust_pvalues_rejects_at_configured_alpha() {
    let alpha_05 = run_adjust(serde_json::json!({"method": "BH", "alpha": 0.05})).await;
    let alpha_01 = run_adjust(serde_json::json!({"method": "BH", "alpha": 0.01})).await;

    for batch in [&alpha_05, &alpha_01] {
        assert_eq!(batch.num_columns(), 3);
        assert!(batch.column_by_name("p_adj").is_some());
        assert!(batch.column_by_name("reject").is_some());
        assert!(batch.column_by_name("reject_0.05").is_none());

        let adjusted = batch
            .column_by_name("p_adj")
            .unwrap()
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        let expected = [0.004, 0.02, 0.02 * 4.0 / 3.0, 0.5];
        for (got, expected) in adjusted.iter().zip(expected) {
            let got = got.unwrap();
            assert!(
                (got - expected).abs() < 1e-12,
                "got {got}, expected {expected}"
            );
        }
    }

    let reject = |batch: &RecordBatch| {
        batch
            .column_by_name("reject")
            .unwrap()
            .as_any()
            .downcast_ref::<Int32Array>()
            .unwrap()
            .values()
            .to_vec()
    };
    assert_eq!(reject(&alpha_05), vec![1, 1, 1, 0]);
    assert_eq!(reject(&alpha_01), vec![1, 0, 0, 0]);
}

#[tokio::test]
async fn adjust_pvalues_defaults_alpha_to_0_05() {
    let batch = run_adjust(serde_json::json!({"method": "BH"})).await;
    let reject = batch
        .column_by_name("reject")
        .unwrap()
        .as_any()
        .downcast_ref::<Int32Array>()
        .unwrap();
    assert_eq!(reject.values(), &[1, 1, 1, 0]);
}

#[test]
fn adjust_pvalues_rejects_invalid_alpha() {
    let node_ctx = NodeCtx::new(SessionContext::new().runtime_env(), None);
    for alpha in [0.0, 1.01, -0.05] {
        let result = AdjustNodeFactory {}.build(
            serde_json::json!({"method": "BH", "alpha": alpha}),
            node_ctx.clone(),
        );
        assert!(result.is_err(), "alpha {alpha} should be invalid");
    }
}
