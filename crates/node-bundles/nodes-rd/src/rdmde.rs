//! `rdmde` DAG node: minimum detectable effect calculations for RD designs.

use std::sync::Arc;

use arrow_array::{Float64Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
};

use crate::common::*;

pub const RDMDE_NODE_KIND: &str = "rdmde";

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RdMdeNodeConfig {
    pub y: String,
    pub x: String,
    #[serde(default = "default_cutoff")]
    pub cutoff: f64,
    #[serde(default = "default_alpha")]
    pub alpha: f64,
    #[serde(default = "default_beta")]
    pub beta: f64,
    #[serde(default = "default_p")]
    pub p: usize,
    #[serde(default)]
    pub deriv: usize,
    #[serde(default = "default_kernel")]
    pub kernel: String,
    #[serde(default = "default_bwselect")]
    pub bwselect: String,
    #[serde(default = "default_vce")]
    pub vce: String,
    #[serde(default)]
    pub init_cond: Option<f64>,
}

fn default_cutoff() -> f64 {
    0.0
}
fn default_alpha() -> f64 {
    0.05
}
fn default_beta() -> f64 {
    0.8
}
fn default_p() -> usize {
    1
}
fn default_kernel() -> String {
    "triangular".into()
}
fn default_bwselect() -> String {
    "mserd".into()
}
fn default_vce() -> String {
    "nn".into()
}

fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("inference", DataType::Utf8, false),
        Field::new("mde", DataType::Float64, true),
        Field::new("se", DataType::Float64, true),
        Field::new("sampsi_l", DataType::Int64, true),
        Field::new("sampsi_r", DataType::Int64, true),
        Field::new("samph_l", DataType::Float64, true),
        Field::new("samph_r", DataType::Float64, true),
        Field::new("n_l", DataType::Int64, true),
        Field::new("n_r", DataType::Int64, true),
        Field::new("alpha", DataType::Float64, true),
        Field::new("beta", DataType::Float64, true),
    ]))
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(None)
        .add_output_port(Some(output_schema()))
}

#[derive(Clone)]
pub struct RdMdeNode {
    meta: NodePorts,
    config: RdMdeNodeConfig,
}

impl RdMdeNode {
    pub fn new(config: RdMdeNodeConfig) -> Self {
        Self {
            meta: port_layout(),
            config,
        }
    }
}

pub struct RdMdeNodeFactory;

impl NodeFactory for RdMdeNodeFactory {
    fn kind(&self) -> &'static str {
        RDMDE_NODE_KIND
    }
    fn desc(&self) -> &'static str {
        "Minimum detectable effect (MDE) for RD designs."
    }
    fn doc(&self) -> &'static str {
        "Computes the minimum detectable effect size for a given power in an RD design."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(RdMdeNodeConfig)
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let config: RdMdeNodeConfig = serde_json::from_value(spec)?;
        Ok(Box::new(RdMdeNode::new(config)))
    }
}

#[async_trait]
impl DagNode for RdMdeNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        RDMDE_NODE_KIND
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
        let input = inputs.first().ok_or(RdNodeError::EmptyInput)?;
        let batches: Vec<RecordBatch> =
            input
                .dataframe()?
                .clone()
                .collect()
                .await
                .map_err(|e| DagError::NodeError {
                    node_type: RDMDE_NODE_KIND.into(),
                    msg: format!("collect failed: {e}"),
                })?;
        if batches.is_empty() {
            return Err(RdNodeError::EmptyInput.into());
        }

        let cfg = &self.config;
        let y = extract_f64(&batches, &cfg.y)?;
        let r = extract_f64(&batches, &cfg.x)?;

        let rd_cfg = rdpower::RdMdeConfig {
            y,
            r,
            cutoff: cfg.cutoff,
            alpha: cfg.alpha,
            beta: cfg.beta,
            p: cfg.p,
            deriv: cfg.deriv,
            kernel: rdrobust::Kernel::parse(&cfg.kernel),
            bwselect: cfg.bwselect.clone(),
            vce: cfg.vce.clone(),
            init_cond: cfg.init_cond,
            ..Default::default()
        };

        let result = rdpower::rdmde(&rd_cfg).map_err(|e| DagError::NodeError {
            node_type: RDMDE_NODE_KIND.into(),
            msg: e.to_string(),
        })?;

        let batch = build_rdmde_result(&result)?;
        let df = node_ctx
            .session()
            .read_batch(batch)
            .map_err(RdNodeError::ReadBatch)?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

fn build_rdmde_result(r: &rdpower::RdMdeResult) -> Result<RecordBatch, RdNodeError> {
    let inference = StringArray::from(vec!["Robust bias-corrected", "Conventional"]);
    let mde = Float64Array::from(vec![Some(r.mde), Some(r.mde_conv)]);
    let se = Float64Array::from(vec![Some(r.se_rbc), Some(r.se_conv)]);
    let sampsi_l = Int64Array::from(vec![Some(r.sampsi_l as i64); 2]);
    let sampsi_r = Int64Array::from(vec![Some(r.sampsi_r as i64); 2]);
    let samph_l = Float64Array::from(vec![Some(r.samph_l); 2]);
    let samph_r = Float64Array::from(vec![Some(r.samph_r); 2]);
    let n_l = Int64Array::from(vec![Some(r.n_l as i64); 2]);
    let n_r = Int64Array::from(vec![Some(r.n_r as i64); 2]);
    let alpha = Float64Array::from(vec![Some(r.alpha); 2]);
    let beta = Float64Array::from(vec![Some(r.beta); 2]);

    Ok(RecordBatch::try_new(
        output_schema(),
        vec![
            Arc::new(inference),
            Arc::new(mde),
            Arc::new(se),
            Arc::new(sampsi_l),
            Arc::new(sampsi_r),
            Arc::new(samph_l),
            Arc::new(samph_r),
            Arc::new(n_l),
            Arc::new(n_r),
            Arc::new(alpha),
            Arc::new(beta),
        ],
    )?)
}
