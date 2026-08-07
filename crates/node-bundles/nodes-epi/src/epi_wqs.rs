//! Weighted Quantile Sum (WQS) regression node.
//!
//! Wraps [`epi::wqs`]. Constructs a weighted index from multiple exposures and
//! tests its association with a binary outcome.
//!
//! Output schema (one row per exposure):
//!
//! | Column                | Type    | Description                          |
//! |-----------------------|---------|--------------------------------------|
//! | `feature`             | Utf8    | Exposure name                         |
//! | `weight`              | Float64 | Estimated weight (Σ=1)                |
//! | `bootstrap_mean`      | Float64 | Mean bootstrap weight                 |
//! | `bootstrap_se`        | Float64 | SE of bootstrap weight                |
//!
//! Plus a summary row in a second port: `beta_wqs`, `beta_wqs_p`, `wqs_or`,
//! `wqs_or_lower`, `wqs_or_upper`, `test_p_value`, `n_train`, `n_test`.

use std::sync::Arc;

use arrow_array::{Float64Array, Int32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use thiserror::Error;

use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::arrow_util::{ColumnError, extract_numeric_lenient};
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
};

#[derive(Debug, Error)]
pub enum EpiWqsError {
    #[error("{0}")]
    Column(String),
    #[error("{0}")]
    Fit(String),
    #[error("collect failed: {0}")]
    Collect(String),
    #[error("read_batch failed: {0}")]
    ReadBatch(String),
}

impl From<ColumnError> for EpiWqsError {
    fn from(e: ColumnError) -> Self {
        Self::Column(e.to_string())
    }
}

impl ::dag_core::dag::NodeError for EpiWqsError {
    fn node_type(&self) -> &str { "epi_wqs" }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct EpiWqsNodeSpec {
    /// Exposure column names (the variables to weight).
    pub exposures: Vec<String>,
    /// Binary outcome column name (0/1).
    pub outcome_column: String,
    /// Optional covariate column names.
    #[serde(default)]
    pub covariates: Vec<String>,
    /// Number of quantile bins for exposure transformation (default 4).
    #[serde(default = "default_nq")]
    pub n_quantiles: usize,
    /// Training fraction (default 0.4).
    #[serde(default = "default_train_frac")]
    pub train_frac: f64,
    /// Bootstrap iterations (default 1000).
    #[serde(default = "default_n_boot")]
    pub n_bootstrap: usize,
    /// Random seed (default 42).
    #[serde(default = "default_seed")]
    pub seed: u64,
}

fn default_nq() -> usize {
    4
}
fn default_train_frac() -> f64 {
    0.4
}
fn default_n_boot() -> usize {
    1000
}
fn default_seed() -> u64 {
    42
}

#[derive(Clone)]
pub struct EpiWqsNode {
    meta: NodePorts,
    exposures: Vec<String>,
    outcome_column: String,
    covariates: Vec<String>,
    n_quantiles: usize,
    train_frac: f64,
    n_bootstrap: usize,
    seed: u64,
}

pub struct EpiWqsNodeFactory {}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_output_port(None) // weights
        .add_output_port(None) // summary
        .add_input_port(None)
}

impl NodeFactory for EpiWqsNodeFactory {
    fn kind(&self) -> &'static str {
        "epi_wqs"
    }
    fn desc(&self) -> &'static str {
        "Weighted Quantile Sum regression with bootstrap weight stability."
    }
    fn doc(&self) -> &'static str {
        "Constructs a Weighted Quantile Sum (WQS) index from multiple exposure \
        variables, estimates weights via constrained optimisation on a \
        training subset, and validates on the held-out test subset. Provides \
        bootstrap mean ± SE for each weight, plus the WQS coefficient β₁, \
        odds ratio, and p-value."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(EpiWqsNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: EpiWqsNodeSpec = serde_json::from_value(spec)?;
        if s.exposures.is_empty() {
            return Err(dag_core::registry::error::Error::SpecRejection {
                kind: "epi_wqs".to_string(),
                reason: "exposures must be a non-empty array".to_string(),
                schema_pretty: serde_json::to_string_pretty(&schema_for!(EpiWqsNodeSpec))
                    .unwrap_or_default(),
            });
        }
        if !(0.1..=0.9).contains(&s.train_frac) {
            return Err(dag_core::registry::error::Error::SpecRejection {
                kind: "epi_wqs".to_string(),
                reason: format!("train_frac must be in [0.1, 0.9], got {}", s.train_frac),
                schema_pretty: serde_json::to_string_pretty(&schema_for!(EpiWqsNodeSpec))
                    .unwrap_or_default(),
            });
        }
        Ok(Box::new(EpiWqsNode {
            meta: port_layout(),
            exposures: s.exposures.clone(),
            outcome_column: s.outcome_column,
            covariates: s.covariates,
            n_quantiles: s.n_quantiles,
            train_frac: s.train_frac,
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
        let s = parse_spec::<EpiWqsNodeSpec>(spec, "epi_wqs")?;
        let out = ctx.output_var.to_string();
        let wqs_fit = ctx.fresh_var("wqs_fit");
        let input = input_0(ctx).to_string();
        let exp_cols = s.exposures.iter().map(|c| c.clone()).collect::<Vec<_>>();
        let covars = if s.covariates.is_empty() {
            String::new()
        } else {
            format!(" + {}", s.covariates.join(" + "))
        };
        let exp_str = s.exposures.join(" + ");
        let code = vec![
            format!("# Weighted Quantile Sum regression"),
            format!("set.seed({})", s.seed),
            format!(
                "{wqs_fit} <- gwqs({} ~ wqs({}, q = {}){}, data = {input}, mix_name = c({}), b = {}, validation = 0, b1_pos = TRUE, pl = 10, family = gaussian)",
                s.outcome_column,
                exp_str,
                s.n_quantiles,
                covars,
                exp_cols
                    .iter()
                    .map(|c| format!("\"{c}\""))
                    .collect::<Vec<_>>()
                    .join(", "),
                s.n_bootstrap
            ),
            format!("{out} <- summary({wqs_fit})"),
            format!("print({out})"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["gWQS".into()]
    }
}

#[async_trait]
impl DagNode for EpiWqsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        "epi_wqs"
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
            .ok_or(EpiWqsError::Column("no input connected".to_string()))?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| EpiWqsError::Collect(e.to_string()))?;

        let y_raw = extract_numeric_lenient(&batches, &self.outcome_column)?;
        for &v in &y_raw {
            if !v.is_nan() && v != 0.0 && v != 1.0 {
                return Err(EpiWqsError::Column(format!(
                    "outcome '{}' must be binary (0/1), found {v}",
                    self.outcome_column
                ))
                .into());
            }
        }

        let x_raw: Vec<Vec<f64>> = self
            .exposures
            .iter()
            .map(|c| extract_numeric_lenient(&batches, c))
            .collect::<Result<_, _>>()?;
        let cov_raw: Vec<Vec<f64>> = self
            .covariates
            .iter()
            .map(|c| extract_numeric_lenient(&batches, c))
            .collect::<Result<_, _>>()?;

        // Complete-case filter.
        let n = y_raw.len();
        let mut y = Vec::with_capacity(n);
        let mut x_filtered: Vec<Vec<f64>> = vec![Vec::with_capacity(n); self.exposures.len()];
        let mut cov_filtered: Vec<Vec<f64>> = vec![Vec::with_capacity(n); self.covariates.len()];
        for i in 0..n {
            if y_raw[i].is_nan() || x_raw.iter().any(|x| x[i].is_nan()) {
                continue;
            }
            if cov_raw.iter().any(|c| c[i].is_nan()) {
                continue;
            }
            y.push(y_raw[i]);
            for (j, x) in x_raw.iter().enumerate() {
                x_filtered[j].push(x[i]);
            }
            for (j, c) in cov_raw.iter().enumerate() {
                cov_filtered[j].push(c[i]);
            }
        }
        if y.is_empty() {
            return Err(EpiWqsError::Column("no complete-case rows".to_string()).into());
        }

        let x_slices: Vec<&[f64]> = x_filtered.iter().map(|v| v.as_slice()).collect();
        let cov_slices: Vec<&[f64]> = cov_filtered.iter().map(|v| v.as_slice()).collect();
        let opts = epi::wqs::WqsOptions {
            n_quantiles: self.n_quantiles,
            train_frac: self.train_frac,
            n_bootstrap: self.n_bootstrap,
            seed: self.seed,
            ..Default::default()
        };
        let result = epi::wqs::wqs(&x_slices, &y, &cov_slices, self.exposures.clone(), &opts)
            .map_err(|e| EpiWqsError::Fit(e.to_string()))?;

        // --- Port 0: weights ---
        let weights_batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("feature", DataType::Utf8, false),
                Field::new("weight", DataType::Float64, false),
                Field::new("bootstrap_mean", DataType::Float64, false),
                Field::new("bootstrap_se", DataType::Float64, false),
            ])),
            vec![
                Arc::new(StringArray::from(result.feature_names.clone())),
                Arc::new(Float64Array::from(result.weights.clone())),
                Arc::new(Float64Array::from(result.bootstrap_mean_weights.clone())),
                Arc::new(Float64Array::from(result.bootstrap_se_weights.clone())),
            ],
        )
        .expect("wqs weights schema");

        // --- Port 1: summary ---
        let summary_batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("beta_wqs", DataType::Float64, false),
                Field::new("beta_wqs_se", DataType::Float64, false),
                Field::new("beta_wqs_p", DataType::Float64, false),
                Field::new("wqs_or", DataType::Float64, false),
                Field::new("wqs_or_lower", DataType::Float64, false),
                Field::new("wqs_or_upper", DataType::Float64, false),
                Field::new("test_p_value", DataType::Float64, true),
                Field::new("n_train", DataType::Int32, false),
                Field::new("n_test", DataType::Int32, false),
            ])),
            vec![
                Arc::new(Float64Array::from(vec![result.beta_wqs])),
                Arc::new(Float64Array::from(vec![result.beta_wqs_se])),
                Arc::new(Float64Array::from(vec![result.beta_wqs_p])),
                Arc::new(Float64Array::from(vec![result.wqs_or])),
                Arc::new(Float64Array::from(vec![result.wqs_or_lower])),
                Arc::new(Float64Array::from(vec![result.wqs_or_upper])),
                Arc::new(Float64Array::from(vec![result.test_p_value])),
                Arc::new(Int32Array::from(vec![result.n_train as i32])),
                Arc::new(Int32Array::from(vec![result.n_test as i32])),
            ],
        )
        .expect("wqs summary schema");

        let ctx = node_ctx.session();
        let df0 = ctx
            .read_batch(weights_batch)
            .map_err(|e| EpiWqsError::ReadBatch(e.to_string()))?;
        let df1 = ctx
            .read_batch(summary_batch)
            .map_err(|e| EpiWqsError::ReadBatch(e.to_string()))?;

        let mut res = PortOutputs::new();
        res.insert(0, df0);
        res.insert(1, df1);
        Ok(res)
    }
}
