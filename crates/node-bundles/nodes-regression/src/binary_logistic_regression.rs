//! Binary logistic regression transform node.
//!
//! Wraps [`statkit::regression::logistic`] — IRLS (Newton-Raphson) binary
//! logistic regression with Wald z-tests, odds ratios, and 95% CIs.
//!
//! Output schema (one row per coefficient):
//!
//! | Column         | Type    | Description                              |
//! |----------------|---------|------------------------------------------|
//! | `term`         | Utf8    | Predictor name or "intercept"            |
//! | `coefficient`  | Float64 | β̂                                       |
//! | `std_error`    | Float64 | SE(β̂)                                   |
//! | `z_stat`       | Float64 | β̂ / SE                                  |
//! | `p_value`      | Float64 | Two-sided Wald p-value                   |
//! | `odds_ratio`   | Float64 | exp(β̂)                                  |
//! | `or_ci_lower`  | Float64 | 95% CI lower bound for OR                 |
//! | `or_ci_upper`  | Float64 | 95% CI upper bound for OR                 |
//! | `log_likelihood` | Float64 | Fitted model log-likelihood            |
//! | `n_obs`        | Int32   | Number of observations used               |
//! | `converged`    | Boolean | Whether IRLS converged                    |

use std::sync::Arc;

use arrow_array::{BooleanArray, Float64Array, Int32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use thiserror::Error;

use dag_core::arrow_util::ColumnError;
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
};

// ── Error ──────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum BinaryLogisticRegressionError {
    #[error("{0}")]
    Column(String),
    #[error("no predictor columns specified")]
    NoPredictors,
    #[error("outcome column and predictor column '{0}' are the same")]
    OutcomeIsPredictor(String),
    #[error("logistic fit failed: {0}")]
    Fit(String),
    #[error("collect failed: {0}")]
    Collect(String),
    #[error("read_batch failed: {0}")]
    ReadBatch(String),
}

impl From<ColumnError> for BinaryLogisticRegressionError {
    fn from(e: ColumnError) -> Self {
        Self::Column(e.to_string())
    }
}

impl ::dag_core::dag::NodeError for BinaryLogisticRegressionError {
    fn node_type(&self) -> &str {
        "binary_logistic_regression"
    }
}

// ── Spec ───────────────────────────────────────────────────────────────────

/// Spec for [`BinaryLogisticRegressionNode`].
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct BinaryLogisticRegressionNodeSpec {
    /// Names of the predictor columns (numeric). At least one is required.
    pub predictors: Vec<String>,
    /// Name of the binary outcome column (values must be 0/1 or true/false).
    pub outcome: String,
    /// Whether to fit an intercept term. Default: `true`.
    #[serde(default = "default_true")]
    pub intercept: bool,
}

fn default_true() -> bool {
    true
}

// ── Node ───────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct BinaryLogisticRegressionNode {
    meta: NodePorts,
    predictors: Vec<String>,
    outcome: String,
    intercept: bool,
}

impl BinaryLogisticRegressionNode {
    pub fn new(predictors: Vec<String>, outcome: String, intercept: bool) -> Self {
        Self {
            meta: port_layout(),
            predictors,
            outcome,
            intercept,
        }
    }
}

pub struct BinaryLogisticRegressionNodeFactory {}

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None).add_input_port(None)
}

impl NodeFactory for BinaryLogisticRegressionNodeFactory {
    fn kind(&self) -> &'static str {
        "binary_logistic_regression"
    }

    fn desc(&self) -> &'static str {
        "Binary logistic regression (IRLS). Outputs OR, 95% CI, and Wald p-values."
    }

    fn doc(&self) -> &'static str {
        "Fits a binary logistic regression via iteratively reweighted least \
        squares (IRLS / Newton-Raphson). The outcome column must be binary \
        (0/1 or true/false). Predictor columns must be numeric. Outputs a \
        summary table with coefficients, standard errors, z-statistics, \
        p-values, odds ratios with 95% CIs, log-likelihood, n_obs, and \
        convergence status."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(BinaryLogisticRegressionNodeSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: BinaryLogisticRegressionNodeSpec = serde_json::from_value(spec)?;
        if s.predictors.is_empty() {
            return Err(dag_core::registry::error::Error::SpecRejection {
                kind: "binary_logistic_regression".to_string(),
                reason: "predictors must be a non-empty array".to_string(),
                schema_pretty: serde_json::to_string_pretty(&schema_for!(
                    BinaryLogisticRegressionNodeSpec
                ))
                .unwrap_or_default(),
            });
        }
        if s.predictors.contains(&s.outcome) {
            return Err(dag_core::registry::error::Error::SpecRejection {
                kind: "binary_logistic_regression".to_string(),
                reason: format!("outcome '{}' must not also appear in predictors", s.outcome),
                schema_pretty: serde_json::to_string_pretty(&schema_for!(
                    BinaryLogisticRegressionNodeSpec
                ))
                .unwrap_or_default(),
            });
        }
        Ok(Box::new(BinaryLogisticRegressionNode::new(
            s.predictors,
            s.outcome,
            s.intercept,
        )))
    }
}

// ── DagNode impl ───────────────────────────────────────────────────────────

#[async_trait]
impl DagNode for BinaryLogisticRegressionNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "binary_logistic_regression"
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
        let input = inputs.first().ok_or(BinaryLogisticRegressionError::Column(
            "no input connected".to_string(),
        ))?;
        let batches = input
            .dataframe()?
            .clone()
            .collect()
            .await
            .map_err(|e| BinaryLogisticRegressionError::Collect(e.to_string()))?;

        // --- Extract all columns leniently (NaN for nulls) ---
        let y_raw = dag_core::arrow_util::extract_numeric_lenient(&batches, &self.outcome)
            .map_err(BinaryLogisticRegressionError::from)?;

        // Validate binary outcome.
        for &v in &y_raw {
            if !v.is_nan() && v != 0.0 && v != 1.0 {
                return Err(BinaryLogisticRegressionError::Column(format!(
                    "outcome '{}' must be binary (0/1), found value {v}",
                    self.outcome
                ))
                .into());
            }
        }

        let mut x_raw: Vec<Vec<f64>> = Vec::with_capacity(self.predictors.len());
        for name in &self.predictors {
            x_raw.push(
                dag_core::arrow_util::extract_numeric_lenient(&batches, name)
                    .map_err(BinaryLogisticRegressionError::from)?,
            );
        }

        // --- Filter to complete cases (no NaN in any column) ---
        let n = y_raw.len();
        let mut y = Vec::with_capacity(n);
        let mut x_filtered: Vec<Vec<f64>> = vec![Vec::with_capacity(n); self.predictors.len()];
        for i in 0..n {
            if y_raw[i].is_nan() || x_raw.iter().any(|x| x[i].is_nan()) {
                continue;
            }
            y.push(y_raw[i]);
            for (j, x) in x_raw.iter().enumerate() {
                x_filtered[j].push(x[i]);
            }
        }

        if y.is_empty() {
            return Err(BinaryLogisticRegressionError::Column(
                "no complete-case rows after removing nulls".to_string(),
            )
            .into());
        }

        // --- Fit logistic regression ---
        let x_slices: Vec<&[f64]> = x_filtered.iter().map(|v| v.as_slice()).collect();
        let fit = statkit::regression::logistic(&x_slices, &y, self.intercept)
            .map_err(|e| BinaryLogisticRegressionError::Fit(e.to_string()))?;

        // --- Build output batch ---
        let batch = build_logistic_batch(&fit, &self.predictors, self.intercept);
        let ctx = node_ctx.session();
        let df = ctx
            .read_batch(batch)
            .map_err(|e| BinaryLogisticRegressionError::ReadBatch(e.to_string()))?;

        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

// ── Output construction ────────────────────────────────────────────────────

fn build_logistic_batch(
    fit: &statkit::regression::LogisticResult,
    predictors: &[String],
    intercept: bool,
) -> RecordBatch {
    let n = fit.n_params;
    let mut terms = Vec::with_capacity(n);
    let mut coefficients = Vec::with_capacity(n);
    let mut std_errors = Vec::with_capacity(n);
    let mut z_stats = Vec::with_capacity(n);
    let mut p_values = Vec::with_capacity(n);
    let mut ors = Vec::with_capacity(n);
    let mut or_lo = Vec::with_capacity(n);
    let mut or_hi = Vec::with_capacity(n);
    let mut ll_col = Vec::with_capacity(n);
    let mut n_obs_col = Vec::with_capacity(n);
    let mut converged_col = Vec::with_capacity(n);

    for i in 0..n {
        let term = if intercept && i == 0 {
            "intercept".to_string()
        } else {
            let pred_idx = if intercept { i - 1 } else { i };
            predictors
                .get(pred_idx)
                .cloned()
                .unwrap_or_else(|| format!("x{pred_idx}"))
        };
        terms.push(term);
        coefficients.push(fit.coefficients[i]);
        std_errors.push(fit.std_errors[i]);
        z_stats.push(fit.z_stats[i]);
        p_values.push(fit.p_values[i]);
        ors.push(fit.odds_ratios[i]);
        or_lo.push(fit.or_ci_lower[i]);
        or_hi.push(fit.or_ci_upper[i]);
        ll_col.push(fit.log_likelihood);
        n_obs_col.push(fit.n_obs as i32);
        converged_col.push(fit.converged);
    }

    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("term", DataType::Utf8, false),
            Field::new("coefficient", DataType::Float64, false),
            Field::new("std_error", DataType::Float64, false),
            Field::new("z_stat", DataType::Float64, false),
            Field::new("p_value", DataType::Float64, false),
            Field::new("odds_ratio", DataType::Float64, false),
            Field::new("or_ci_lower", DataType::Float64, false),
            Field::new("or_ci_upper", DataType::Float64, false),
            Field::new("log_likelihood", DataType::Float64, false),
            Field::new("n_obs", DataType::Int32, false),
            Field::new("converged", DataType::Boolean, false),
        ])),
        vec![
            Arc::new(StringArray::from(terms)),
            Arc::new(Float64Array::from(coefficients)),
            Arc::new(Float64Array::from(std_errors)),
            Arc::new(Float64Array::from(z_stats)),
            Arc::new(Float64Array::from(p_values)),
            Arc::new(Float64Array::from(ors)),
            Arc::new(Float64Array::from(or_lo)),
            Arc::new(Float64Array::from(or_hi)),
            Arc::new(Float64Array::from(ll_col)),
            Arc::new(Int32Array::from(n_obs_col)),
            Arc::new(BooleanArray::from(converged_col)),
        ],
    )
    .expect("schema mismatch in build_logistic_batch")
}
