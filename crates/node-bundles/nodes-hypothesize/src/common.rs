//! Shared infrastructure for `hypothesize.*` DAG nodes.
//!
//! Every primitive test node emits a single-row `RecordBatch` conforming to
//! the **standard test-row schema** (design doc §2.2). This module provides
//! helpers to build that batch from a [`hypothesize::HypothesisTest`] and to
//! extract numeric columns from upstream data.

use std::sync::Arc;

use arrow_array::{Array, ArrayRef, Float64Array, Int32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use thiserror::Error;

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::registry::NodeCtx;

// ─── Error ──────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum HypoNodeError {
    #[error("column '{0}' not found or wrong type")]
    Column(String),
    #[error("insufficient data: {0}")]
    Insufficient(String),
    #[error("invalid spec: {0}")]
    Spec(String),
    #[error("test failed: {0}")]
    Test(String),
    #[error("test-row output assembly failed: {0}")]
    Output(String),
    #[error("collect failed: {0}")]
    Collect(String),
    #[error("read_batch failed: {0}")]
    ReadBatch(String),
}

impl ::dag_core::dag::NodeError for HypoNodeError {
    fn node_type(&self) -> &str {
        "hypothesize"
    }
}

// ─── Standard test-row schema ───────────────────────────────────────────────

/// Build the standard test-row output schema (design doc §2.2).
///
/// `p_value` is nullable: nodes with NA-propagation semantics (e.g.
/// `hypothesize.boolean_test` over component tests that could not be
/// estimated) emit a null p-value instead of silently dropping the row.
pub fn test_row_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("statistic", DataType::Float64, false),
        Field::new("p_value", DataType::Float64, true),
        Field::new("dof", DataType::Float64, false),
        Field::new("alternative", DataType::Utf8, false),
        Field::new("method", DataType::Utf8, false),
        Field::new("estimate", DataType::Float64, true),
        Field::new("null_value", DataType::Float64, true),
        Field::new("n", DataType::Int32, true),
        Field::new("stderr", DataType::Float64, true),
        Field::new("conf_low", DataType::Float64, true),
        Field::new("conf_high", DataType::Float64, true),
    ]))
}

/// Convert a [`hypothesize::HypothesisTest`] into a single-row `RecordBatch`
/// using the standard test-row schema.
pub fn test_to_batch(
    _ctx: &NodeCtx,
    t: &hypothesize::HypothesisTest,
) -> Result<RecordBatch, HypoNodeError> {
    test_to_batch_with_p(_ctx, t, Some(t.p_value))
}

/// Like [`test_to_batch`], but with an explicit (possibly null) p-value that
/// overrides `t.p_value`. Used by nodes whose NA semantics live at the node
/// layer: a [`hypothesize::HypothesisTest`] cannot represent a missing
/// p-value (plain `f64`), so such nodes build the shell test and pass the
/// null-aware p-value separately.
pub fn test_to_batch_with_p(
    _ctx: &NodeCtx,
    t: &hypothesize::HypothesisTest,
    p_value: Option<f64>,
) -> Result<RecordBatch, HypoNodeError> {
    let schema = test_row_schema();
    let estimate = t.extra_f64("estimate");
    let null_value = t.extra_f64("null_value");
    let n = t.extra_f64("n").map(|v| v as i32);
    let stderr = t.extra_f64("stderr");
    let conf_low = t.extra_f64("conf_low");
    let conf_high = t.extra_f64("conf_high");

    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Float64Array::from(vec![t.stat])),
            Arc::new(Float64Array::from(vec![p_value])),
            Arc::new(Float64Array::from(vec![t.dof])),
            Arc::new(StringArray::from(vec![t.alternative.as_r_str()])),
            Arc::new(StringArray::from(vec![t.method])),
            opt_f64_array(estimate),
            opt_f64_array(null_value),
            opt_i32_array(n),
            opt_f64_array(stderr),
            opt_f64_array(conf_low),
            opt_f64_array(conf_high),
        ],
    )
    .map_err(|e| HypoNodeError::Output(e.to_string()))?;
    Ok(batch)
}

/// Register a single-row batch as a DataFrame on a fresh session and return
/// it wrapped in `PortOutputs` for output port 0.
pub fn emit_test_row(
    node_ctx: &NodeCtx,
    t: &hypothesize::HypothesisTest,
) -> Result<PortOutputs, DagError> {
    emit_test_row_with_p(node_ctx, t, Some(t.p_value))
}

/// Like [`emit_test_row`], but emitting an explicit (possibly null) p-value
/// (see [`test_to_batch_with_p`]).
pub fn emit_test_row_with_p(
    node_ctx: &NodeCtx,
    t: &hypothesize::HypothesisTest,
    p_value: Option<f64>,
) -> Result<PortOutputs, DagError> {
    let batch = test_to_batch_with_p(node_ctx, t, p_value).map_err(DagError::from)?;
    let session = node_ctx.session();
    let df = session.read_batch(batch).map_err(|e| DagError::NodeError {
        node_type: "hypothesize".to_string(),
        msg: format!("read_batch failed: {e}"),
    })?;
    let mut res = PortOutputs::new();
    res.insert(0, df);
    Ok(res)
}

fn opt_f64_array(v: Option<f64>) -> ArrayRef {
    Arc::new(Float64Array::from(vec![v]))
}

fn opt_i32_array(v: Option<i32>) -> ArrayRef {
    // A missing value must still produce an Int32-typed array (null slot),
    // never a NullArray: RecordBatch::try_new requires the column's physical
    // type to match the schema's `Int32`, even for an all-null column.
    Arc::new(Int32Array::from(vec![v]))
}

// ─── Column extraction helpers ──────────────────────────────────────────────

/// Collect all input data into a single `RecordBatch`.
pub async fn collect_input(
    inputs: &[dag_core::node::NodeInput],
) -> Result<Vec<RecordBatch>, HypoNodeError> {
    let input = inputs
        .first()
        .ok_or_else(|| HypoNodeError::Insufficient("no input connected".into()))?;
    input
        .dataframe()
        .map_err(|e| HypoNodeError::Collect(e.to_string()))?
        .clone()
        .collect()
        .await
        .map_err(|e| HypoNodeError::Collect(e.to_string()))
}

/// Extract a numeric column (f64) from a set of `RecordBatch`es.
pub fn extract_f64_column(
    batches: &[RecordBatch],
    column: &str,
) -> Result<Vec<f64>, HypoNodeError> {
    let mut values = Vec::new();
    for batch in batches {
        let arr = batch
            .column_by_name(column)
            .ok_or_else(|| HypoNodeError::Column(column.to_string()))?;
        if let Some(a) = arr.as_any().downcast_ref::<Float64Array>() {
            for i in 0..a.len() {
                if !a.is_null(i) {
                    values.push(a.value(i));
                }
            }
        } else {
            return Err(HypoNodeError::Column(format!("{column} is not Float64")));
        }
    }
    Ok(values)
}

/// Null-aware variant of [`extract_f64_column`]: preserves Arrow nulls as
/// `None`, keeping row order across batches. Nodes whose statistics must
/// propagate missingness (e.g. `boolean_test`, `adjust_pvalues`) use this
/// instead of silently dropping null rows.
pub fn extract_opt_f64_column(
    batches: &[RecordBatch],
    column: &str,
) -> Result<Vec<Option<f64>>, HypoNodeError> {
    let mut values = Vec::new();
    for batch in batches {
        let arr = batch
            .column_by_name(column)
            .ok_or_else(|| HypoNodeError::Column(column.to_string()))?;
        if let Some(a) = arr.as_any().downcast_ref::<Float64Array>() {
            for i in 0..a.len() {
                values.push(if a.is_null(i) { None } else { Some(a.value(i)) });
            }
        } else {
            return Err(HypoNodeError::Column(format!("{column} is not Float64")));
        }
    }
    Ok(values)
}

/// Extract two columns as row-aligned complete pairs (R complete-case
/// semantics): a row contributes a pair only when both sides are non-null;
/// any row with a null on either side drops the whole pair (counted in
/// `n_dropped`). Row order is preserved across batches. Errors if either
/// column is missing from a batch, is not Float64, or the two columns'
/// total lengths differ.
pub fn extract_paired_columns(
    batches: &[RecordBatch],
    x_col: &str,
    y_col: &str,
) -> Result<(Vec<(f64, f64)>, usize), HypoNodeError> {
    let mut pairs = Vec::new();
    let mut n_dropped = 0usize;
    let mut x_total = 0usize;
    let mut y_total = 0usize;
    for batch in batches {
        let x_arr = batch
            .column_by_name(x_col)
            .ok_or_else(|| HypoNodeError::Column(x_col.to_string()))?;
        let y_arr = batch
            .column_by_name(y_col)
            .ok_or_else(|| HypoNodeError::Column(y_col.to_string()))?;
        let xs = x_arr
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or_else(|| HypoNodeError::Column(format!("{x_col} not Float64")))?;
        let ys = y_arr
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or_else(|| HypoNodeError::Column(format!("{y_col} not Float64")))?;
        x_total += xs.len();
        y_total += ys.len();
        for i in 0..xs.len() {
            if !xs.is_null(i) && !ys.is_null(i) {
                pairs.push((xs.value(i), ys.value(i)));
            } else {
                n_dropped += 1;
            }
        }
    }
    if x_total != y_total {
        return Err(HypoNodeError::Spec(format!(
            "columns '{x_col}' ({x_total} rows) and '{y_col}' ({y_total} rows) must have the same length"
        )));
    }
    Ok((pairs, n_dropped))
}

/// Extract a group-label column from batches, returning group→indices mapping
/// preserving first-seen order.
pub fn extract_groups(
    batches: &[RecordBatch],
    value_col: &str,
    group_col: &str,
) -> Result<Vec<(String, Vec<f64>)>, HypoNodeError> {
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();

    for batch in batches {
        let val_arr = batch
            .column_by_name(value_col)
            .ok_or_else(|| HypoNodeError::Column(value_col.to_string()))?;
        let grp_arr = batch
            .column_by_name(group_col)
            .ok_or_else(|| HypoNodeError::Column(group_col.to_string()))?;
        let vals = val_arr
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or_else(|| HypoNodeError::Column(format!("{value_col} not Float64")))?;

        let grp_strings: Vec<Option<String>> =
            dag_core::node::string_opt_values(grp_arr.as_ref()).unwrap_or_default();

        for i in 0..vals.len() {
            if vals.is_null(i) {
                continue;
            }
            if let Some(g) = &grp_strings[i] {
                if !groups.contains_key(g) {
                    order.push(g.clone());
                }
                groups.entry(g.clone()).or_default().push(vals.value(i));
            }
        }
    }

    Ok(order
        .into_iter()
        .map(|g| {
            let vals = groups.remove(&g).unwrap_or_default();
            (g, vals)
        })
        .collect())
}
