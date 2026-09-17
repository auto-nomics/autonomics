//! E-Value sensitivity analysis node.
//!
//! Computes E-values for unmeasured confounding given an effect estimate
//! and optional confidence interval. Supports RR, OR, HR, OLS, and MD
//! effect measures. Uses the [`evalue`] crate (Rust port of the R `EValue`
//! package).
//!
//! The node is purely computational (no upstream inputs needed) — it takes
//! effect measure parameters directly from its config and outputs a single-row
//! DataFrame with RR-converted values and E-values.

use std::sync::Arc;

use arrow_array::{Float64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
};

// =====================================================================
// Error type
// =====================================================================

#[derive(Debug, Error)]
pub enum EvalueNodeError {
    #[error("E-value computation failed: {0}")]
    Evalue(#[from] evalue::EvalueError),
    #[error("failed to build result batch: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
    #[error("failed to read result batch: {0}")]
    ReadBatch(#[from] datafusion::error::DataFusionError),
}

impl ::dag_core::dag::NodeError for EvalueNodeError {
    fn node_type(&self) -> &str {
        "evalue"
    }
}

// =====================================================================
// Config
// =====================================================================

/// Effect measure type.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub enum MeasureType {
    RR,
    OR,
    HR,
    OLS,
    MD,
}

/// Configuration for the E-value node.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct EvalueConfig {
    /// Effect measure type.
    pub measure: MeasureType,
    /// Point estimate.
    pub est: f64,
    /// Lower confidence interval limit (optional).
    #[serde(default)]
    pub lo: Option<f64>,
    /// Upper confidence interval limit (optional).
    #[serde(default)]
    pub hi: Option<f64>,
    /// Standard error of the point estimate (for OLS/MD).
    #[serde(default)]
    pub se: Option<f64>,
    /// Standard deviation of the outcome (for OLS only).
    #[serde(default)]
    pub sd: Option<f64>,
    /// Exposure contrast (for OLS only, default 1.0).
    #[serde(default = "default_delta")]
    pub delta: f64,
    /// True value to shift to (default: 1 for ratio measures, 0 for additive).
    #[serde(default)]
    pub true_val: Option<f64>,
    /// Whether the outcome is rare (for OR/HR, default false).
    #[serde(default = "default_false")]
    pub rare: bool,
}

fn default_delta() -> f64 {
    1.0
}
fn default_false() -> bool {
    false
}

// =====================================================================
// Output schema
// =====================================================================

fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("measure", DataType::Utf8, false),
        Field::new("rr_point", DataType::Float64, true),
        Field::new("rr_lower", DataType::Float64, true),
        Field::new("rr_upper", DataType::Float64, true),
        Field::new("evalue_point", DataType::Float64, true),
        Field::new("evalue_lower", DataType::Float64, true),
        Field::new("evalue_upper", DataType::Float64, true),
    ]))
}

// =====================================================================
// Node
// =====================================================================

const EVALUE_NODE_KIND: &str = "evalue";

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port(Some(output_schema()))
}

#[derive(Clone)]
pub struct EvalueNode {
    meta: NodePorts,
    config: EvalueConfig,
}

pub struct EvalueNodeFactory {}

impl NodeFactory for EvalueNodeFactory {
    fn kind(&self) -> &'static str {
        EVALUE_NODE_KIND
    }

    fn desc(&self) -> &'static str {
        "E-value sensitivity analysis for unmeasured confounding (RR/OR/HR/OLS/MD)."
    }

    fn doc(&self) -> &'static str {
        "E-value sensitivity analysis node. Computes the E-value — the minimum \
        strength of association (on the risk-ratio scale) that unmeasured \
        confounding would need to have with both exposure and outcome to fully \
        explain away an observed association.\n\n\
        Supports effect measures: RR (risk ratio), OR (odds ratio), HR (hazard \
        ratio), OLS (linear regression coefficient), MD (standardized mean \
        difference).\n\n\
        Outputs a single-row DataFrame with RR-converted values and E-values \
        for the point estimate and the CI limit closer to the null."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(EvalueConfig)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let config: EvalueConfig = serde_json::from_value(spec)?;
        let node = EvalueNode::new(config);
        Ok(Box::new(node))
    }
}

impl EvalueNode {
    pub fn new(config: EvalueConfig) -> Self {
        Self {
            meta: port_layout(),
            config,
        }
    }

    fn compute(&self) -> Result<RecordBatch, EvalueNodeError> {
        let cfg = &self.config;

        let result = match cfg.measure {
            MeasureType::RR => {
                let true_val = cfg.true_val.unwrap_or(1.0);
                evalue::evalue::evalues_rr(cfg.est, cfg.lo, cfg.hi, true_val)?
            }
            MeasureType::OR => {
                let true_val = cfg.true_val.unwrap_or(1.0);
                evalue::evalue::evalues_or(cfg.est, cfg.lo, cfg.hi, cfg.rare, true_val)?
            }
            MeasureType::HR => {
                let true_val = cfg.true_val.unwrap_or(1.0);
                evalue::evalue::evalues_hr(cfg.est, cfg.lo, cfg.hi, cfg.rare, true_val)?
            }
            MeasureType::MD => {
                let true_val = cfg.true_val.unwrap_or(0.0);
                evalue::evalue::evalues_md(cfg.est, cfg.se, true_val)?
            }
            MeasureType::OLS => {
                let true_val = cfg.true_val.unwrap_or(0.0);
                let sd = cfg.sd.ok_or_else(|| {
                    EvalueNodeError::Evalue(evalue::EvalueError::Invalid(
                        "OLS measure requires sd".into(),
                    ))
                })?;
                evalue::evalue::evalues_ols(cfg.est, cfg.se, sd, cfg.delta, true_val)?
            }
        };

        let measure_str = result.measure;
        let batch = RecordBatch::try_new(
            output_schema(),
            vec![
                Arc::new(StringArray::from(vec![measure_str])),
                Arc::new(Float64Array::from(vec![result.rr_values[0]])),
                Arc::new(Float64Array::from(vec![result.rr_values[1]])),
                Arc::new(Float64Array::from(vec![result.rr_values[2]])),
                Arc::new(Float64Array::from(vec![result.evalues[0]])),
                Arc::new(Float64Array::from(vec![result.evalues[1]])),
                Arc::new(Float64Array::from(vec![result.evalues[2]])),
            ],
        )?;

        Ok(batch)
    }
}

#[async_trait]
impl DagNode for EvalueNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        EVALUE_NODE_KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        _inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let batch = self.compute()?;
        let ctx = node_ctx.session();
        let df = ctx.read_batch(batch).map_err(EvalueNodeError::ReadBatch)?;

        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

impl Default for EvalueConfig {
    fn default() -> Self {
        Self {
            measure: MeasureType::RR,
            est: 1.0,
            lo: None,
            hi: None,
            se: None,
            sd: None,
            delta: default_delta(),
            true_val: None,
            rare: default_false(),
        }
    }
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::prelude::SessionContext;

    fn node_ctx() -> NodeCtx {
        NodeCtx::new(SessionContext::new().runtime_env(), None)
    }

    #[tokio::test]
    async fn test_evalue_rr_node() {
        let mut node = EvalueNode::new(EvalueConfig {
            measure: MeasureType::RR,
            est: 0.80,
            lo: Some(0.71),
            hi: Some(0.91),
            true_val: Some(1.0),
            ..Default::default()
        });

        let res = node
            .execute(
                &node_ctx(),
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();

        let outputs = res.dataframe(0).unwrap().clone();
        let batch = outputs.collect().await.unwrap().into_iter().next().unwrap();
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 7);

        let evalue_point = batch
            .column(4)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .value(0);
        // E-value for RR=0.80 should be ≈ 1.809
        assert!(
            (evalue_point - 1.809).abs() < 0.01,
            "E-value point: {evalue_point}"
        );
    }

    #[tokio::test]
    async fn test_evalue_or_node() {
        let mut node = EvalueNode::new(EvalueConfig {
            measure: MeasureType::OR,
            est: 0.86,
            lo: Some(0.75),
            hi: Some(0.99),
            rare: false,
            true_val: Some(1.0),
            ..Default::default()
        });

        let res = node
            .execute(
                &node_ctx(),
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();

        let outputs = res.dataframe(0).unwrap().clone();
        let batch = outputs.collect().await.unwrap().into_iter().next().unwrap();
        let evalue_point = batch
            .column(4)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .value(0);
        assert!(evalue_point > 1.0, "E-value should be > 1: {evalue_point}");
    }

    #[tokio::test]
    async fn test_evalue_ols_node() {
        let mut node = EvalueNode::new(EvalueConfig {
            measure: MeasureType::OLS,
            est: 0.3,
            se: Some(0.1),
            sd: Some(1.0),
            delta: 1.0,
            true_val: Some(0.0),
            ..Default::default()
        });

        let res = node
            .execute(
                &node_ctx(),
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();

        let outputs = res.dataframe(0).unwrap().clone();
        let batch = outputs.collect().await.unwrap().into_iter().next().unwrap();
        let evalue_point = batch
            .column(4)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .value(0);
        assert!(evalue_point.is_finite());
    }
}
