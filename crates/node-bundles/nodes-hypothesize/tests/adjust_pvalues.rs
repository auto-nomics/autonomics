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

// ── F04: full-family correction across batches, null rows preserved ─────────
// Golden values from R 4.6.1 `p.adjust(p, method, n = <total rows incl NA>)`
// (epsilon 1e-14). The node's documented default n counts null rows into the
// family size — equivalent to R only when n is passed explicitly (R's own
// default n lazily re-evaluates length(p) AFTER the NA drop, a known quirk
// deliberately not replicated here).

fn nullable_pcol_batch(p_values: &[Option<f64>]) -> RecordBatch {
    let schema = Arc::new(Schema::new(vec![Field::new(
        "p_value",
        DataType::Float64,
        true,
    )]));
    RecordBatch::try_new(schema, vec![Arc::new(Float64Array::from(p_values.to_vec()))]).unwrap()
}

async fn run_adjust_batches(
    spec: serde_json::Value,
    batches: Vec<RecordBatch>,
) -> Vec<RecordBatch> {
    let session = SessionContext::new();
    let node_ctx = NodeCtx::new(session.runtime_env(), None);
    let mut node = AdjustNodeFactory {}
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
        .unwrap();
    outputs
        .dataframe(0)
        .unwrap()
        .clone()
        .collect()
        .await
        .unwrap()
}

fn collect_opt_f64(batch: &RecordBatch, col: &str) -> Vec<Option<f64>> {
    batch
        .column_by_name(col)
        .unwrap()
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap()
        .iter()
        .map(|v| v)
        .collect()
}

fn collect_opt_i32(batch: &RecordBatch, col: &str) -> Vec<Option<i32>> {
    batch
        .column_by_name(col)
        .unwrap()
        .as_any()
        .downcast_ref::<Int32Array>()
        .unwrap()
        .iter()
        .map(|v| v)
        .collect()
}

/// F04 regression: two batches of 2 rows each used to fail with
/// "all columns in a record batch must have the same length" (the adjusted
/// vector of the whole family was appended to the first batch only). The
/// family [0.001, NA | 0.02, 0.5] is adjusted once with n = 4 (null row
/// counts toward n) and backfilled per batch, order preserved.
/// R: p.adjust(c(0.001, 0.02, 0.5), "BH", n = 4) = c(0.004, 0.04, 0.66666666666666663).
#[tokio::test]
async fn adjust_pvalues_two_batches_with_null_matches_r() {
    let out = run_adjust_batches(
        serde_json::json!({"method": "BH"}),
        vec![
            nullable_pcol_batch(&[Some(0.001), None]),
            nullable_pcol_batch(&[Some(0.02), Some(0.5)]),
        ],
    )
    .await;

    let rows: usize = out.iter().map(|b| b.num_rows()).sum();
    assert_eq!(rows, 4, "row count must be preserved");
    let p_adj: Vec<Option<f64>> = out.iter().flat_map(|b| collect_opt_f64(b, "p_adj")).collect();
    let reject: Vec<Option<i32>> = out.iter().flat_map(|b| collect_opt_i32(b, "reject")).collect();
    let raw: Vec<Option<f64>> = out
        .iter()
        .flat_map(|b| collect_opt_f64(b, "p_value"))
        .collect();

    // order preserved across the batch seam
    assert_eq!(
        raw,
        vec![Some(0.001), None, Some(0.02), Some(0.5)],
        "raw p column must pass through untouched"
    );
    let expected = [
        Some(0.004),
        None,
        Some(0.04),
        Some(0.666_666_666_666_666_63),
    ];
    for (got, exp) in p_adj.iter().zip(expected) {
        match (got, exp) {
            (Some(g), Some(e)) => assert!(
                (g - e).abs() < 1e-14,
                "p_adj got {g}, expected {e}"
            ),
            (None, None) => {}
            other => panic!("p_adj mismatch: {other:?}"),
        }
    }
    // null p → null reject (not 0)
    assert_eq!(reject, vec![Some(1), None, Some(1), Some(0)]);
}

/// R: p.adjust(c(0.01, 0.02), "BH", n = 10) = c(0.1, 0.1) — the n_total
/// override keeps R's rank denominator on the estimable count.
#[tokio::test]
async fn adjust_pvalues_n_total_override_matches_r() {
    let out = run_adjust_batches(
        serde_json::json!({"method": "BH", "n_total": 10}),
        vec![nullable_pcol_batch(&[Some(0.01), Some(0.02)])],
    )
    .await;
    let p_adj: Vec<Option<f64>> = out.iter().flat_map(|b| collect_opt_f64(b, "p_adj")).collect();
    for (got, exp) in p_adj.iter().zip([Some(0.1), Some(0.1)]) {
        assert!((got.unwrap() - exp.unwrap()).abs() < 1e-14);
    }

    // BY, Holm, Bonferroni with the same override (R goldens).
    // R: p.adjust(c(0.01, 0.02), "BY", n = 10)  = c(0.29289682539682538, …)
    // R: p.adjust(c(0.01, 0.02), "holm", n = 10) = c(0.1, 0.18)
    // R: p.adjust(c(0.01, 0.02), "bonferroni", n = 10) = c(0.1, 0.2)
    for (method, expected) in [
        ("BY", vec![0.292_896_825_396_825_38, 0.292_896_825_396_825_38]),
        ("holm", vec![0.1, 0.18]),
        ("bonferroni", vec![0.1, 0.2]),
    ] {
        let out = run_adjust_batches(
            serde_json::json!({"method": method, "n_total": 10}),
            vec![nullable_pcol_batch(&[Some(0.01), Some(0.02)])],
        )
        .await;
        let p_adj: Vec<Option<f64>> =
            out.iter().flat_map(|b| collect_opt_f64(b, "p_adj")).collect();
        for (got, exp) in p_adj.iter().zip(expected) {
            assert!(
                (got.unwrap() - exp).abs() < 1e-14,
                "{method}: got {:?}, expected {exp}",
                got
            );
        }
    }
}

/// R: p.adjust(c(0.01, NA), "BH", n = 2) = c(0.02, NA) — the null row joins
/// the family (n = 2) but keeps a null p_adj/reject.
#[tokio::test]
async fn adjust_pvalues_null_row_counts_toward_n() {
    let out = run_adjust_batches(
        serde_json::json!({"method": "BH"}),
        vec![nullable_pcol_batch(&[Some(0.01), None])],
    )
    .await;
    let p_adj: Vec<Option<f64>> = out.iter().flat_map(|b| collect_opt_f64(b, "p_adj")).collect();
    assert_eq!(p_adj.len(), 2);
    assert!((p_adj[0].unwrap() - 0.02).abs() < 1e-14);
    assert_eq!(p_adj[1], None);
    let reject: Vec<Option<i32>> = out.iter().flat_map(|b| collect_opt_i32(b, "reject")).collect();
    assert_eq!(reject, vec![Some(1), None]);
}

/// R: p.adjust(c(NA, NA), "BH") = c(NA, NA).
#[tokio::test]
async fn adjust_pvalues_all_null_family() {
    let out = run_adjust_batches(
        serde_json::json!({"method": "BH"}),
        vec![nullable_pcol_batch(&[None, None])],
    )
    .await;
    let p_adj: Vec<Option<f64>> = out.iter().flat_map(|b| collect_opt_f64(b, "p_adj")).collect();
    let reject: Vec<Option<i32>> = out.iter().flat_map(|b| collect_opt_i32(b, "reject")).collect();
    assert_eq!(p_adj, vec![None, None]);
    assert_eq!(reject, vec![None, None]);
}

/// Omitted method keeps the Bonferroni default (no silent change) and emits
/// a runtime WARN. R: p.adjust(c(0.001,0.01,0.02,0.5), "bonferroni", n=4)
/// = c(0.004, 0.04, 0.08, 1).
#[tokio::test]
async fn adjust_pvalues_default_method_is_bonferroni() {
    let out = run_adjust_batches(
        serde_json::json!({}),
        vec![nullable_pcol_batch(&[Some(0.001), Some(0.01), Some(0.02), Some(0.5)])],
    )
    .await;
    let p_adj: Vec<Option<f64>> = out.iter().flat_map(|b| collect_opt_f64(b, "p_adj")).collect();
    for (got, exp) in p_adj.iter().zip([0.004, 0.04, 0.08, 1.0]) {
        assert!((got.unwrap() - exp).abs() < 1e-14);
    }
}

/// R: stopifnot(n >= lp) — an n_total smaller than the estimable row count
/// is an error, not a silent re-weighting.
#[tokio::test]
async fn adjust_pvalues_n_total_below_estimable_errors() {
    let session = SessionContext::new();
    let node_ctx = NodeCtx::new(session.runtime_env(), None);
    let mut node = AdjustNodeFactory {}
        .build(serde_json::json!({"method": "BH", "n_total": 1}), node_ctx.clone())
        .unwrap();
    let df = session
        .read_batch(nullable_pcol_batch(&[Some(0.1), Some(0.2)]))
        .unwrap();
    let err = node
        .execute(
            &node_ctx,
            &[NodeInput::new_dataframe(0, df)],
            &NodeReporter::noop(),
        )
        .await;
    assert!(err.is_err(), "n_total < estimable rows must error");
}

/// p_adj/reject columns are nullable now; the nullability itself is pinned
/// by the schema.
#[tokio::test]
async fn adjust_pvalues_appended_columns_nullable() {
    let out = run_adjust_batches(
        serde_json::json!({"method": "BH"}),
        vec![nullable_pcol_batch(&[Some(0.1)])],
    )
    .await;
    let schema = out[0].schema();
    assert_eq!(schema.field_with_name("p_adj").unwrap().is_nullable(), true);
    assert_eq!(schema.field_with_name("reject").unwrap().is_nullable(), true);
}
