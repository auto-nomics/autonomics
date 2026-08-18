//! Causal mediation analysis node.
//!
//! Wraps [`epi::mediation`]. Decomposes total effect into natural direct (NDE)
//! and natural indirect (NIE) effects via the two-model counterfactual approach.
//!
//! Output (single row):
//!
//! | Column          | Type    | Description                          |
//! |-----------------|---------|--------------------------------------|
//! | `nde`           | Float64 | Natural Direct Effect                |
//! | `nde_ci_lower`  | Float64 | NDE bootstrap 95% CI lower           |
//! | `nde_ci_upper`  | Float64 | NDE bootstrap 95% CI upper           |
//! | `nie`           | Float64 | Natural Indirect Effect              |
//! | `nie_ci_lower`  | Float64 | NIE bootstrap 95% CI lower           |
//! | `nie_ci_upper`  | Float64 | NIE bootstrap 95% CI upper           |
//! | `te`            | Float64 | Total Effect (NDE + NIE)             |
//! | `prop_mediated` | Float64 | Proportion mediated (NIE / TE)       |
//! | `alpha_x`       | Float64 | Mediator model α₁ (X → M)            |
//! | `beta_x`        | Float64 | Outcome model β₁ (direct X → Y)      |
//! | `beta_m`        | Float64 | Outcome model β₂ (M → Y)             |
//! | `n_obs`         | Int32   | Number of observations                |

use std::sync::Arc;

use arrow_array::{Float64Array, Int32Array, RecordBatch};
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
pub enum MediationError {
    #[error("{0}")]
    Column(String),
    #[error("{0}")]
    Fit(String),
    #[error("collect failed: {0}")]
    Collect(String),
    #[error("read_batch failed: {0}")]
    ReadBatch(String),
}

impl From<ColumnError> for MediationError {
    fn from(e: ColumnError) -> Self {
        Self::Column(e.to_string())
    }
}

impl ::dag_core::dag::NodeError for MediationError {
    fn node_type(&self) -> &str {
        "mediation"
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct MediationNodeSpec {
    /// Exposure column name.
    pub exposure_column: String,
    /// Mediator column name.
    pub mediator_column: String,
    /// Outcome column name.
    pub outcome_column: String,
    /// Optional confounder column names.
    #[serde(default)]
    pub covariates: Vec<String>,
    /// Include X×M interaction in the outcome model (default false).
    #[serde(default)]
    pub interaction: bool,
    /// Bootstrap iterations (default 1000).
    #[serde(default = "default_n_boot")]
    pub n_bootstrap: usize,
    /// Random seed (default 42).
    #[serde(default = "default_seed")]
    pub seed: u64,
}

fn default_n_boot() -> usize {
    1000
}
fn default_seed() -> u64 {
    42
}

#[derive(Clone)]
pub struct MediationNode {
    meta: NodePorts,
    exposure_column: String,
    mediator_column: String,
    outcome_column: String,
    covariates: Vec<String>,
    interaction: bool,
    n_bootstrap: usize,
    seed: u64,
}

pub struct MediationNodeFactory {}

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None).add_input_port(None)
}

impl NodeFactory for MediationNodeFactory {
    fn kind(&self) -> &'static str {
        "mediation"
    }
    fn desc(&self) -> &'static str {
        "Causal mediation analysis (NDE/NIE/TE decomposition)."
    }
    fn doc(&self) -> &'static str {
        "Performs causal mediation analysis via the two-model counterfactual \
        approach (VanderWeele 2015). Fits mediator (M ~ X + C) and outcome \
        (Y ~ X + M [+ X:M] + C) models via OLS, then decomposes the total \
        effect into natural direct (NDE) and natural indirect (NIE) effects. \
        Bootstrap percentile CIs."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MediationNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: MediationNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(MediationNode {
            meta: port_layout(),
            exposure_column: s.exposure_column,
            mediator_column: s.mediator_column,
            outcome_column: s.outcome_column,
            covariates: s.covariates,
            interaction: s.interaction,
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
        let s = parse_spec::<MediationNodeSpec>(spec, "mediation")?;
        let out = ctx.output_var.to_string();
        let covars = if s.covariates.is_empty() {
            String::new()
        } else {
            format!(" + {}", s.covariates.join(" + "))
        };
        let m_model = ctx.fresh_var("model_m");
        let y_model = ctx.fresh_var("model_y");
        let med = ctx.fresh_var("med_result");
        let med_smry = ctx.fresh_var("med_smry");
        let input = input_0(ctx).to_string();
        let code = vec![
            format!("# Mediation analysis"),
            format!(
                "# Mediator model: {} ~ {}{}",
                s.mediator_column, s.exposure_column, covars
            ),
            format!(
                "{m_model} <- lm({} ~ {}{}, data = {input})",
                s.mediator_column, s.exposure_column, covars
            ),
            format!(
                "# Outcome model: {} ~ {} + {}{}",
                s.outcome_column, s.exposure_column, s.mediator_column, covars
            ),
            format!(
                "{y_model} <- lm({} ~ {} + {}{}, data = {input})",
                s.outcome_column, s.exposure_column, s.mediator_column, covars
            ),
            format!("set.seed({})", s.seed),
            format!(
                "{med} <- mediate({m_model}, {y_model}, treat = \"{}\", mediator = \"{}\", boot = TRUE, sims = {})",
                s.exposure_column, s.mediator_column, s.n_bootstrap
            ),
            format!("{med_smry} <- summary({med})"),
            format!("{out} <- data.frame("),
            format!("  nde = as.numeric({med_smry}$d0),"),
            format!("  nde_ci_lower = as.numeric({med_smry}$d0.ci[1]),"),
            format!("  nde_ci_upper = as.numeric({med_smry}$d0.ci[2]),"),
            format!("  nie = as.numeric({med_smry}$z0),"),
            format!("  nie_ci_lower = as.numeric({med_smry}$z0.ci[1]),"),
            format!("  nie_ci_upper = as.numeric({med_smry}$z0.ci[2]),"),
            format!("  te = as.numeric({med_smry}$tau.coef),"),
            format!("  te_ci_lower = as.numeric({med_smry}$tau.ci[1]),"),
            format!("  te_ci_upper = as.numeric({med_smry}$tau.ci[2]),"),
            format!("  prop_mediated = as.numeric({med_smry}$n.avg),"),
            format!("  alpha_x = as.numeric(coef({m_model})[2]),"),
            format!("  beta_x = as.numeric(coef({y_model})[2]),"),
            format!("  beta_m = as.numeric(coef({y_model})[3]),"),
            format!("  n_obs = as.integer(nobs({y_model}))"),
            format!(")"),
            format!("print({out})"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["mediation".into()]
    }
}

#[async_trait]
impl DagNode for MediationNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        "mediation"
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
            .ok_or(MediationError::Column("no input connected".to_string()))?;
        let batches = input
            .dataframe()?
            .clone()
            .collect()
            .await
            .map_err(|e| MediationError::Collect(e.to_string()))?;

        let x_raw = dag_core::arrow_util::extract_numeric_lenient(&batches, &self.exposure_column)?;
        let m_raw = dag_core::arrow_util::extract_numeric_lenient(&batches, &self.mediator_column)?;
        let y_raw = dag_core::arrow_util::extract_numeric_lenient(&batches, &self.outcome_column)?;
        let cov_raw: Vec<Vec<f64>> = self
            .covariates
            .iter()
            .map(|c| dag_core::arrow_util::extract_numeric_lenient(&batches, c))
            .collect::<Result<_, _>>()?;

        // Complete-case filter.
        let n = y_raw.len();
        let mut x = Vec::with_capacity(n);
        let mut m = Vec::with_capacity(n);
        let mut y = Vec::with_capacity(n);
        let mut cov_filtered: Vec<Vec<f64>> = vec![Vec::with_capacity(n); self.covariates.len()];
        for i in 0..n {
            if x_raw[i].is_nan()
                || m_raw[i].is_nan()
                || y_raw[i].is_nan()
                || cov_raw.iter().any(|c| c[i].is_nan())
            {
                continue;
            }
            x.push(x_raw[i]);
            m.push(m_raw[i]);
            y.push(y_raw[i]);
            for (j, c) in cov_raw.iter().enumerate() {
                cov_filtered[j].push(c[i]);
            }
        }
        if x.is_empty() {
            return Err(MediationError::Column("no complete-case rows".to_string()).into());
        }

        let cov_slices: Vec<&[f64]> = cov_filtered.iter().map(|v| v.as_slice()).collect();
        let opts = epi::mediation::MediationOptions {
            n_bootstrap: self.n_bootstrap,
            seed: self.seed,
            ..Default::default()
        };
        let result = epi::mediation::mediation(&x, &m, &y, &cov_slices, self.interaction, &opts)
            .map_err(|e| MediationError::Fit(e.to_string()))?;

        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("nde", DataType::Float64, false),
                Field::new("nde_ci_lower", DataType::Float64, false),
                Field::new("nde_ci_upper", DataType::Float64, false),
                Field::new("nie", DataType::Float64, false),
                Field::new("nie_ci_lower", DataType::Float64, false),
                Field::new("nie_ci_upper", DataType::Float64, false),
                Field::new("te", DataType::Float64, false),
                Field::new("te_ci_lower", DataType::Float64, false),
                Field::new("te_ci_upper", DataType::Float64, false),
                Field::new("prop_mediated", DataType::Float64, true),
                Field::new("alpha_x", DataType::Float64, false),
                Field::new("beta_x", DataType::Float64, false),
                Field::new("beta_m", DataType::Float64, false),
                Field::new("n_obs", DataType::Int32, false),
            ])),
            vec![
                Arc::new(Float64Array::from(vec![result.nde])),
                Arc::new(Float64Array::from(vec![result.nde_ci_lower])),
                Arc::new(Float64Array::from(vec![result.nde_ci_upper])),
                Arc::new(Float64Array::from(vec![result.nie])),
                Arc::new(Float64Array::from(vec![result.nie_ci_lower])),
                Arc::new(Float64Array::from(vec![result.nie_ci_upper])),
                Arc::new(Float64Array::from(vec![result.te])),
                Arc::new(Float64Array::from(vec![result.te_ci_lower])),
                Arc::new(Float64Array::from(vec![result.te_ci_upper])),
                Arc::new(Float64Array::from(vec![result.prop_mediated])),
                Arc::new(Float64Array::from(vec![result.alpha_x])),
                Arc::new(Float64Array::from(vec![result.beta_x])),
                Arc::new(Float64Array::from(vec![result.beta_m])),
                Arc::new(Int32Array::from(vec![result.n_obs as i32])),
            ],
        )
        .expect("mediation schema");

        let ctx = node_ctx.session();
        let df = ctx
            .read_batch(batch)
            .map_err(|e| MediationError::ReadBatch(e.to_string()))?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}
