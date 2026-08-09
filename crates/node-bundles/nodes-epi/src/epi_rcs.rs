//! Restricted cubic spline (RCS) logistic regression node.
//!
//! Wraps [`epi::rcs::rcs_logistic`]. Fits a dose-response curve between a
//! continuous exposure and a binary outcome, testing for nonlinearity.
//!
//! Output schema (single row with test results):
//!
//! | Column           | Type    | Description                            |
//! |------------------|---------|----------------------------------------|
//! | `lr_stat`        | Float64 | LR statistic for nonlinearity           |
//! | `df_nonlinear`   | Int32   | d.o.f. for the nonlinearity test        |
//! | `p_nonlinear`    | Float64 | p-value for nonlinear component         |
//! | `p_overall`      | Float64 | p-value for overall association         |
//! | `n_knots`        | Int32   | Number of knots used                    |
//! | `n_obs`          | Int32   | Number of observations                   |
//!
//! A second output port carries the spline curve (log-odds, OR, and 95% CI
//! bands at each grid point) for plotting.
//! Columns: `x` (Float64), `log_odds` (Float64), `or` (Float64),
//! `or_lower` (Float64), `or_upper` (Float64).

use std::sync::Arc;

use arrow_array::{Float64Array, Int32Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use thiserror::Error;

use dag_core::arrow_util::{ColumnError, extract_numeric_lenient};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
};

#[derive(Debug, Error)]
pub enum EpiRcsError {
    #[error("{0}")]
    Column(String),
    #[error("{0}")]
    Fit(String),
    #[error("collect failed: {0}")]
    Collect(String),
    #[error("read_batch failed: {0}")]
    ReadBatch(String),
}

impl From<ColumnError> for EpiRcsError {
    fn from(e: ColumnError) -> Self {
        Self::Column(e.to_string())
    }
}

impl ::dag_core::dag::NodeError for EpiRcsError {
    fn node_type(&self) -> &str {
        "epi_rcs"
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct EpiRcsNodeSpec {
    /// Continuous exposure column name.
    pub x_column: String,
    /// Binary outcome column name (0/1).
    pub outcome_column: String,
    /// Optional covariate column names (adjusted for in the logistic model).
    #[serde(default)]
    pub covariates: Vec<String>,
    /// Number of RCS knots (3–7, default 4).
    #[serde(default = "default_n_knots")]
    pub n_knots: usize,
    /// Grid resolution for the curve output (default 100).
    #[serde(default = "default_n_points")]
    pub n_grid_points: usize,
}

fn default_n_knots() -> usize {
    4
}
fn default_n_points() -> usize {
    100
}

#[derive(Clone)]
pub struct EpiRcsNode {
    meta: NodePorts,
    x_column: String,
    outcome_column: String,
    covariates: Vec<String>,
    n_knots: usize,
    n_grid_points: usize,
}

pub struct EpiRcsNodeFactory {}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_output_port(None)
        .add_output_port(None)
        .add_input_port(None)
}

impl NodeFactory for EpiRcsNodeFactory {
    fn kind(&self) -> &'static str {
        "epi_rcs"
    }
    fn desc(&self) -> &'static str {
        "Restricted cubic spline logistic regression with nonlinearity LR test."
    }
    fn doc(&self) -> &'static str {
        "Fits a restricted cubic spline (RCS) logistic regression of a binary \
        outcome on a continuous exposure. Tests for nonlinearity via a \
        likelihood-ratio test comparing the spline model vs a linear-only \
        model. Outputs test statistics (port 0) and the fitted curve for \
        plotting (port 1)."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(EpiRcsNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: EpiRcsNodeSpec = serde_json::from_value(spec)?;
        if !(3..=7).contains(&s.n_knots) {
            return Err(dag_core::registry::error::Error::SpecRejection {
                kind: "epi_rcs".to_string(),
                reason: format!("n_knots must be 3–7, got {}", s.n_knots),
                schema_pretty: serde_json::to_string_pretty(&schema_for!(EpiRcsNodeSpec))
                    .unwrap_or_default(),
            });
        }
        Ok(Box::new(EpiRcsNode {
            meta: port_layout(),
            x_column: s.x_column,
            outcome_column: s.outcome_column,
            covariates: s.covariates,
            n_knots: s.n_knots,
            n_grid_points: s.n_grid_points,
        }))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<EpiRcsNodeSpec>(spec, "epi_rcs")?;
        let out = ctx.output_var.to_string();
        let fit = ctx.fresh_var("rcs_fit");
        let input = input_0(ctx).to_string();
        let out2 = ctx.fresh_var("rcs_curve");
        let covars = if s.covariates.is_empty() {
            String::new()
        } else {
            format!(" + {}", s.covariates.join(" + "))
        };
        let pred = ctx.fresh_var("rcs_pred");
        let code = vec![
            format!("# Restricted cubic splines ({} knots)", s.n_knots),
            format!("ddist <- datadist({input})"),
            format!("options(datadist = 'ddist')"),
            format!(
                "{fit} <- lrm({} ~ rcs({}, {}){}, data = {input}, x=TRUE)",
                s.outcome_column, s.x_column, s.n_knots, covars
            ),
            // Port 0: summary statistics
            format!("{out} <- data.frame("),
            format!("  lr_stat = as.numeric({fit}$stats[\"Model L.R.\"]),"),
            format!(
                "  p_overall = 1 - pchisq(as.numeric({fit}$stats[\"Model L.R.\"]), as.numeric({fit}$stats[\"d.f.\"])),"
            ),
            format!("  n_knots = {},", s.n_knots),
            format!("  n_obs = {fit}$stats[\"Obs\"]"),
            format!(")"),
            // Port 1: OR curve points
            format!(
                "{pred} <- Predict({fit}, {} = seq(min({input}${}), max({input}${}), length.out = {}))",
                s.x_column, s.x_column, s.x_column, s.n_grid_points
            ),
            format!("{out2} <- data.frame("),
            format!("  x = {pred}${},", s.x_column),
            format!("  log_odds = {pred}$yhat,"),
            format!("  or = exp({pred}$yhat)"),
            format!(")"),
            format!("print({out})"),
            format!("options(datadist = NULL)"),
        ];
        Ok(dag_core::codegen::NodeCodegen {
            code,
            output_vars: vec![out, out2],
            extra_packages: vec![],
        })
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["rms".into()]
    }
}

#[async_trait]
impl DagNode for EpiRcsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        "epi_rcs"
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
            .ok_or(EpiRcsError::Column("no input connected".to_string()))?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| EpiRcsError::Collect(e.to_string()))?;

        let x_raw = extract_numeric_lenient(&batches, &self.x_column)?;
        let y_raw = extract_numeric_lenient(&batches, &self.outcome_column)?;
        // Validate binary outcome.
        for &v in &y_raw {
            if !v.is_nan() && v != 0.0 && v != 1.0 {
                return Err(EpiRcsError::Column(format!(
                    "outcome '{}' must be binary (0/1), found {v}",
                    self.outcome_column
                ))
                .into());
            }
        }

        // Extract covariates.
        let cov_raw: Vec<Vec<f64>> = self
            .covariates
            .iter()
            .map(|c| extract_numeric_lenient(&batches, c))
            .collect::<Result<_, _>>()?;

        // Complete-case filter.
        let n = y_raw.len();
        let mut x = Vec::with_capacity(n);
        let mut y_u64 = Vec::with_capacity(n);
        let mut cov_filtered: Vec<Vec<f64>> = vec![Vec::with_capacity(n); self.covariates.len()];
        for i in 0..n {
            if y_raw[i].is_nan() || x_raw[i].is_nan() {
                continue;
            }
            if cov_raw.iter().any(|c| c[i].is_nan()) {
                continue;
            }
            x.push(x_raw[i]);
            y_u64.push(y_raw[i] as u64);
            for (j, c) in cov_raw.iter().enumerate() {
                cov_filtered[j].push(c[i]);
            }
        }

        if x.is_empty() {
            return Err(EpiRcsError::Column("no complete-case rows".to_string()).into());
        }

        let cov_slices: Vec<&[f64]> = cov_filtered.iter().map(|v| v.as_slice()).collect();
        let result = epi::rcs::rcs_logistic(&x, &y_u64, self.n_knots, &cov_slices)
            .map_err(|e| EpiRcsError::Fit(e.to_string()))?;

        // --- Shared derived values for both output ports ---
        let x_min = x.iter().copied().fold(f64::INFINITY, f64::min);
        let x_max = x.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let cov_means: Vec<f64> = cov_filtered
            .iter()
            .map(|c| c.iter().sum::<f64>() / c.len() as f64)
            .collect();

        // Peak risk threshold identification (brute-force grid search).
        let peak_x = epi::rcs::find_peak_risk(
            &result.spline_fit,
            &result.knots,
            &cov_means,
            x_min,
            x_max,
            1000,
        );

        // --- Output port 0: test statistics ---
        let stats_batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("lr_stat", DataType::Float64, false),
                Field::new("df_nonlinear", DataType::Int32, false),
                Field::new("p_nonlinear", DataType::Float64, false),
                Field::new("p_overall", DataType::Float64, false),
                Field::new("peak_x", DataType::Float64, false),
                Field::new("n_knots", DataType::Int32, false),
                Field::new("n_obs", DataType::Int32, false),
            ])),
            vec![
                Arc::new(Float64Array::from(vec![result.lr_stat])),
                Arc::new(Int32Array::from(vec![result.df_nonlinear as i32])),
                Arc::new(Float64Array::from(vec![result.p_nonlinear])),
                Arc::new(Float64Array::from(vec![result.p_overall])),
                Arc::new(Float64Array::from(vec![peak_x])),
                Arc::new(Int32Array::from(vec![result.knots.len() as i32])),
                Arc::new(Int32Array::from(vec![x.len() as i32])),
            ],
        )
        .expect("rcs stats schema");

        // --- Output port 1: fitted curve ---
        let step = if self.n_grid_points > 0 {
            (x_max - x_min) / self.n_grid_points as f64
        } else {
            0.0
        };

        /// 95% CI z-quantile.
        const Z_975: f64 = 1.959963984540054;

        let curve_data: Vec<(f64, f64, f64, f64, f64)> = (0..=self.n_grid_points)
            .map(|i| {
                let xv = x_min + i as f64 * step;
                let (eta, se) = epi::rcs::predict_log_odds_with_se(
                    &result.spline_fit,
                    &result.knots,
                    xv,
                    &cov_means,
                );
                let or = eta.exp();
                let or_lower = (eta - Z_975 * se).exp();
                let or_upper = (eta + Z_975 * se).exp();
                (xv, eta, or, or_lower, or_upper)
            })
            .collect();

        let curve_x: Vec<f64> = curve_data.iter().map(|&(x, _, _, _, _)| x).collect();
        let curve_log_odds: Vec<f64> = curve_data.iter().map(|&(_, lo, _, _, _)| lo).collect();
        let curve_or: Vec<f64> = curve_data.iter().map(|&(_, _, or, _, _)| or).collect();
        let curve_or_lo: Vec<f64> = curve_data.iter().map(|&(_, _, _, lo, _)| lo).collect();
        let curve_or_hi: Vec<f64> = curve_data.iter().map(|&(_, _, _, _, hi)| hi).collect();

        let curve_batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("x", DataType::Float64, false),
                Field::new("log_odds", DataType::Float64, false),
                Field::new("or", DataType::Float64, false),
                Field::new("or_lower", DataType::Float64, false),
                Field::new("or_upper", DataType::Float64, false),
            ])),
            vec![
                Arc::new(Float64Array::from(curve_x)),
                Arc::new(Float64Array::from(curve_log_odds)),
                Arc::new(Float64Array::from(curve_or)),
                Arc::new(Float64Array::from(curve_or_lo)),
                Arc::new(Float64Array::from(curve_or_hi)),
            ],
        )
        .expect("rcs curve schema");

        let ctx = node_ctx.session();
        let df0 = ctx
            .read_batch(stats_batch)
            .map_err(|e| EpiRcsError::ReadBatch(e.to_string()))?;
        let df1 = ctx
            .read_batch(curve_batch)
            .map_err(|e| EpiRcsError::ReadBatch(e.to_string()))?;

        let mut res = PortOutputs::new();
        res.insert(0, df0);
        res.insert(1, df1);
        Ok(res)
    }
}
