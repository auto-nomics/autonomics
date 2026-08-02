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

use super::meta::{DagNode, NodeInput, NodePorts};
use super::numeric_util::ColumnError;
use crate::{
    dag::{DagError, graph::PortOutputs},
    node_registry::registry::{NodeCtx, NodeFactory},
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
impl From<CausalError> for DagError {
    fn from(e: CausalError) -> Self {
        DagError::NodeError {
            node_type: "causal".to_string(),
            msg: e.to_string(),
        }
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
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        let s: CausalNodeSpec = serde_json::from_value(spec)?;
        if s.method != "iptw" && s.method != "psm" {
            return Err(crate::node_registry::error::Error::SpecRejection {
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
        _: &crate::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs
            .first()
            .ok_or(CausalError::Column("no input".into()))?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| CausalError::Collect(e.to_string()))?;

        let t_raw =
            super::numeric_util::extract_numeric_lenient(&batches, &self.spec.treatment_column)?;
        let y_raw =
            super::numeric_util::extract_numeric_lenient(&batches, &self.spec.outcome_column)?;
        let cov_raw: Vec<Vec<f64>> = self
            .spec
            .covariates
            .iter()
            .map(|c| super::numeric_util::extract_numeric_lenient(&batches, c))
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
