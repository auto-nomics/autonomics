//! `rdpower` DAG node: power calculations for RD designs.

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

pub const RDPOWER_NODE_KIND: &str = "rdpower";

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RdPowerNodeConfig {
    pub y: String,
    pub x: String,
    #[serde(default = "default_cutoff")]
    pub cutoff: f64,
    #[serde(default)]
    pub tau: Option<f64>,
    #[serde(default = "default_alpha")]
    pub alpha: f64,
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
    #[serde(default = "default_level")]
    pub level: f64,
}

fn default_cutoff() -> f64 { 0.0 }
fn default_alpha() -> f64 { 0.05 }
fn default_p() -> usize { 1 }
fn default_kernel() -> String { "triangular".into() }
fn default_bwselect() -> String { "mserd".into() }
fn default_vce() -> String { "nn".into() }
fn default_level() -> f64 { 95.0 }

fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("inference", DataType::Utf8, false),
        Field::new("tau", DataType::Float64, false),
        Field::new("power", DataType::Float64, true),
        Field::new("se", DataType::Float64, true),
        Field::new("sampsi_l", DataType::Int64, true),
        Field::new("sampsi_r", DataType::Int64, true),
        Field::new("samph_l", DataType::Float64, true),
        Field::new("samph_r", DataType::Float64, true),
        Field::new("n_l", DataType::Int64, true),
        Field::new("n_r", DataType::Int64, true),
        Field::new("alpha", DataType::Float64, true),
        Field::new("size_distortion", DataType::Float64, true),
    ]))
}

fn port_layout() -> NodePorts {
    NodePorts::new().add_input_port(None).add_output_port(Some(output_schema()))
}

#[derive(Clone)]
pub struct RdPowerNode {
    meta: NodePorts,
    config: RdPowerNodeConfig,
}

impl RdPowerNode {
    pub fn new(config: RdPowerNodeConfig) -> Self {
        Self { meta: port_layout(), config }
    }
}

pub struct RdPowerNodeFactory;

impl NodeFactory for RdPowerNodeFactory {
    fn kind(&self) -> &'static str { RDPOWER_NODE_KIND }
    fn desc(&self) -> &'static str { "Power calculations for RD designs (Cattaneo, Titiunik, Vázquez-Bare 2019)." }
    fn doc(&self) -> &'static str {
        "Computes the power of an RD design against a specified treatment effect, \
        using robust bias-corrected and conventional inference from rdrobust."
    }
    fn spec_schema(&self) -> schemars::Schema { schema_for!(RdPowerNodeConfig) }
    fn ports(&self) -> NodePorts { port_layout() }

    fn build(&self, spec: serde_json::Value, _ctx: NodeCtx)
        -> dag_core::registry::error::Result<Box<dyn DagNode>>
    {
        let config: RdPowerNodeConfig = serde_json::from_value(spec)?;
        Ok(Box::new(RdPowerNode::new(config)))
    }

    fn codegen_r(&self, spec: &serde_json::Value, ctx: &mut dag_core::codegen::CodegenCtx)
        -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError>
    {
        use dag_core::codegen::helpers::*;
        let cfg = parse_spec::<RdPowerNodeConfig>(spec, RDPOWER_NODE_KIND)?;
        let input = ctx.input_vars.first().map(|s| s.as_str()).unwrap_or("__missing_input");
        let out = ctx.output_var.to_string();

        let mut args = vec![
            format!("data = cbind({input}${}, {input}${})", r_str(&cfg.y), r_str(&cfg.x)),
            format!("cutoff = {}", cfg.cutoff),
            format!("alpha = {}", cfg.alpha),
            format!("p = {}", cfg.p),
            format!("deriv = {}", cfg.deriv),
            format!("kernel = {}", r_str(&cfg.kernel)),
            format!("bwselect = {}", r_str(&cfg.bwselect)),
            format!("vce = {}", r_str(&cfg.vce)),
        ];
        if let Some(t) = cfg.tau { args.push(format!("tau = {t}")); }

        let code = vec![
            "# rdpower: RD power calculation".to_string(),
            "library(rdpower)".to_string(),
            format!("{out} <- rdpower({})", args.join(", ")),
            format!("print({out})"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }

    fn r_packages(&self) -> Vec<String> { vec!["rdpower".into()] }
}

#[async_trait]
impl DagNode for RdPowerNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new((*self).clone()) }
    fn kind(&self) -> &'static str { RDPOWER_NODE_KIND }
    fn as_any(&self) -> &dyn std::any::Any { self }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or(RdNodeError::EmptyInput)?;
        let batches: Vec<RecordBatch> = input.data.clone().collect().await
            .map_err(|e| DagError::NodeError { node_type: RDPOWER_NODE_KIND.into(), msg: format!("collect failed: {e}") })?;
        if batches.is_empty() { return Err(RdNodeError::EmptyInput.into()); }

        let cfg = &self.config;
        let y = extract_f64(&batches, &cfg.y)?;
        let r = extract_f64(&batches, &cfg.x)?;

        let rd_cfg = rdpower::RdPowerConfig {
            y, r,
            cutoff: cfg.cutoff,
            tau: cfg.tau,
            alpha: cfg.alpha,
            p: cfg.p,
            deriv: cfg.deriv,
            kernel: rdrobust::Kernel::parse(&cfg.kernel),
            bwselect: cfg.bwselect.clone(),
            vce: cfg.vce.clone(),
            level: cfg.level,
            ..Default::default()
        };

        let result = rdpower::rdpower(&rd_cfg).map_err(|e| DagError::NodeError {
            node_type: RDPOWER_NODE_KIND.into(), msg: e.to_string(),
        })?;

        let batch = build_rdpower_result(&result)?;
        let df = node_ctx.session().read_batch(batch).map_err(RdNodeError::ReadBatch)?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

fn build_rdpower_result(r: &rdpower::RdPowerResult) -> Result<RecordBatch, RdNodeError> {
    let inference = StringArray::from(vec!["Robust bias-corrected", "Conventional"]);
    let tau = Float64Array::from(vec![r.tau; 2]);
    let power = Float64Array::from(vec![Some(r.power_rbc), Some(r.power_conv)]);
    let se = Float64Array::from(vec![Some(r.se_rbc), Some(r.se_conv)]);
    let sampsi_l = Int64Array::from(vec![Some(r.sampsi_l as i64); 2]);
    let sampsi_r = Int64Array::from(vec![Some(r.sampsi_r as i64); 2]);
    let samph_l = Float64Array::from(vec![Some(r.samph_l); 2]);
    let samph_r = Float64Array::from(vec![Some(r.samph_r); 2]);
    let n_l = Int64Array::from(vec![Some(r.n_l as i64); 2]);
    let n_r = Int64Array::from(vec![Some(r.n_r as i64); 2]);
    let alpha = Float64Array::from(vec![Some(r.alpha); 2]);
    let size_dist = Float64Array::from(vec![None, Some(r.size_dist)]);

    Ok(RecordBatch::try_new(output_schema(), vec![
        Arc::new(inference), Arc::new(tau), Arc::new(power), Arc::new(se),
        Arc::new(sampsi_l), Arc::new(sampsi_r), Arc::new(samph_l), Arc::new(samph_r),
        Arc::new(n_l), Arc::new(n_r), Arc::new(alpha), Arc::new(size_dist),
    ])?)
}
