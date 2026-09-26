//! Reduced-rank regression node (`rrr`).
//!
//! Fits the multivariate model `Y = X·B + E` by (optionally weighted) least
//! squares and projects the coefficients onto their optimal rank-`k`
//! approximation via [`statkit::regression::reduced_rank_regression`] — the
//! exact Izenman estimator (per-response WLS → SVD of the centred fitted
//! matrix → `B_k = B_ols·V_kV_kᵀ`), replacing the cross-correlation SVD
//! approximation previously used by the factor pipeline.
//!
//! Outputs:
//! - **0** — tidy coefficients: one row per (response, predictor) with the
//!   intercept rows named `"intercept"`;
//! - **1** — scree: rank, variance explained (`d²/Σd²`), cumulative.

use std::sync::Arc;

use arrow_array::{
    Array, Float32Array, Float64Array, Int8Array, Int16Array, Int32Array, Int64Array, RecordBatch,
    StringArray, UInt8Array, UInt16Array, UInt32Array, UInt64Array,
};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use thiserror::Error;

use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
};

#[derive(Debug, Error)]
pub enum RrrError {
    #[error("missing column '{name}' in input DataFrame")]
    MissingColumn { name: String },
    #[error("column '{name}' is not numeric (got type: {dtype})")]
    NonNumericColumn { name: String, dtype: String },
    #[error("no input data: expected at least one row")]
    EmptyInput,
    #[error("no complete-case rows")]
    NoCompleteCases,
    #[error("rrr fit failed: {0}")]
    Fit(String),
    #[error("failed to build output: {0}")]
    Output(String),
    #[error("collect failed: {0}")]
    Collect(String),
}

impl ::dag_core::dag::NodeError for RrrError {
    fn node_type(&self) -> &str {
        "rrr"
    }
}

fn extract_column(batches: &[RecordBatch], name: &str) -> Result<Vec<f64>, RrrError> {
    let schema = batches
        .first()
        .map(|b| b.schema().clone())
        .ok_or(RrrError::EmptyInput)?;
    let idx = schema.index_of(name).map_err(|_| RrrError::MissingColumn {
        name: name.to_string(),
    })?;
    let dtype = schema.field(idx).data_type().clone();
    let is_numeric = matches!(
        dtype,
        DataType::Float16
            | DataType::Float32
            | DataType::Float64
            | DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
    );
    if !is_numeric {
        return Err(RrrError::NonNumericColumn {
            name: name.to_string(),
            dtype: dtype.to_string(),
        });
    }
    let mut values = Vec::new();
    for batch in batches {
        let col = batch.column(idx);
        extract_numeric_column(col, &mut values);
    }
    Ok(values)
}

fn extract_numeric_column(col: &dyn Array, out: &mut Vec<f64>) {
    macro_rules! cast {
        ($arr:expr, $T:ty) => {
            if let Some(a) = $arr.as_any().downcast_ref::<$T>() {
                for v in a.iter() {
                    out.push(match v {
                        Some(val) => val as f64,
                        None => f64::NAN,
                    });
                }
                return;
            }
        };
    }
    cast!(col, Int8Array);
    cast!(col, Int16Array);
    cast!(col, Int32Array);
    cast!(col, Int64Array);
    cast!(col, UInt8Array);
    cast!(col, UInt16Array);
    cast!(col, UInt32Array);
    cast!(col, UInt64Array);
    cast!(col, Float32Array);
    cast!(col, Float64Array);
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RrrNodeSpec {
    /// Predictor (X) column names.
    pub predictors: Vec<String>,
    /// Response (Y) column names.
    pub responses: Vec<String>,
    /// Rank of the coefficient restriction (default: full rank `min(p, r)`).
    #[serde(default)]
    pub rank: Option<usize>,
    /// Optional observation weight column (sampling weights → WLS).
    #[serde(default)]
    pub weight_column: Option<String>,
}

#[derive(Clone)]
pub struct RrrNode {
    meta: NodePorts,
    predictors: Vec<String>,
    responses: Vec<String>,
    rank: Option<usize>,
    weight_column: Option<String>,
}

impl RrrNode {
    pub fn new(spec: RrrNodeSpec) -> Self {
        Self {
            meta: NodePorts::new()
                .add_input_port(None)
                .add_output_port(None)
                .add_output_port(None),
            predictors: spec.predictors,
            responses: spec.responses,
            rank: spec.rank,
            weight_column: spec.weight_column,
        }
    }
}

#[async_trait]
impl DagNode for RrrNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "rrr"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or(RrrError::EmptyInput)?;
        let batches = input
            .dataframe()?
            .clone()
            .collect()
            .await
            .map_err(|e| RrrError::Collect(e.to_string()))?;

        let x_raw: Vec<Vec<f64>> = self
            .predictors
            .iter()
            .map(|c| extract_column(&batches, c))
            .collect::<Result<_, _>>()?;
        let y_raw: Vec<Vec<f64>> = self
            .responses
            .iter()
            .map(|c| extract_column(&batches, c))
            .collect::<Result<_, _>>()?;
        let w_raw = match &self.weight_column {
            Some(c) => Some(extract_column(&batches, c)?),
            None => None,
        };

        // Complete-case filter across predictors, responses, and weight.
        let n = y_raw.first().map(|c| c.len()).unwrap_or(0);
        let mut keep: Vec<usize> = Vec::with_capacity(n);
        for row in 0..n {
            let bad = x_raw.iter().any(|c| c[row].is_nan())
                || y_raw.iter().any(|c| c[row].is_nan())
                || w_raw.as_ref().is_some_and(|w| w[row].is_nan());
            if !bad {
                keep.push(row);
            }
        }
        if keep.len() < self.predictors.len() + self.responses.len() + 2 {
            return Err(RrrError::NoCompleteCases.into());
        }

        let subset = |c: &Vec<f64>| -> Vec<f64> { keep.iter().map(|&i| c[i]).collect() };
        let x: Vec<Vec<f64>> = x_raw.iter().map(subset).collect();
        let y: Vec<Vec<f64>> = y_raw.iter().map(subset).collect();
        let w = w_raw.as_ref().map(|c| subset(c));

        let x_refs: Vec<&[f64]> = x.iter().map(|v| v.as_slice()).collect();
        let y_refs: Vec<&[f64]> = y.iter().map(|v| v.as_slice()).collect();
        let result =
            statkit::regression::reduced_rank_regression(&x_refs, &y_refs, w.as_deref(), self.rank)
                .map_err(|e| RrrError::Fit(e.to_string()))?;

        // ── Port 0: tidy coefficients (response, predictor, coefficient). ──
        let mut resp_names: Vec<String> = Vec::new();
        let mut pred_names: Vec<String> = Vec::new();
        let mut coefs: Vec<f64> = Vec::new();
        let mut pred_iter: Vec<String> = vec!["intercept".to_string()];
        pred_iter.extend(self.predictors.iter().cloned());
        for (j, resp) in self.responses.iter().enumerate() {
            for (i, pred) in pred_iter.iter().enumerate() {
                resp_names.push(resp.clone());
                pred_names.push(pred.clone());
                coefs.push(if i == 0 {
                    result.intercept[j]
                } else {
                    result.coefficients[i - 1][j]
                });
            }
        }
        let coef_batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("response", DataType::Utf8, false),
                Field::new("predictor", DataType::Utf8, false),
                Field::new("coefficient", DataType::Float64, false),
            ])),
            vec![
                Arc::new(StringArray::from(resp_names)),
                Arc::new(StringArray::from(pred_names)),
                Arc::new(Float64Array::from(coefs)),
            ],
        )
        .map_err(|e| RrrError::Output(e.to_string()))?;

        // ── Port 1: scree (rank, variance_explained, cumulative). ──
        let ranks: Vec<i32> = (1..=result.singular_values.len())
            .map(|v| v as i32)
            .collect();
        let scree_batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("rank", DataType::Int32, false),
                Field::new("variance_explained", DataType::Float64, false),
                Field::new("cumulative", DataType::Float64, false),
            ])),
            vec![
                Arc::new(Int32Array::from(ranks)),
                Arc::new(Float64Array::from(result.variance_explained.clone())),
                Arc::new(Float64Array::from(result.cumulative.clone())),
            ],
        )
        .map_err(|e| RrrError::Output(e.to_string()))?;

        let ctx = node_ctx.session();
        let df_coefs = ctx
            .read_batch(coef_batch)
            .map_err(|e| RrrError::Output(format!("read_batch: {e}")))?;
        let df_scree = ctx
            .read_batch(scree_batch)
            .map_err(|e| RrrError::Output(format!("read_batch: {e}")))?;
        let mut res = PortOutputs::new();
        res.insert(0, df_coefs);
        res.insert(1, df_scree);
        Ok(res)
    }
}

pub struct RrrNodeFactory {}

impl NodeFactory for RrrNodeFactory {
    fn kind(&self) -> &'static str {
        "rrr"
    }
    fn desc(&self) -> &'static str {
        "Reduced-rank regression (rank-k coefficients + variance explained)."
    }
    fn doc(&self) -> &'static str {
        "Fits Y = X·B by (optionally weighted) least squares and restricts B \
         to rank k via the SVD of the centred fitted matrix (Izenman's \
         reduced-rank regression): B_k = B_ols·V_k·V_kᵀ, with the intercept \
         re-added from the weighted means. Outputs tidy rank-k coefficients \
         and the variance-explained scree (d²/Σd² per canonical direction)."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(RrrNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(None)
            .add_output_port(None)
            .add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: RrrNodeSpec = serde_json::from_value(spec)?;
        let reject = |reason: String| dag_core::registry::error::Error::SpecRejection {
            kind: "rrr".to_string(),
            reason,
            schema_pretty: serde_json::to_string_pretty(&self.spec_schema()).unwrap_or_default(),
        };
        if s.predictors.is_empty() || s.responses.is_empty() {
            return Err(reject("predictors and responses must be non-empty".into()));
        }
        if s.rank.is_some_and(|k| k == 0) {
            return Err(reject("rank must be ≥ 1".into()));
        }
        Ok(Box::new(RrrNode::new(s)))
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn node_ctx() -> NodeCtx {
        NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        )
    }

    fn make_batch() -> RecordBatch {
        // y1, y2 driven by one shared direction (f = 2·x1 − x2): rank 1 is
        // essentially lossless; y3 adds an idiosyncratic direction.
        let n = 60;
        let x1: Vec<f64> = (0..n).map(|i| ((i * 7) % 13) as f64 / 13.0).collect();
        let x2: Vec<f64> = (0..n)
            .map(|i| (i as f64 * 0.6180339887498949).fract())
            .collect();
        let f: Vec<f64> = x1.iter().zip(&x2).map(|(&a, &b)| 2.0 * a - b).collect();
        let y1: Vec<f64> = f.iter().map(|&v| 1.0 + v).collect();
        let y2: Vec<f64> = f.iter().map(|&v| 3.0 - 2.0 * v).collect();
        let y3: Vec<f64> = (0..n)
            .map(|i| 0.5 + x2[i] * 0.3 + ((i as f64 * 0.41421356).fract() - 0.5) * 0.1)
            .collect();
        let wt: Vec<f64> = (0..n).map(|i| 1000.0 + 50.0 * (i % 7) as f64).collect();
        let to_arr = |v: Vec<f64>| Arc::new(Float64Array::from(v)) as Arc<dyn arrow_array::Array>;
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("x1", DataType::Float64, false),
                Field::new("x2", DataType::Float64, false),
                Field::new("y1", DataType::Float64, false),
                Field::new("y2", DataType::Float64, false),
                Field::new("y3", DataType::Float64, false),
                Field::new("wt", DataType::Float64, false),
            ])),
            vec![
                to_arr(x1),
                to_arr(x2),
                to_arr(y1),
                to_arr(y2),
                to_arr(y3),
                to_arr(wt),
            ],
        )
        .unwrap()
    }

    async fn run(spec: serde_json::Value) -> (Vec<RecordBatch>, Vec<RecordBatch>) {
        let mut node = RrrNodeFactory {}.build(spec, node_ctx()).unwrap();
        let input = dag_core::node::NodeInput::new_dataframe(
            0,
            datafusion::prelude::SessionContext::new()
                .read_batch(make_batch())
                .unwrap(),
        );
        let outs = node
            .execute(
                &node_ctx(),
                &[input],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let coefs = outs.dataframe(0).unwrap().clone().collect().await.unwrap();
        let scree = outs.dataframe(1).unwrap().clone().collect().await.unwrap();
        (coefs, scree)
    }

    fn coef_cell(rows: &[RecordBatch], response: &str, predictor: &str) -> f64 {
        let batch = &rows[0];
        let resp = batch
            .column(batch.schema().index_of("response").unwrap())
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let pred = batch
            .column(batch.schema().index_of("predictor").unwrap())
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let val = batch
            .column(batch.schema().index_of("coefficient").unwrap())
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        for i in 0..batch.num_rows() {
            if resp.value(i) == response && pred.value(i) == predictor {
                return val.value(i);
            }
        }
        panic!("coefficient ({response}, {predictor}) not found");
    }

    #[tokio::test]
    async fn rank1_end_to_end() {
        let (coefs, scree) = run(serde_json::json!({
            "predictors": ["x1", "x2"],
            "responses": ["y1", "y2"],
            "rank": 1,
        }))
        .await;
        // y1 = 1 + f exactly: rank-1 recovers the OLS fit for these two.
        assert!((coef_cell(&coefs, "y1", "intercept") - 1.0).abs() < 1e-9);
        assert!((coef_cell(&coefs, "y1", "x1") - 2.0).abs() < 1e-9);
        assert!((coef_cell(&coefs, "y1", "x2") + 1.0).abs() < 1e-9);
        assert!((coef_cell(&coefs, "y2", "intercept") - 3.0).abs() < 1e-9);
        // Tidy layout: 2 responses × 3 rows.
        assert_eq!(coefs.iter().map(|b| b.num_rows()).sum::<usize>(), 6);
        // Scree: 2 directions, first explains ~everything.
        let ve = scree[0]
            .column(scree[0].schema().index_of("variance_explained").unwrap())
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!(ve.value(0) > 0.999);
        assert_eq!(scree.iter().map(|b| b.num_rows()).sum::<usize>(), 2);
    }

    #[tokio::test]
    async fn full_rank_matches_weighted_ols() {
        let (coefs, _scree) = run(serde_json::json!({
            "predictors": ["x1", "x2"],
            "responses": ["y1", "y2", "y3"],
            "weight_column": "wt",
        }))
        .await;
        // No rank restriction ⇒ per-response WLS with the same weights.
        let n = 60;
        let x1: Vec<f64> = (0..n).map(|i| ((i * 7) % 13) as f64 / 13.0).collect();
        let x2: Vec<f64> = (0..n)
            .map(|i| (i as f64 * 0.6180339887498949).fract())
            .collect();
        let f: Vec<f64> = x1.iter().zip(&x2).map(|(&a, &b)| 2.0 * a - b).collect();
        let y1: Vec<f64> = f.iter().map(|&v| 1.0 + v).collect();
        let wt: Vec<f64> = (0..n).map(|i| 1000.0 + 50.0 * (i % 7) as f64).collect();
        let wls = statkit::regression::wls(&[&x1, &x2], &y1, &wt, true).unwrap();
        assert!((coef_cell(&coefs, "y1", "intercept") - wls.coefficients[0]).abs() < 1e-8);
        assert!((coef_cell(&coefs, "y1", "x1") - wls.coefficients[1]).abs() < 1e-8);
        assert!((coef_cell(&coefs, "y1", "x2") - wls.coefficients[2]).abs() < 1e-8);
    }

    #[tokio::test]
    async fn rank_zero_rejected_at_build() {
        let err = RrrNodeFactory {}.build(
            serde_json::json!({
                "predictors": ["x1"],
                "responses": ["y1"],
                "rank": 0,
            }),
            node_ctx(),
        );
        assert!(err.is_err());
    }
}
