//! `rdsampsi` DAG node: sample size calculations for RD designs.

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

pub const RDSAMPSI_NODE_KIND: &str = "rdsampsi";

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RdSampsiNodeConfig {
    pub y: String,
    pub x: String,
    #[serde(default = "default_cutoff")]
    pub cutoff: f64,
    #[serde(default)]
    pub tau: Option<f64>,
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
        Field::new("sampsi_total", DataType::Int64, true),
        Field::new("sampsi_h_l", DataType::Int64, true),
        Field::new("sampsi_h_r", DataType::Int64, true),
        Field::new("n_l", DataType::Int64, true),
        Field::new("n_r", DataType::Int64, true),
        Field::new("samph_l", DataType::Float64, true),
        Field::new("samph_r", DataType::Float64, true),
        Field::new("tau", DataType::Float64, true),
        Field::new("beta", DataType::Float64, true),
        Field::new("alpha", DataType::Float64, true),
        Field::new("nratio", DataType::Float64, true),
        Field::new("size_distortion", DataType::Float64, true),
    ]))
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(None)
        .add_output_port(Some(output_schema()))
}

#[derive(Clone)]
pub struct RdSampsiNode {
    meta: NodePorts,
    config: RdSampsiNodeConfig,
}

impl RdSampsiNode {
    pub fn new(config: RdSampsiNodeConfig) -> Self {
        Self {
            meta: port_layout(),
            config,
        }
    }
}

pub struct RdSampsiNodeFactory;

impl NodeFactory for RdSampsiNodeFactory {
    fn kind(&self) -> &'static str {
        RDSAMPSI_NODE_KIND
    }
    fn desc(&self) -> &'static str {
        "Sample size calculations for RD designs."
    }
    fn doc(&self) -> &'static str {
        "Computes the required sample size to achieve a desired power for an RD design."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(RdSampsiNodeConfig)
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let config: RdSampsiNodeConfig = serde_json::from_value(spec)?;
        Ok(Box::new(RdSampsiNode::new(config)))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let cfg = parse_spec::<RdSampsiNodeConfig>(spec, RDSAMPSI_NODE_KIND)?;
        let input = ctx
            .input_vars
            .first()
            .map(|s| s.as_str())
            .unwrap_or("__missing_input");
        let out = ctx.output_var.to_string();
        let mut args = vec![
            format!(
                "data = cbind({input}${}, {input}${})",
                r_str(&cfg.y),
                r_str(&cfg.x)
            ),
            format!("cutoff = {}", cfg.cutoff),
            format!("alpha = {}", cfg.alpha),
            format!("beta = {}", cfg.beta),
            format!("p = {}", cfg.p),
            format!("deriv = {}", cfg.deriv),
            format!("kernel = {}", r_str(&cfg.kernel)),
            format!("bwselect = {}", r_str(&cfg.bwselect)),
            format!("vce = {}", r_str(&cfg.vce)),
        ];
        if let Some(t) = cfg.tau {
            args.push(format!("tau = {t}"));
        }
        let code = vec![
            "# rdsampsi: RD sample size calculation".to_string(),
            "library(rdpower)".to_string(),
            format!("{out} <- rdsampsi({})", args.join(", ")),
            format!("print({out})"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["rdpower".into()]
    }
}

#[async_trait]
impl DagNode for RdSampsiNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        RDSAMPSI_NODE_KIND
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
                    node_type: RDSAMPSI_NODE_KIND.into(),
                    msg: format!("collect failed: {e}"),
                })?;
        if batches.is_empty() {
            return Err(RdNodeError::EmptyInput.into());
        }

        let cfg = &self.config;
        let y = extract_f64(&batches, &cfg.y)?;
        let r = extract_f64(&batches, &cfg.x)?;

        let rd_cfg = rdpower::RdSampsiConfig {
            y,
            r,
            cutoff: cfg.cutoff,
            tau: cfg.tau,
            alpha: cfg.alpha,
            beta: cfg.beta,
            p: cfg.p,
            deriv: cfg.deriv,
            kernel: rdrobust::Kernel::parse(&cfg.kernel),
            bwselect: cfg.bwselect.clone(),
            vce: cfg.vce.clone(),
            ..Default::default()
        };

        let result = rdpower::rdsampsi(&rd_cfg).map_err(|e| DagError::NodeError {
            node_type: RDSAMPSI_NODE_KIND.into(),
            msg: e.to_string(),
        })?;

        let batch = build_rdsampsi_result(&result)?;
        let df = node_ctx
            .session()
            .read_batch(batch)
            .map_err(RdNodeError::ReadBatch)?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

fn build_rdsampsi_result(r: &rdpower::RdSampsiResult) -> Result<RecordBatch, RdNodeError> {
    let inference = StringArray::from(vec!["Robust bias-corrected", "Conventional"]);
    let sampsi_total = Int64Array::from(vec![
        Some(r.sampsi_h_tot as i64),
        Some(r.sampsi_h_tot_cl as i64),
    ]);
    let sampsi_h_l = Int64Array::from(vec![
        Some(r.sampsi_h_l as i64),
        Some(r.sampsi_h_l_cl as i64),
    ]);
    let sampsi_h_r = Int64Array::from(vec![
        Some(r.sampsi_h_r as i64),
        Some(r.sampsi_h_r_cl as i64),
    ]);
    let n_l = Int64Array::from(vec![Some(r.n_l as i64); 2]);
    let n_r = Int64Array::from(vec![Some(r.n_r as i64); 2]);
    let samph_l = Float64Array::from(vec![Some(r.samph_l); 2]);
    let samph_r = Float64Array::from(vec![Some(r.samph_r); 2]);
    let tau = Float64Array::from(vec![Some(r.tau); 2]);
    let beta = Float64Array::from(vec![Some(r.beta); 2]);
    let alpha = Float64Array::from(vec![Some(r.alpha); 2]);
    let nratio = Float64Array::from(vec![Some(r.nratio), Some(r.nratio_cl)]);
    let size_dist = Float64Array::from(vec![None, Some(r.size_dist)]);

    Ok(RecordBatch::try_new(
        output_schema(),
        vec![
            Arc::new(inference),
            Arc::new(sampsi_total),
            Arc::new(sampsi_h_l),
            Arc::new(sampsi_h_r),
            Arc::new(n_l),
            Arc::new(n_r),
            Arc::new(samph_l),
            Arc::new(samph_r),
            Arc::new(tau),
            Arc::new(beta),
            Arc::new(alpha),
            Arc::new(nratio),
            Arc::new(size_dist),
        ],
    )?)
}
