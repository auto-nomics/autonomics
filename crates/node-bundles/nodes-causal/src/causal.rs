//! IPTW + PSM causal inference node.
//!
//! Output: ATE/ATT with bootstrap CI, propensity scores, weights.

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
pub enum CausalError {
    #[error("{0}")]
    Column(String),
    #[error("{0}")]
    Fit(String),
    #[error("collect failed: {0}")]
    Collect(String),
    #[error("read_batch failed: {0}")]
    ReadBatch(String),
}

impl From<ColumnError> for CausalError {
    fn from(e: ColumnError) -> Self {
        Self::Column(e.to_string())
    }
}
impl ::dag_core::dag::NodeError for CausalError {
    fn node_type(&self) -> &str {
        "causal"
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CausalNodeSpec {
    /// Method: "iptw" or "psm".
    pub method: String,
    /// Binary treatment column (0/1).
    pub treatment_column: String,
    /// Outcome column.
    pub outcome_column: String,
    /// Confounder column names.
    pub covariates: Vec<String>,
    /// Bootstrap iterations (default 1000).
    #[serde(default = "default_n_boot")]
    pub n_bootstrap: usize,
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
pub struct CausalNode {
    meta: NodePorts,
    spec: CausalNodeSpec,
}
pub struct CausalNodeFactory {}
fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None).add_input_port(None)
}

impl NodeFactory for CausalNodeFactory {
    fn kind(&self) -> &'static str {
        "causal"
    }
    fn desc(&self) -> &'static str {
        "IPTW or PSM causal inference (propensity score based)."
    }
    fn doc(&self) -> &'static str {
        "Estimates treatment effects via IPTW (ATE) or PSM (ATT). Propensity scores estimated from logistic regression on covariates. Bootstrap percentile CIs."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(CausalNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: CausalNodeSpec = serde_json::from_value(spec)?;
        if s.method != "iptw" && s.method != "psm" {
            return Err(dag_core::registry::error::Error::SpecRejection {
                kind: "causal".into(),
                reason: format!("method must be 'iptw' or 'psm', got '{}'", s.method),
                schema_pretty: serde_json::to_string_pretty(&schema_for!(CausalNodeSpec))
                    .unwrap_or_default(),
            });
        }
        Ok(Box::new(CausalNode {
            meta: port_layout(),
            spec: s,
        }))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<CausalNodeSpec>(spec, "causal")?;
        let out = ctx.output_var.to_string();
        let covars = s.covariates.join(" + ");
        let ps_model = ctx.fresh_var("ps_model");
        let ps_score = ctx.fresh_var("ps_score");
        let weight_var = ctx.fresh_var("ipw");
        let input = input_0(ctx).to_string();
        let wlm = ctx.fresh_var("wlm");
        let code = match s.method.as_str() {
            "iptw" | "ipw" => vec![
                format!("# Causal inference via IPTW"),
                format!(
                    "# Propensity score model: {t} ~ {c}",
                    t = s.treatment_column,
                    c = covars
                ),
                format!(
                    "{ps_model} <- glm({t} ~ {c}, data = {input}, family = binomial)",
                    t = s.treatment_column,
                    c = covars
                ),
                format!("{ps_score} <- predict({ps_model}, type = \"response\")"),
                format!(
                    "{weight_var} <- ifelse({input}${t} == 1, 1/{ps_score}, 1/(1-{ps_score}))",
                    t = s.treatment_column
                ),
                format!(
                    "{wlm} <- lm({y} ~ {t}, data = {input}, weights = {weight_var})",
                    y = s.outcome_column,
                    t = s.treatment_column
                ),
                format!("{out} <- data.frame("),
                format!("  method = \"iptw\","),
                format!("  ate = coef({wlm})[2],"),
                format!("  ate_se = summary({wlm})$coefficients[2, 2],"),
                format!(
                    "  n_treated = sum({input}${t} == 1),",
                    t = s.treatment_column
                ),
                format!("  n_obs = nrow({input})"),
                format!(")"),
                format!("{out}$ate_ci_lower <- {out}$ate - 1.96 * {out}$ate_se"),
                format!("{out}$ate_ci_upper <- {out}$ate + 1.96 * {out}$ate_se"),
                format!("print({out})"),
            ],
            "psm" | "matching" => vec![
                format!("# Causal inference via propensity score matching"),
                format!(
                    "matched <- matchit({t} ~ {c}, data = {input}, method = \"nearest\")",
                    t = s.treatment_column,
                    c = covars
                ),
                format!("matched_data <- MatchIt::match.data(matched)"),
                format!(
                    "{out}_fit <- lm({y} ~ {t}, data = matched_data)",
                    y = s.outcome_column,
                    t = s.treatment_column
                ),
                format!("{out}_smry <- summary({out}_fit)"),
                format!("{out} <- data.frame("),
                format!("  method = \"psm\","),
                format!("  att = {out}_smry$coefficients[2, 1],"),
                format!("  att_se = {out}_smry$coefficients[2, 2],"),
                format!(
                    "  n_treated = sum(matched_data${t} == 1),",
                    t = s.treatment_column
                ),
                format!("  n_obs = nrow(matched_data)"),
                format!(")"),
                format!("print({out})"),
            ],
            other => vec![format!(
                "# NOTE: causal method '{other}' not yet mapped to R codegen"
            )],
        };
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["MatchIt".into(), "survey".into()]
    }
}

#[async_trait]
impl DagNode for CausalNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        "causal"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs
            .first()
            .ok_or(CausalError::Column("no input".into()))?;
        let batches = input
            .dataframe()?
            .clone()
            .collect()
            .await
            .map_err(|e| CausalError::Collect(e.to_string()))?;

        let t_raw =
            dag_core::arrow_util::extract_numeric_lenient(&batches, &self.spec.treatment_column)?;
        let y_raw =
            dag_core::arrow_util::extract_numeric_lenient(&batches, &self.spec.outcome_column)?;
        let cov_raw: Vec<Vec<f64>> = self
            .spec
            .covariates
            .iter()
            .map(|c| dag_core::arrow_util::extract_numeric_lenient(&batches, c))
            .collect::<Result<_, _>>()?;
        let n = t_raw.len();
        let mut t = Vec::with_capacity(n);
        let mut y = Vec::with_capacity(n);
        let mut cov_f: Vec<Vec<f64>> = vec![Vec::with_capacity(n); self.spec.covariates.len()];
        for i in 0..n {
            if t_raw[i].is_nan() || y_raw[i].is_nan() || cov_raw.iter().any(|c| c[i].is_nan()) {
                continue;
            }
            if t_raw[i] != 0.0 && t_raw[i] != 1.0 {
                return Err(CausalError::Column(format!(
                    "treatment must be 0/1, got {}",
                    t_raw[i]
                ))
                .into());
            }
            t.push(t_raw[i]);
            y.push(y_raw[i]);
            for (j, c) in cov_raw.iter().enumerate() {
                cov_f[j].push(c[i]);
            }
        }
        if t.is_empty() {
            return Err(CausalError::Column("no complete-case rows".into()).into());
        }
        let cov_slices: Vec<&[f64]> = cov_f.iter().map(|v| v.as_slice()).collect();

        let batch = match self.spec.method.as_str() {
            "iptw" => {
                let opts = epi::causal::IptwOptions {
                    n_bootstrap: self.spec.n_bootstrap,
                    seed: self.spec.seed,
                    ..Default::default()
                };
                let r = epi::causal::iptw(&t, &y, &cov_slices, &opts)
                    .map_err(|e| CausalError::Fit(e.to_string()))?;
                RecordBatch::try_new(
                    Arc::new(Schema::new(vec![
                        Field::new("ate", DataType::Float64, false),
                        Field::new("ate_se", DataType::Float64, false),
                        Field::new("ate_ci_lower", DataType::Float64, false),
                        Field::new("ate_ci_upper", DataType::Float64, false),
                        Field::new("ess", DataType::Float64, false),
                        Field::new("n_treated", DataType::Int32, false),
                        Field::new("n_obs", DataType::Int32, false),
                    ])),
                    vec![
                        Arc::new(Float64Array::from(vec![r.ate])),
                        Arc::new(Float64Array::from(vec![r.ate_se])),
                        Arc::new(Float64Array::from(vec![r.ate_ci_lower])),
                        Arc::new(Float64Array::from(vec![r.ate_ci_upper])),
                        Arc::new(Float64Array::from(vec![r.ess])),
                        Arc::new(Int32Array::from(vec![r.n_treated as i32])),
                        Arc::new(Int32Array::from(vec![r.n_obs as i32])),
                    ],
                )
                .unwrap()
            }
            "psm" => {
                let opts = epi::causal::PsmOptions {
                    n_bootstrap: self.spec.n_bootstrap,
                    seed: self.spec.seed,
                    ..Default::default()
                };
                let r = epi::causal::psm(&t, &y, &cov_slices, &opts)
                    .map_err(|e| CausalError::Fit(e.to_string()))?;
                RecordBatch::try_new(
                    Arc::new(Schema::new(vec![
                        Field::new("att", DataType::Float64, false),
                        Field::new("att_se", DataType::Float64, false),
                        Field::new("att_ci_lower", DataType::Float64, false),
                        Field::new("att_ci_upper", DataType::Float64, false),
                        Field::new("n_matched", DataType::Int32, false),
                        Field::new("n_treated", DataType::Int32, false),
                        Field::new("n_control", DataType::Int32, false),
                    ])),
                    vec![
                        Arc::new(Float64Array::from(vec![r.att])),
                        Arc::new(Float64Array::from(vec![r.att_se])),
                        Arc::new(Float64Array::from(vec![r.att_ci_lower])),
                        Arc::new(Float64Array::from(vec![r.att_ci_upper])),
                        Arc::new(Int32Array::from(vec![r.n_matched as i32])),
                        Arc::new(Int32Array::from(vec![r.n_treated as i32])),
                        Arc::new(Int32Array::from(vec![r.n_control as i32])),
                    ],
                )
                .unwrap()
            }
            _ => unreachable!(),
        };
        let ctx = node_ctx.session();
        let df = ctx
            .read_batch(batch)
            .map_err(|e| CausalError::ReadBatch(e.to_string()))?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}
