//! Robust linear regression (M-estimator) node.
//!
//! Wraps [`statkit::regression::rlm`] — MASS `rlm` parity (Huber /
//! Tukey bisquare ψ, case weights, MAD scale). The sensitivity-analysis
//! estimator of Li et al. 2026 Table S5.
//!
//! Output (single row, one `coef_/se_/t_/p_` quartet per term including
//! the intercept):
//!
//! | Column              | Type    | Description                        |
//! |---------------------|---------|------------------------------------|
//! | `coef_{term}`       | Float64 | Robust coefficient                 |
//! | `se_{term}`         | Float64 | Sandwich standard error            |
//! | `t_{term}`          | Float64 | t-statistic                        |
//! | `p_{term}`          | Float64 | Two-sided p (Student-t, df = n−p)  |
//! | `sigma_robust`      | Float64 | Final MAD scale estimate           |
//! | `converged`         | Int32   | 1 if the IRLS converged            |
//! | `n_iter`            | Int32   | IRLS iterations used               |
//! | `n_obs`             | Int32   | Complete-case observations          |

use std::sync::Arc;

use arrow_array::{Float64Array, Int32Array, RecordBatch};
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
use statkit::regression::{PsiFunction, RlmOptions, rlm};

#[derive(Debug, Error)]
pub enum RlmError {
    #[error("{0}")]
    Column(String),
    #[error("{0}")]
    Fit(String),
    #[error("collect failed: {0}")]
    Collect(String),
    #[error("read_batch failed: {0}")]
    ReadBatch(String),
}

impl ::dag_core::dag::NodeError for RlmError {
    fn node_type(&self) -> &str {
        "rlm"
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RlmNodeSpec {
    /// Outcome column name.
    pub outcome_column: String,
    /// Predictor column names.
    pub predictors: Vec<String>,
    /// Sampling-weight column name; `null` → unit weights (MASS
    /// `wt.method = "case"` semantics either way).
    #[serde(default)]
    pub weight_column: Option<String>,
    /// ψ function: "huber" (default) or "tukey" (bisquare).
    #[serde(default = "default_psi")]
    pub psi: String,
    /// Tuning constant: Huber k (default 1.345) or Tukey c (4.685),
    /// applied per `psi`.
    #[serde(default)]
    pub tuning_const: Option<f64>,
    /// Max IRLS iterations (default 20, MASS default).
    #[serde(default = "default_max_iter")]
    pub max_iter: usize,
    /// Convergence tolerance (default 1e-4, MASS default).
    #[serde(default = "default_acc")]
    pub acc: f64,
}

fn default_psi() -> String {
    "huber".to_string()
}
fn default_max_iter() -> usize {
    20
}
fn default_acc() -> f64 {
    1e-4
}

fn parse_psi(s: &str) -> Result<PsiFunction, RlmError> {
    match s {
        "huber" => Ok(PsiFunction::Huber),
        "tukey" => Ok(PsiFunction::TukeyBisquare),
        other => Err(RlmError::Column(format!(
            "psi must be \"huber\" or \"tukey\", got \"{other}\""
        ))),
    }
}

#[derive(Clone)]
pub struct RlmNode {
    meta: NodePorts,
    outcome_column: String,
    predictors: Vec<String>,
    weight_column: Option<String>,
    opts: RlmOptions,
}

pub struct RlmNodeFactory {}

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None).add_input_port(None)
}

impl NodeFactory for RlmNodeFactory {
    fn kind(&self) -> &'static str {
        "rlm"
    }
    fn desc(&self) -> &'static str {
        "Robust linear regression (M-estimator, MASS rlm parity)."
    }
    fn doc(&self) -> &'static str {
        "Iteratively reweighted least squares with Huber or Tukey-bisquare \
        ψ functions, MAD scale estimation, and case weights — numerically \
        equivalent to MASS::rlm(method=\"M\", wt.method=\"case\", \
        scale.est=\"MAD\"). Reports sandwich standard errors and \
        observation-wise robust weights; use for outlier-robust \
        sensitivity analyses of weighted survey regressions."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(RlmNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: RlmNodeSpec = serde_json::from_value(spec)?;
        let psi = parse_psi(&s.psi)
            .map_err(|e| dag_core::registry::error::Error::Unknown(e.to_string()))?;
        let opts = RlmOptions {
            psi,
            huber_k: s.tuning_const.unwrap_or(1.345),
            tukey_c: s.tuning_const.unwrap_or(4.685),
            max_iter: s.max_iter,
            acc: s.acc,
        };
        Ok(Box::new(RlmNode {
            meta: port_layout(),
            outcome_column: s.outcome_column,
            predictors: s.predictors,
            weight_column: s.weight_column,
            opts,
        }))
    }
}

#[async_trait]
impl DagNode for RlmNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        "rlm"
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
        let input = inputs
            .first()
            .ok_or(RlmError::Column("no input connected".to_string()))?;
        let batches = input
            .dataframe()?
            .clone()
            .collect()
            .await
            .map_err(|e| RlmError::Collect(e.to_string()))?;

        let y_raw = dag_core::arrow_util::extract_numeric_lenient(&batches, &self.outcome_column)?;
        let x_raw: Vec<Vec<f64>> = self
            .predictors
            .iter()
            .map(|c| dag_core::arrow_util::extract_numeric_lenient(&batches, c))
            .collect::<Result<_, _>>()
            .map_err(|e: dag_core::arrow_util::ColumnError| RlmError::Column(e.to_string()))?;
        let w_raw = match &self.weight_column {
            Some(c) => Some(dag_core::arrow_util::extract_numeric_lenient(&batches, c)?),
            None => None,
        };

        // Complete-case filter across every column in play.
        let n = y_raw.len();
        let mut y = Vec::with_capacity(n);
        let mut w: Vec<f64> = Vec::with_capacity(n);
        let mut x_filtered: Vec<Vec<f64>> = vec![Vec::with_capacity(n); self.predictors.len()];
        for i in 0..n {
            if y_raw[i].is_nan()
                || x_raw.iter().any(|c| c[i].is_nan())
                || w_raw.as_ref().is_some_and(|w| w[i].is_nan())
            {
                continue;
            }
            y.push(y_raw[i]);
            if let Some(wr) = &w_raw {
                w.push(wr[i]);
            }
            for (j, c) in x_raw.iter().enumerate() {
                x_filtered[j].push(c[i]);
            }
        }
        if y.is_empty() {
            return Err(RlmError::Column("no complete-case rows".to_string()).into());
        }
        if w.is_empty() {
            w = vec![1.0; y.len()];
        }

        let x_slices: Vec<&[f64]> = x_filtered.iter().map(|v| v.as_slice()).collect();
        let result = rlm(&x_slices, &y, &w, &self.opts)
            .map_err(|e| RlmError::Fit(e.to_string()))?;

        // Wide single-row schema: one quartet per term (intercept first).
        let mut terms: Vec<String> = Vec::with_capacity(result.n_params);
        terms.push("intercept".to_string());
        terms.extend(self.predictors.iter().cloned());

        let mut fields: Vec<Field> = Vec::with_capacity(result.n_params * 4 + 4);
        let mut cols: Vec<Arc<dyn arrow_array::Array>> = Vec::with_capacity(result.n_params * 4 + 4);
        for (j, term) in terms.iter().enumerate() {
            fields.push(Field::new(format!("coef_{term}"), DataType::Float64, false));
            cols.push(Arc::new(Float64Array::from(vec![result.coefficients[j]])));
            fields.push(Field::new(format!("se_{term}"), DataType::Float64, false));
            cols.push(Arc::new(Float64Array::from(vec![result.std_errors[j]])));
            fields.push(Field::new(format!("t_{term}"), DataType::Float64, false));
            cols.push(Arc::new(Float64Array::from(vec![result.t_stats[j]])));
            fields.push(Field::new(format!("p_{term}"), DataType::Float64, true));
            cols.push(Arc::new(Float64Array::from(vec![result.p_values[j]])));
        }
        fields.push(Field::new("sigma_robust", DataType::Float64, false));
        cols.push(Arc::new(Float64Array::from(vec![result.scale])));
        fields.push(Field::new("converged", DataType::Int32, false));
        cols.push(Arc::new(Int32Array::from(vec![i32::from(result.converged)])));
        fields.push(Field::new("n_iter", DataType::Int32, false));
        cols.push(Arc::new(Int32Array::from(vec![result.n_iter as i32])));
        fields.push(Field::new("n_obs", DataType::Int32, false));
        cols.push(Arc::new(Int32Array::from(vec![result.n_obs as i32])));

        let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), cols)
            .expect("rlm schema");

        let ctx = node_ctx.session();
        let df = ctx
            .read_batch(batch)
            .map_err(|e| RlmError::ReadBatch(e.to_string()))?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
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

    fn make_batch(columns: Vec<(&str, Vec<f64>)>) -> RecordBatch {
        let fields: Vec<Field> = columns
            .iter()
            .map(|(name, _)| Field::new(*name, DataType::Float64, false))
            .collect();
        let arrays: Vec<Arc<dyn arrow_array::Array>> = columns
            .iter()
            .map(|(_, vals)| {
                Arc::new(Float64Array::from(vals.clone())) as Arc<dyn arrow_array::Array>
            })
            .collect();
        RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).unwrap()
    }

    fn cell(rows: &[RecordBatch], name: &str) -> f64 {
        let batch = &rows[0];
        let idx = batch.schema().index_of(name).unwrap();
        let col = batch.column(idx);
        if let Some(a) = col.as_any().downcast_ref::<Float64Array>() {
            a.value(0)
        } else {
            col.as_any()
                .downcast_ref::<Int32Array>()
                .unwrap()
                .value(0) as f64
        }
    }

    async fn run_node(spec: serde_json::Value, batch: RecordBatch) -> Vec<RecordBatch> {
        let mut node = RlmNodeFactory {}.build(spec, node_ctx()).unwrap();
        let input = dag_core::node::NodeInput::new_dataframe(
            0,
            datafusion::prelude::SessionContext::new()
                .read_batch(batch)
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
        outs.dataframe(0).unwrap().clone().collect().await.unwrap()
    }

    #[tokio::test]
    async fn outlier_robust_smoke() {
        // y = 3 + 2·x with one gross outlier; rlm must stay on the line.
        let n = 40;
        let x: Vec<f64> = (0..n).map(|i| (i % 5) as f64).collect();
        let mut y: Vec<f64> = x.iter().map(|&v| 3.0 + 2.0 * v).collect();
        y[7] += 1000.0;
        let batch = make_batch(vec![("x", x), ("y", y)]);

        let rows = run_node(
            serde_json::json!({
                "outcome_column": "y",
                "predictors": ["x"],
                "psi": "tukey",
            }),
            batch,
        )
        .await;
        assert_eq!(rows.iter().map(|b| b.num_rows()).sum::<usize>(), 1);
        assert!((cell(&rows, "coef_intercept") - 3.0).abs() < 0.05);
        assert!((cell(&rows, "coef_x") - 2.0).abs() < 0.05);
        assert_eq!(cell(&rows, "converged") as i32, 1);
        assert!(cell(&rows, "se_x") > 0.0);
        assert!(cell(&rows, "sigma_robust") < 1.0);
        assert_eq!(cell(&rows, "n_obs") as usize, n);
    }

    #[tokio::test]
    async fn bad_psi_is_rejected_at_build() {
        let err = RlmNodeFactory {}.build(
            serde_json::json!({
                "outcome_column": "y",
                "predictors": ["x"],
                "psi": "cauchy",
            }),
            node_ctx(),
        );
        assert!(err.is_err());
    }
}
