//! LASSO logistic regression node with k-fold cross-validation.
//!
//! Wraps [`epi::lasso`]. Performs feature selection via LASSO with a λ path
//! and CV, plus optional bootstrap selection frequencies.
//!
//! **Port 0** — Feature-level results (one row per feature):
//!
//! | Column              | Type    | Description                          |
//! |---------------------|---------|--------------------------------------|
//! | `feature`           | Utf8    | Feature name                          |
//! | `selected_min`      | Boolean | Selected at λ.min                     |
//! | `selected_1se`      | Boolean | Selected at λ.1se                     |
//! | `coef_min`          | Float64 | Coefficient at λ.min                  |
//! | `coef_1se`          | Float64 | Coefficient at λ.1se                  |
//! | `lambda_min`        | Float64 | Optimal λ (minimum CV deviance)       |
//! | `lambda_1se`        | Float64 | λ.1se (most regularised within 1 SE)  |
//! | `bootstrap_freq`    | Float64 | Selection frequency (if bootstrap > 0)|
//!
//! **Port 1** — CV curve (one row per λ on the path):
//!
//! | Column              | Type    | Description                          |
//! |---------------------|---------|--------------------------------------|
//! | `lambda`            | Float64 | Penalty λ                            |
//! | `cv_mean`           | Float64 | Mean CV deviance across folds        |
//! | `cv_se`             | Float64 | Standard error of CV deviance        |
//! | `n_selected`        | Int32   | Non-zero coefficients at this λ       |
//! | `is_lambda_min`     | Boolean | True at λ.min                        |
//! | `is_lambda_1se`     | Boolean | True at λ.1se                        |

use std::sync::Arc;

use arrow_array::{BooleanArray, Float64Array, Int32Array, RecordBatch, StringArray};
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
pub enum EpiLassoError {
    #[error("{0}")]
    Column(String),
    #[error("{0}")]
    Fit(String),
    #[error("collect failed: {0}")]
    Collect(String),
    #[error("read_batch failed: {0}")]
    ReadBatch(String),
}

impl From<ColumnError> for EpiLassoError {
    fn from(e: ColumnError) -> Self {
        Self::Column(e.to_string())
    }
}

impl ::dag_core::dag::NodeError for EpiLassoError {
    fn node_type(&self) -> &str {
        "epi_lasso"
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct EpiLassoNodeSpec {
    /// Predictor column names.
    pub predictors: Vec<String>,
    /// Binary outcome column name (0/1).
    pub outcome_column: String,
    /// Number of λ values on the path (default 100).
    #[serde(default = "default_n_lambda")]
    pub n_lambda: usize,
    /// Number of CV folds (default 10).
    #[serde(default = "default_cv_folds")]
    pub cv_folds: usize,
    /// Bootstrap iterations for selection frequency (0 = skip, default 1000).
    #[serde(default = "default_n_boot")]
    pub n_bootstrap: usize,
    /// Random seed (default 42).
    #[serde(default = "default_seed")]
    pub seed: u64,
}

fn default_n_lambda() -> usize {
    100
}
fn default_cv_folds() -> usize {
    10
}
fn default_n_boot() -> usize {
    1000
}
fn default_seed() -> u64 {
    42
}

#[derive(Clone)]
pub struct EpiLassoNode {
    meta: NodePorts,
    predictors: Vec<String>,
    outcome_column: String,
    n_lambda: usize,
    cv_folds: usize,
    n_bootstrap: usize,
    seed: u64,
}

pub struct EpiLassoNodeFactory {}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_output_port(None) // port 0: feature-level results
        .add_output_port(None) // port 1: CV curve
        .add_input_port(None)
}

impl NodeFactory for EpiLassoNodeFactory {
    fn kind(&self) -> &'static str {
        "epi_lasso"
    }
    fn desc(&self) -> &'static str {
        "LASSO logistic regression with k-fold CV and bootstrap selection frequencies."
    }
    fn doc(&self) -> &'static str {
        "Performs LASSO penalised logistic regression for feature selection. \
        Builds a λ path via coordinate descent, selects optimal λ via k-fold \
        cross-validation, and optionally computes bootstrap selection \
        frequencies. Outputs λ.min and λ.1se results, plus selection status \
        and coefficients for each predictor."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(EpiLassoNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: EpiLassoNodeSpec = serde_json::from_value(spec)?;
        if s.predictors.is_empty() {
            return Err(dag_core::registry::error::Error::SpecRejection {
                kind: "epi_lasso".to_string(),
                reason: "predictors must be a non-empty array".to_string(),
                schema_pretty: serde_json::to_string_pretty(&schema_for!(EpiLassoNodeSpec))
                    .unwrap_or_default(),
            });
        }
        Ok(Box::new(EpiLassoNode {
            meta: port_layout(),
            predictors: s.predictors.clone(),
            outcome_column: s.outcome_column,
            n_lambda: s.n_lambda,
            cv_folds: s.cv_folds,
            n_bootstrap: s.n_bootstrap,
            seed: s.seed,
        }))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<EpiLassoNodeSpec>(spec, "epi_lasso")?;
        let out = ctx.output_var.to_string();
        let cv_out = ctx.fresh_var("cv_curve");
        let cv_fit = ctx.fresh_var("cv_fit");
        let x_mat = ctx.fresh_var("x_mat");
        let y_vec = ctx.fresh_var("y_vec");
        let input = input_0(ctx).to_string();
        let x_cols = s
            .predictors
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let code = vec![
            format!(
                "# LASSO regression ({}-fold CV, {} lambdas)",
                s.cv_folds, s.n_lambda
            ),
            format!("{x_mat} <- as.matrix({input}[, c({x_cols})])"),
            format!("{y_vec} <- {input}${}", s.outcome_column),
            format!("set.seed({})", s.seed),
            format!(
                "{cv_fit} <- cv.glmnet({x_mat}, {y_vec}, alpha = 1, nfolds = {}, nlambda = {})",
                s.cv_folds, s.n_lambda
            ),
            format!("{out} <- data.frame("),
            format!("  feature = colnames({x_mat}),"),
            format!("  coef_min = as.numeric(coef({cv_fit}, s = \"lambda.min\")[-1]),"),
            format!("  coef_1se = as.numeric(coef({cv_fit}, s = \"lambda.1se\")[-1]),"),
            format!("  lambda_min = {cv_fit}$lambda.min,"),
            format!("  lambda_1se = {cv_fit}$lambda.1se"),
            format!(")"),
            format!("# NOTE: bootstrap_freq not generated in R reference"),
            format!("print({out})"),
            format!("# CV curve (port 1)"),
            format!("{cv_out} <- data.frame("),
            format!("  lambda = {cv_fit}$lambda,"),
            format!("  cv_mean = {cv_fit}$cvm,"),
            format!("  cv_se = {cv_fit}$cvsd,"),
            format!("  n_selected = {cv_fit}$nzero,"),
            format!("  is_lambda_min = {cv_fit}$lambda == {cv_fit}$lambda.min,"),
            format!("  is_lambda_1se = {cv_fit}$lambda == {cv_fit}$lambda.1se"),
            format!(")"),
            format!("print({cv_out})"),
        ];
        Ok(dag_core::codegen::NodeCodegen {
            code,
            output_vars: vec![out, cv_out],
            extra_packages: vec![],
        })
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["glmnet".into()]
    }
}

#[async_trait]
impl DagNode for EpiLassoNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        "epi_lasso"
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
            .ok_or(EpiLassoError::Column("no input connected".to_string()))?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| EpiLassoError::Collect(e.to_string()))?;

        let y_raw = extract_numeric_lenient(&batches, &self.outcome_column)?;
        for &v in &y_raw {
            if !v.is_nan() && v != 0.0 && v != 1.0 {
                return Err(EpiLassoError::Column(format!(
                    "outcome '{}' must be binary (0/1), found {v}",
                    self.outcome_column
                ))
                .into());
            }
        }

        let x_raw: Vec<Vec<f64>> = self
            .predictors
            .iter()
            .map(|c| extract_numeric_lenient(&batches, c))
            .collect::<Result<_, _>>()?;

        // Complete-case filter.
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
            return Err(EpiLassoError::Column("no complete-case rows".to_string()).into());
        }

        // Run LASSO CV.
        let x_slices: Vec<&[f64]> = x_filtered.iter().map(|v| v.as_slice()).collect();
        let opts = epi::lasso::LassoOptions {
            n_lambda: self.n_lambda,
            cv_folds: self.cv_folds,
            seed: self.seed,
            ..Default::default()
        };
        let cv = epi::lasso::lasso_cv(&x_slices, &y, self.predictors.clone(), &opts)
            .map_err(|e| EpiLassoError::Fit(e.to_string()))?;

        // Optional bootstrap.
        let boot_freqs = if self.n_bootstrap > 0 {
            let (counts, freqs) = epi::lasso::lasso_bootstrap_selection(
                &x_slices,
                &y,
                cv.lambdas[cv.idx_1se],
                self.n_bootstrap,
                self.seed,
                &opts,
            )
            .map_err(|e| EpiLassoError::Fit(e.to_string()))?;
            let _ = counts;
            Some(freqs)
        } else {
            None
        };

        // Build output: one row per feature.
        let nf = self.predictors.len();
        let lam_min = cv.lambdas[cv.idx_min];
        let lam_1se = cv.lambdas[cv.idx_1se];

        let mut features = Vec::with_capacity(nf);
        let mut sel_min = Vec::with_capacity(nf);
        let mut sel_1se = Vec::with_capacity(nf);
        let mut coef_min = Vec::with_capacity(nf);
        let mut coef_1se = Vec::with_capacity(nf);
        let mut lam_min_col = Vec::with_capacity(nf);
        let mut lam_1se_col = Vec::with_capacity(nf);
        let mut boot_col = Vec::with_capacity(nf);

        for j in 0..nf {
            features.push(self.predictors[j].clone());
            // Index 0 = intercept, so feature j is at coefficient index j+1.
            let idx = j + 1;
            sel_min.push(cv.fit_min.selected.get(idx).copied().unwrap_or(false));
            sel_1se.push(cv.fit_1se.selected.get(idx).copied().unwrap_or(false));
            coef_min.push(cv.fit_min.coefficients.get(idx).copied().unwrap_or(0.0));
            coef_1se.push(cv.fit_1se.coefficients.get(idx).copied().unwrap_or(0.0));
            lam_min_col.push(lam_min);
            lam_1se_col.push(lam_1se);
            boot_col.push(boot_freqs.as_ref().map(|f| f[j]).unwrap_or(f64::NAN));
        }

        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("feature", DataType::Utf8, false),
                Field::new("selected_min", DataType::Boolean, false),
                Field::new("selected_1se", DataType::Boolean, false),
                Field::new("coef_min", DataType::Float64, false),
                Field::new("coef_1se", DataType::Float64, false),
                Field::new("lambda_min", DataType::Float64, false),
                Field::new("lambda_1se", DataType::Float64, false),
                Field::new("bootstrap_freq", DataType::Float64, true),
            ])),
            vec![
                Arc::new(StringArray::from(features)),
                Arc::new(BooleanArray::from(sel_min)),
                Arc::new(BooleanArray::from(sel_1se)),
                Arc::new(Float64Array::from(coef_min)),
                Arc::new(Float64Array::from(coef_1se)),
                Arc::new(Float64Array::from(lam_min_col)),
                Arc::new(Float64Array::from(lam_1se_col)),
                Arc::new(Float64Array::from(boot_col)),
            ],
        )
        .expect("lasso output schema");

        // ── Port 1: CV curve (one row per λ) ──
        let nl = cv.lambdas.len();
        let cv_lambdas: Float64Array = cv.lambdas.iter().copied().collect();
        let cv_means: Float64Array = cv.cv_mean.iter().copied().collect();
        let cv_ses: Float64Array = cv.cv_se.iter().copied().collect();
        let cv_nsel: Int32Array = cv.n_selected_path.iter().map(|&n| n as i32).collect();
        let is_min: BooleanArray = (0..nl).map(|i| i == cv.idx_min).collect();
        let is_1se: BooleanArray = (0..nl).map(|i| i == cv.idx_1se).collect();

        let cv_batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("lambda", DataType::Float64, false),
                Field::new("cv_mean", DataType::Float64, false),
                Field::new("cv_se", DataType::Float64, false),
                Field::new("n_selected", DataType::Int32, false),
                Field::new("is_lambda_min", DataType::Boolean, false),
                Field::new("is_lambda_1se", DataType::Boolean, false),
            ])),
            vec![
                Arc::new(cv_lambdas),
                Arc::new(cv_means),
                Arc::new(cv_ses),
                Arc::new(cv_nsel),
                Arc::new(is_min),
                Arc::new(is_1se),
            ],
        )
        .expect("lasso cv curve schema");

        let ctx = node_ctx.session();
        let df = ctx
            .read_batch(batch)
            .map_err(|e| EpiLassoError::ReadBatch(e.to_string()))?;
        let cv_df = ctx
            .read_batch(cv_batch)
            .map_err(|e| EpiLassoError::ReadBatch(e.to_string()))?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        res.insert(1, cv_df);
        Ok(res)
    }
}
