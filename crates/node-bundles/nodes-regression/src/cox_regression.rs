//! Cox proportional hazards regression node.
//!
//! Wraps [`statkit::regression::cox`]. Fits the semiparametric Cox model via
//! partial likelihood maximisation (Newton-Raphson with Breslow ties).
//!
//! Output schema (one row per coefficient):
//!
//! | Column         | Type    | Description                              |
//! |----------------|---------|------------------------------------------|
//! | `term`         | Utf8    | Predictor name                            |
//! | `coefficient`  | Float64 | β̂                                       |
//! | `std_error`    | Float64 | SE(β̂)                                   |
//! | `z_stat`       | Float64 | β̂ / SE                                  |
//! | `p_value`      | Float64 | Two-sided Wald p-value                   |
//! | `hazard_ratio` | Float64 | exp(β̂)                                  |
//! | `hr_ci_lower`  | Float64 | 95% CI lower bound for HR                 |
//! | `hr_ci_upper`  | Float64 | 95% CI upper bound for HR                 |
//! | `log_likelihood` | Float64 | Partial log-likelihood                |
//! | `concordance`  | Float64 | Harrell's C-index                         |
//! | `n_obs`        | Int32   | Total observations                        |
//! | `n_events`     | Int32   | Number of events (δ=1)                    |
//! | `converged`    | Boolean | Whether Newton-Raphson converged          |

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

#[derive(Debug, Error)]
pub enum CoxRegressionError {
    #[error("{0}")]
    Column(String),
    #[error("Cox fit failed: {0}")]
    Fit(String),
    #[error("collect failed: {0}")]
    Collect(String),
    #[error("read_batch failed: {0}")]
    ReadBatch(String),
}

impl From<ColumnError> for CoxRegressionError {
    fn from(e: ColumnError) -> Self {
        Self::Column(e.to_string())
    }
}

impl ::dag_core::dag::NodeError for CoxRegressionError {
    fn node_type(&self) -> &str {
        "cox_regression"
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CoxRegressionNodeSpec {
    /// Predictor column names (numeric). No intercept is fitted.
    pub predictors: Vec<String>,
    /// Survival time column name (event or censoring time).
    pub time_column: String,
    /// Event indicator column name (1 = event, 0 = censored).
    pub event_column: String,
}

#[derive(Clone)]
pub struct CoxRegressionNode {
    meta: NodePorts,
    predictors: Vec<String>,
    time_column: String,
    event_column: String,
}

pub struct CoxRegressionNodeFactory {}

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None).add_input_port(None)
}

impl NodeFactory for CoxRegressionNodeFactory {
    fn kind(&self) -> &'static str {
        "cox_regression"
    }
    fn desc(&self) -> &'static str {
        "Cox proportional hazards regression (partial likelihood, Breslow ties)."
    }
    fn doc(&self) -> &'static str {
        "Fits a Cox proportional hazards model via partial likelihood \
        maximisation (Newton-Raphson with step-halving). Ties are handled via \
        Breslow's approximation. The time column gives observed survival times \
        (event or censoring). The event column must be binary (1 = event, \
        0 = censored). Outputs hazard ratios with 95% CIs, Wald p-values, \
        Harrell's concordance index, and the partial log-likelihood."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(CoxRegressionNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: CoxRegressionNodeSpec = serde_json::from_value(spec)?;
        if s.predictors.is_empty() {
            return Err(dag_core::registry::error::Error::SpecRejection {
                kind: "cox_regression".to_string(),
                reason: "predictors must be a non-empty array".to_string(),
                schema_pretty: serde_json::to_string_pretty(&schema_for!(CoxRegressionNodeSpec))
                    .unwrap_or_default(),
            });
        }
        Ok(Box::new(CoxRegressionNode {
            meta: port_layout(),
            predictors: s.predictors,
            time_column: s.time_column,
            event_column: s.event_column,
        }))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<CoxRegressionNodeSpec>(spec, "cox_regression")?;
        let out = ctx.output_var.to_string();
        let fit = ctx.fresh_var("cox_fit");
        let smry = ctx.fresh_var("cox_smry");
        let input = input_0(ctx).to_string();
        let preds = s.predictors.join(" + ");
        let formula = format!("Surv({}, {}) ~ {}", s.time_column, s.event_column, preds);
        let code = vec![
            format!("# Cox proportional hazards regression"),
            format!("{fit} <- coxph({formula}, data = {input})"),
            format!("{smry} <- summary({fit})"),
            format!("{out} <- data.frame("),
            format!("  term = rownames({smry}$coefficients),"),
            format!("  coefficient = {smry}$coefficients[, \"coef\"],"),
            format!("  std_error = {smry}$coefficients[, \"se(coef)\"],"),
            format!("  z_stat = {smry}$coefficients[, \"z\"],"),
            format!("  p_value = {smry}$coefficients[, \"Pr(>|z|)\"]"),
            format!(")"),
            format!("{out}$hazard_ratio <- exp({out}$coefficient)"),
            format!("{out}$hr_ci_lower <- exp({smry}$conf.int[, \"lower .95\"])"),
            format!("{out}$hr_ci_upper <- exp({smry}$conf.int[, \"upper .95\"])"),
            format!("{out}$log_likelihood <- {fit}$loglik[2]"),
            format!("{out}$concordance <- {smry}$concordance[1]"),
            format!("{out}$n_obs <- {fit}$n"),
            format!("{out}$n_events <- {fit}$nevent"),
            format!("{out}$converged <- TRUE"),
            format!("print({out})"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["survival".into()]
    }
}

#[async_trait]
impl DagNode for CoxRegressionNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        "cox_regression"
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
            .ok_or(CoxRegressionError::Column("no input connected".to_string()))?;
        let batches = input
            .dataframe()?
            .clone()
            .collect()
            .await
            .map_err(|e| CoxRegressionError::Collect(e.to_string()))?;

        let time_raw = dag_core::arrow_util::extract_numeric_lenient(&batches, &self.time_column)?;
        let event_raw =
            dag_core::arrow_util::extract_numeric_lenient(&batches, &self.event_column)?;

        // Validate binary event indicator.
        for &v in &event_raw {
            if !v.is_nan() && v != 0.0 && v != 1.0 {
                return Err(CoxRegressionError::Column(format!(
                    "event '{}' must be binary (0=censored, 1=event), found {v}",
                    self.event_column
                ))
                .into());
            }
        }

        let mut x_raw: Vec<Vec<f64>> = Vec::with_capacity(self.predictors.len());
        for name in &self.predictors {
            x_raw.push(dag_core::arrow_util::extract_numeric_lenient(
                &batches, name,
            )?);
        }

        // Complete-case filter.
        let n = time_raw.len();
        let mut time = Vec::with_capacity(n);
        let mut event = Vec::with_capacity(n);
        let mut x_filtered: Vec<Vec<f64>> = vec![Vec::with_capacity(n); self.predictors.len()];
        for i in 0..n {
            if time_raw[i].is_nan() || event_raw[i].is_nan() || x_raw.iter().any(|x| x[i].is_nan())
            {
                continue;
            }
            time.push(time_raw[i]);
            event.push(event_raw[i]);
            for (j, x) in x_raw.iter().enumerate() {
                x_filtered[j].push(x[i]);
            }
        }

        if time.is_empty() {
            return Err(CoxRegressionError::Column(
                "no complete-case rows after removing nulls".to_string(),
            )
            .into());
        }

        let x_slices: Vec<&[f64]> = x_filtered.iter().map(|v| v.as_slice()).collect();
        let fit = statkit::regression::cox(&time, &event, &x_slices)
            .map_err(|e| CoxRegressionError::Fit(e.to_string()))?;

        // Build output batch.
        let np = fit.n_params;
        let n_obs_col = vec![fit.n_obs as i32; np];
        let n_events_col = vec![fit.n_events as i32; np];
        let ll_col = vec![fit.log_likelihood; np];
        let c_col = vec![fit.concordance; np];
        let converged_col = vec![fit.converged; np];

        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("term", DataType::Utf8, false),
                Field::new("coefficient", DataType::Float64, false),
                Field::new("std_error", DataType::Float64, false),
                Field::new("z_stat", DataType::Float64, false),
                Field::new("p_value", DataType::Float64, false),
                Field::new("hazard_ratio", DataType::Float64, false),
                Field::new("hr_ci_lower", DataType::Float64, false),
                Field::new("hr_ci_upper", DataType::Float64, false),
                Field::new("log_likelihood", DataType::Float64, false),
                Field::new("concordance", DataType::Float64, false),
                Field::new("n_obs", DataType::Int32, false),
                Field::new("n_events", DataType::Int32, false),
                Field::new("converged", DataType::Boolean, false),
            ])),
            vec![
                Arc::new(StringArray::from(self.predictors.clone())),
                Arc::new(Float64Array::from(fit.coefficients.clone())),
                Arc::new(Float64Array::from(fit.std_errors.clone())),
                Arc::new(Float64Array::from(fit.z_stats.clone())),
                Arc::new(Float64Array::from(fit.p_values.clone())),
                Arc::new(Float64Array::from(fit.hazard_ratios.clone())),
                Arc::new(Float64Array::from(fit.hr_ci_lower.clone())),
                Arc::new(Float64Array::from(fit.hr_ci_upper.clone())),
                Arc::new(Float64Array::from(ll_col)),
                Arc::new(Float64Array::from(c_col)),
                Arc::new(Int32Array::from(n_obs_col)),
                Arc::new(Int32Array::from(n_events_col)),
                Arc::new(BooleanArray::from(converged_col)),
            ],
        )
        .expect("cox output schema");

        let ctx = node_ctx.session();
        let df = ctx
            .read_batch(batch)
            .map_err(|e| CoxRegressionError::ReadBatch(e.to_string()))?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}
