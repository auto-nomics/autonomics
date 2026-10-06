//! F05: `hypothesize.t_test` complete-case pairing + estimate CI/SE rows.
//!
//! Audit counter-example: with `after = [1,NA,5,10]`, `before = [0,3,NA,2]`,
//! the old node extracted each column independently (NA-dropped per column),
//! pairing x[0]=1 with y[0]=0 but x[3]=10 with y[2]=3 → mean diff 3.6667.
//! R `t.test(paired=TRUE)` deletes whole pairs: only (1,0) and (10,2)
//! survive → mean diff 4.5. These tests pin the R behavior through the node,
//! plus the new stderr/conf_low/conf_high row columns at conf_level.
//!
//! Golden values from R 4.6.1 `Rscript` (epsilon 1e-14).

use arrow_array::{Float64Array, Int32Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use dag_core::dag::node_event::NodeReporter;
use dag_core::node::{DagNode, NodeInput};
use dag_core::registry::{NodeCtx, NodeFactory};
use datafusion::prelude::SessionContext;
use nodes_hypothesize::common::test_row_schema;
use nodes_hypothesize::parametric::TTestNodeFactory;
use std::sync::Arc;

/// Audit table: after = c(1,NA,5,10), before = c(0,3,NA,2).
fn audit_batch() -> RecordBatch {
    wide_batch(
        &[Some(1.0), None, Some(5.0), Some(10.0)],
        &[Some(0.0), Some(3.0), None, Some(2.0)],
    )
}

fn wide_batch(x: &[Option<f64>], y: &[Option<f64>]) -> RecordBatch {
    let schema = Arc::new(Schema::new(vec![
        Field::new("x", DataType::Float64, true),
        Field::new("y", DataType::Float64, true),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Float64Array::from(x.to_vec())),
            Arc::new(Float64Array::from(y.to_vec())),
        ],
    )
    .unwrap()
}

async fn run_t_test(spec: serde_json::Value, batches: Vec<RecordBatch>) -> RecordBatch {
    let session = SessionContext::new();
    let node_ctx = NodeCtx::new(session.runtime_env(), None);
    let mut node = TTestNodeFactory {}
        .build(spec, node_ctx.clone())
        .expect("valid spec");
    let df = session.read_batches(batches).unwrap();
    let outputs = node
        .execute(
            &node_ctx,
            &[NodeInput::new_dataframe(0, df)],
            &NodeReporter::noop(),
        )
        .await
        .expect("t_test node must succeed");
    let mut rows = outputs
        .dataframe(0)
        .unwrap()
        .clone()
        .collect()
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    rows.remove(0)
}

fn opt_f64(batch: &RecordBatch, col: &str) -> Option<f64> {
    batch
        .column_by_name(col)
        .unwrap()
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap()
        .iter()
        .next()
        .flatten()
}

fn opt_i32(batch: &RecordBatch, col: &str) -> Option<i32> {
    batch
        .column_by_name(col)
        .unwrap()
        .as_any()
        .downcast_ref::<Int32Array>()
        .unwrap()
        .iter()
        .next()
        .flatten()
}

fn assert_close(got: Option<f64>, expected: f64, what: &str) {
    let got = got.unwrap_or_else(|| panic!("{what} must be non-null"));
    assert!(
        (got - expected).abs() < 1e-12,
        "{what}: got {got}, expected {expected}"
    );
}

/// R: t.test(after, before, paired=TRUE) over the audit table — complete
/// pairs (1,0),(10,2): estimate 4.5 (NOT 3.6667 from per-column NA-drops),
/// stderr 3.5, t 1.2857142857142858, df 1, p 0.42083315167886859,
/// CI (-39.971716576611428, 48.971716576611435), n 2.
#[tokio::test]
async fn paired_complete_case_matches_r_audit_example() {
    let row = run_t_test(
        serde_json::json!({"x_column": "x", "y_column": "y", "paired": true}),
        vec![audit_batch()],
    )
    .await;
    assert_close(opt_f64(&row, "estimate"), 4.5, "estimate");
    assert_close(opt_f64(&row, "stderr"), 3.5, "stderr");
    assert_close(opt_f64(&row, "statistic"), 1.2857142857142858, "statistic");
    assert_close(opt_f64(&row, "dof"), 1.0, "dof");
    assert_close(opt_f64(&row, "p_value"), 0.42083315167886859, "p_value");
    assert_close(opt_f64(&row, "conf_low"), -39.971716576611428, "conf_low");
    assert_close(opt_f64(&row, "conf_high"), 48.971716576611435, "conf_high");
    assert_eq!(opt_i32(&row, "n"), Some(2));
}

/// Same audit data split across two record batches — pairing stays
/// row-aligned across the batch seam (batch 1: rows 1–2, batch 2: rows 3–4).
#[tokio::test]
async fn paired_complete_case_across_batches() {
    let b1 = wide_batch(&[Some(1.0), None], &[Some(0.0), Some(3.0)]);
    let b2 = wide_batch(&[Some(5.0), Some(10.0)], &[None, Some(2.0)]);
    let row = run_t_test(
        serde_json::json!({"x_column": "x", "y_column": "y", "paired": true}),
        vec![b1, b2],
    )
    .await;
    assert_close(opt_f64(&row, "estimate"), 4.5, "estimate");
    assert_close(opt_f64(&row, "stderr"), 3.5, "stderr");
    assert_close(opt_f64(&row, "p_value"), 0.42083315167886859, "p_value");
    assert_eq!(opt_i32(&row, "n"), Some(2));
}

/// R: t.test(x, y, paired=TRUE, alternative="greater") — p 0.21041657583943429,
/// one-sided CI (-17.59813030136263, Inf) → conf_high cell null.
#[tokio::test]
async fn paired_greater_one_sided_matches_r() {
    let row = run_t_test(
        serde_json::json!({
            "x_column": "x", "y_column": "y", "paired": true,
            "alternative": "greater"
        }),
        vec![audit_batch()],
    )
    .await;
    assert_close(opt_f64(&row, "p_value"), 0.21041657583943429, "p_value");
    assert_close(opt_f64(&row, "conf_low"), -17.59813030136263, "conf_low");
    assert_eq!(opt_f64(&row, "conf_high"), None, "+Inf bound → null cell");
}

/// R: t.test(x, y, paired=TRUE, conf.level=0.9) — CI
/// (-17.59813030136263, 26.59813030136263).
#[tokio::test]
async fn paired_conf_level_90_matches_r() {
    let row = run_t_test(
        serde_json::json!({
            "x_column": "x", "y_column": "y", "paired": true,
            "conf_level": 0.9
        }),
        vec![audit_batch()],
    )
    .await;
    assert_close(opt_f64(&row, "conf_low"), -17.59813030136263, "conf_low");
    assert_close(opt_f64(&row, "conf_high"), 26.59813030136263, "conf_high");
}

/// R (2026-10-05): t.test(c(1.2,NA,2.4,3.1,NA,4.7), c(2.1,3.5,NA,4.0,5.2,6.8))
/// — wide two-sample drops NA per column (vector semantics): Welch on
/// x=[1.2,2.4,3.1,4.7], y=[2.1,3.5,4.0,5.2,6.8]:
/// m1=2.85, m2=4.32, est=-1.47, stderr 1.07961412859719,
/// t -1.36159759404970, df 6.97484737539948, p 0.21566981730390,
/// CI (-4.02474919454316, 1.08474919454316), n=9.
#[tokio::test]
async fn wide_welch_independent_na_matches_r() {
    let batch = wide_batch(
        &[Some(1.2), None, Some(2.4), Some(3.1), None, Some(4.7)],
        &[Some(2.1), Some(3.5), None, Some(4.0), Some(5.2), Some(6.8)],
    );
    let row = run_t_test(
        serde_json::json!({"x_column": "x", "y_column": "y"}),
        vec![batch],
    )
    .await;
    assert_close(opt_f64(&row, "estimate"), -1.47, "estimate");
    assert_close(opt_f64(&row, "stderr"), 1.07961412859719, "stderr");
    assert_close(opt_f64(&row, "statistic"), -1.36159759404970, "statistic");
    assert_close(opt_f64(&row, "dof"), 6.97484737539948, "dof");
    assert_close(opt_f64(&row, "p_value"), 0.21566981730390, "p_value");
    assert_close(opt_f64(&row, "conf_low"), -4.02474919454316, "conf_low");
    assert_close(opt_f64(&row, "conf_high"), 1.08474919454316, "conf_high");
    assert_eq!(opt_i32(&row, "n"), Some(9));
}

/// R (2026-10-05): same wide table, var.equal=TRUE, per-column NA drop —
/// stderr 1.10628722697653, t -1.32876884425168, df 7,
/// p 0.22560080879041, CI (-4.08595360613604, 1.14595360613604).
#[tokio::test]
async fn wide_pooled_independent_na_matches_r() {
    let batch = wide_batch(
        &[Some(1.2), None, Some(2.4), Some(3.1), None, Some(4.7)],
        &[Some(2.1), Some(3.5), None, Some(4.0), Some(5.2), Some(6.8)],
    );
    let row = run_t_test(
        serde_json::json!({"x_column": "x", "y_column": "y", "var_equal": true}),
        vec![batch],
    )
    .await;
    assert_close(opt_f64(&row, "stderr"), 1.10628722697653, "stderr");
    assert_close(opt_f64(&row, "statistic"), -1.32876884425168, "statistic");
    assert_close(opt_f64(&row, "dof"), 7.0, "dof");
    assert_close(opt_f64(&row, "p_value"), 0.22560080879041, "p_value");
    assert_close(opt_f64(&row, "conf_low"), -4.08595360613604, "conf_low");
    assert_close(opt_f64(&row, "conf_high"), 1.14595360613604, "conf_high");
}

/// Fix-review R02 (2026-10-05): unpaired wide columns are independent
/// samples; NA rows must drop per column, never pairwise. R:
/// t.test(c(1,NA,5,10), c(0,3,NA,2)) → est 11/3, t 1.33394593769983,
/// df 2.45305039787798, p 0.29254219697638,
/// CI (-6.29498360849420, 13.62831694182753).
#[tokio::test]
async fn unpaired_wide_independent_na_matches_r_review_r02() {
    let batch = wide_batch(
        &[Some(1.0), None, Some(5.0), Some(10.0)],
        &[Some(0.0), Some(3.0), None, Some(2.0)],
    );
    let row = run_t_test(
        serde_json::json!({"x_column": "x", "y_column": "y"}),
        vec![batch],
    )
    .await;
    assert_close(opt_f64(&row, "estimate"), 11.0 / 3.0, "estimate");
    assert_close(opt_f64(&row, "statistic"), 1.33394593769983, "statistic");
    assert_close(opt_f64(&row, "dof"), 2.45305039787798, "dof");
    assert_close(opt_f64(&row, "p_value"), 0.29254219697638, "p_value");
    assert_close(opt_f64(&row, "conf_low"), -6.29498360849420, "conf_low");
    assert_close(opt_f64(&row, "conf_high"), 13.62831694182753, "conf_high");
}

/// Fix-review R03 (2026-10-05): mu is the null value in every mode.
/// R: t.test(c(1,10), c(0,2), paired=TRUE, mu=1) → t=1, p=0.5 (audit
/// fixture complete pairs; mu=0 keeps t=9/7≈1.285714).
#[tokio::test]
async fn paired_mu_is_honored_matches_r_review_r03() {
    let batch = wide_batch(
        &[Some(1.0), None, Some(5.0), Some(10.0)],
        &[Some(0.0), Some(3.0), None, Some(2.0)],
    );
    let row = run_t_test(
        serde_json::json!({"x_column": "x", "y_column": "y", "paired": true, "mu": 1.0}),
        vec![batch],
    )
    .await;
    assert_close(opt_f64(&row, "estimate"), 4.5, "estimate");
    assert_close(opt_f64(&row, "statistic"), 1.0, "statistic");
    assert_close(opt_f64(&row, "p_value"), 0.5, "p_value");
}

/// The emitted row conforms to the standard test-row schema, including the
/// three appended columns (stderr/conf_low/conf_high, nullable Float64).
#[tokio::test]
async fn t_test_row_matches_schema_with_ci_columns() {
    let row = run_t_test(
        serde_json::json!({"x_column": "x", "y_column": "y", "paired": true}),
        vec![audit_batch()],
    )
    .await;
    let expected = test_row_schema();
    assert_eq!(row.schema().as_ref(), expected.as_ref());
    assert!(
        row.schema()
            .field_with_name("stderr")
            .unwrap()
            .is_nullable()
    );
    assert!(
        row.schema()
            .field_with_name("conf_low")
            .unwrap()
            .is_nullable()
    );
    assert!(
        row.schema()
            .field_with_name("conf_high")
            .unwrap()
            .is_nullable()
    );
}

/// conf_level outside (0, 1) is rejected at build time.
#[test]
fn t_test_rejects_invalid_conf_level() {
    let node_ctx = NodeCtx::new(SessionContext::new().runtime_env(), None);
    for bad in [0.0, 1.0, -0.1, 1.5] {
        let result = TTestNodeFactory {}.build(
            serde_json::json!({"x_column": "x", "y_column": "y", "conf_level": bad}),
            node_ctx.clone(),
        );
        assert!(result.is_err(), "conf_level {bad} should be rejected");
    }
}

/// Every pair broken by a null → insufficient data error, not a silent
/// wrong-answer on misaligned columns.
#[tokio::test]
async fn paired_all_pairs_broken_errors() {
    let session = SessionContext::new();
    let node_ctx = NodeCtx::new(session.runtime_env(), None);
    let mut node = TTestNodeFactory {}
        .build(
            serde_json::json!({"x_column": "x", "y_column": "y", "paired": true}),
            node_ctx.clone(),
        )
        .unwrap();
    let batch = wide_batch(&[Some(1.0), None], &[None, Some(3.0)]);
    let df = session.read_batch(batch).unwrap();
    let err = node
        .execute(
            &node_ctx,
            &[NodeInput::new_dataframe(0, df)],
            &NodeReporter::noop(),
        )
        .await;
    assert!(err.is_err(), "no complete pair → must error");
}
