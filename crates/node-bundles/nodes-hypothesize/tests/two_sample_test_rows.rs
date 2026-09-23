//! Regression tests for the standard test-row output of multi-sample
//! `hypothesize.*` nodes.
//!
//! Bug: two-sample/k-sample tests never set the `n` extra, and the row
//! assembler rendered a missing `n` as a `NullArray` — clashing with the
//! schema's `Int32` column and panicking inside `RecordBatch::try_new`
//! (`expected Int32 but found Null at column index 7`). These tests pin the
//! 6-row two-group case that reproduced the panic in milliseconds.

use arrow_array::{Array, Float64Array, Int32Array, RecordBatch, StringArray};
use arrow_schema::DataType;
use dag_core::dag::node_event::NodeReporter;
use dag_core::node::{DagNode, NodeInput};
use dag_core::registry::{NodeCtx, NodeFactory};
use datafusion::prelude::SessionContext;
use nodes_hypothesize::common::test_row_schema;
use nodes_hypothesize::parametric::TTestNodeFactory;
use nodes_hypothesize::ranks::WilcoxonNodeFactory;
use nodes_hypothesize::variance::AnovaNodeFactory;
use std::sync::Arc;

/// The 6-row table from the bug report: VALUES(1,'A'),(2,'A'),(3,'A'),
/// (5,'B'),(6,'B'),(7,'B') — x = [1,2,3] vs y = [5,6,7].
fn grouped_batch() -> RecordBatch {
    use arrow_schema::{Field, Schema};
    let schema = Arc::new(Schema::new(vec![
        Field::new("val", DataType::Float64, false),
        Field::new("grp", DataType::Utf8, false),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Float64Array::from(vec![1.0, 2.0, 3.0, 5.0, 6.0, 7.0])),
            Arc::new(StringArray::from(vec!["A", "A", "A", "B", "B", "B"])),
        ],
    )
    .unwrap()
}

/// Wide 3-row table with the same two samples as separate columns.
fn wide_batch() -> RecordBatch {
    use arrow_schema::{Field, Schema};
    let schema = Arc::new(Schema::new(vec![
        Field::new("x", DataType::Float64, false),
        Field::new("y", DataType::Float64, false),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Float64Array::from(vec![1.0, 2.0, 3.0])),
            Arc::new(Float64Array::from(vec![5.0, 6.0, 7.0])),
        ],
    )
    .unwrap()
}

async fn run_node(
    factory: &dyn NodeFactory,
    spec: serde_json::Value,
    input: RecordBatch,
) -> RecordBatch {
    let session = SessionContext::new();
    let node_ctx = NodeCtx::new(session.runtime_env(), None);
    let mut node = factory.build(spec, node_ctx.clone()).expect("valid spec");
    let df = session.read_batch(input).unwrap();
    let outputs = node
        .execute(
            &node_ctx,
            &[NodeInput::new_dataframe(0, df)],
            &NodeReporter::noop(),
        )
        .await
        .expect("node must not panic");
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

/// The output must carry exactly the standard test-row schema, with every
/// column physically matching its declared type.
fn assert_schema_conform(batch: &RecordBatch) {
    let schema = test_row_schema();
    assert_eq!(batch.schema().as_ref(), schema.as_ref());
    for (i, field) in schema.fields().iter().enumerate() {
        assert_eq!(
            batch.column(i).data_type(),
            field.data_type(),
            "column '{}' physical type must match schema",
            field.name()
        );
    }
}

fn n_of(batch: &RecordBatch) -> i32 {
    batch
        .column_by_name("n")
        .unwrap()
        .as_any()
        .downcast_ref::<Int32Array>()
        .unwrap()
        .value(0)
}

fn f64_of(batch: &RecordBatch, col: &str) -> f64 {
    batch
        .column_by_name(col)
        .unwrap()
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap()
        .value(0)
}

/// Grouped two-sample t-test — the exact panic case from the bug report.
///
/// x = [1,2,3], y = [5,6,7]: pooled sp² = 1, se = √(2/3), t = −4/√(2/3),
/// df = 4, p = 0.0080498931 (closed form for the t₄ survival function).
#[tokio::test]
async fn t_test_grouped_two_sample_emits_valid_row() {
    for (var_equal, method) in [
        (false, "Welch Two Sample t-test"),
        (true, "Two Sample t-test (pooled)"),
    ] {
        let batch = run_node(
            &TTestNodeFactory,
            serde_json::json!({"x_column": "val", "group_column": "grp", "var_equal": var_equal}),
            grouped_batch(),
        )
        .await;
        assert_schema_conform(&batch);
        assert!((f64_of(&batch, "statistic") - -4.898_979_485_566_356).abs() < 1e-9);
        assert!((f64_of(&batch, "dof") - 4.0).abs() < 1e-9);
        assert!((f64_of(&batch, "p_value") - 0.008_049_893_1).abs() < 1e-7);
        assert_eq!(n_of(&batch), 6);
        let method_col = batch
            .column_by_name("method")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(method_col.value(0), method);
    }
}

/// Wide x/y form of the two-sample t-test — same numbers, different wiring.
#[tokio::test]
async fn t_test_wide_two_sample_emits_valid_row() {
    let batch = run_node(
        &TTestNodeFactory,
        serde_json::json!({"x_column": "x", "y_column": "y"}),
        wide_batch(),
    )
    .await;
    assert_schema_conform(&batch);
    assert!((f64_of(&batch, "statistic") - -4.898_979_485_566_356).abs() < 1e-9);
    assert!((f64_of(&batch, "dof") - 4.0).abs() < 1e-9);
    assert_eq!(n_of(&batch), 6);
}

/// Grouped one-way ANOVA — same data, so F = t² = 24 with df1 = 1.
#[tokio::test]
async fn oneway_anova_grouped_emits_valid_row() {
    for var_equal in [true, false] {
        let batch = run_node(
            &AnovaNodeFactory,
            serde_json::json!({"value_column": "val", "group_column": "grp", "var_equal": var_equal}),
            grouped_batch(),
        )
        .await;
        assert_schema_conform(&batch);
        assert!((f64_of(&batch, "statistic") - 24.0).abs() < 1e-9);
        assert!((f64_of(&batch, "dof") - 1.0).abs() < 1e-9);
        assert!((f64_of(&batch, "p_value") - 0.008_049_893_1).abs() < 1e-7);
        assert_eq!(n_of(&batch), 6);
    }
}

/// Grouped Wilcoxon (Mann–Whitney): x dominates y completely → U = 0, n = 6.
#[tokio::test]
async fn wilcoxon_grouped_emits_valid_row() {
    let batch = run_node(
        &WilcoxonNodeFactory,
        serde_json::json!({"x_column": "val", "group_column": "grp"}),
        grouped_batch(),
    )
    .await;
    assert_schema_conform(&batch);
    assert!((f64_of(&batch, "statistic") - 0.0).abs() < 1e-12);
    assert!((f64_of(&batch, "p_value") - 0.080_66).abs() < 5e-4);
    assert_eq!(n_of(&batch), 6);
}

/// One-sample mode keeps its already-working behaviour: n filled, row valid.
#[tokio::test]
async fn t_test_one_sample_still_emits_valid_row() {
    let batch = run_node(
        &TTestNodeFactory,
        serde_json::json!({"x_column": "x"}),
        wide_batch(),
    )
    .await;
    assert_schema_conform(&batch);
    assert_eq!(n_of(&batch), 3);
}
