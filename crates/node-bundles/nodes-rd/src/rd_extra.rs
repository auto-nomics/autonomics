//! DAG nodes for rdmulti, rddensity, and rdlocrand.

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

// =====================================================================
// rdmc: Multi-cutoff RD
// =====================================================================

pub const RDMC_NODE_KIND: &str = "rdmc";

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RdMcNodeConfig {
    pub y: String,
    pub x: String,
    pub c: String, // cutoff variable column name
    #[serde(default = "default_cutoff")]
    pub cutoff_default: f64,
    #[serde(default = "default_p")]
    pub p: usize,
    #[serde(default = "default_kernel")]
    pub kernel: String,
    #[serde(default = "default_bwselect")]
    pub bwselect: String,
    #[serde(default = "default_vce")]
    pub vce: String,
    #[serde(default = "default_level")]
    pub level: f64,
}

fn default_cutoff() -> f64 {
    0.0
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
fn default_level() -> f64 {
    95.0
}

fn rdmc_output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("cutoff", DataType::Utf8, false),
        Field::new("tau_cl", DataType::Float64, true),
        Field::new("tau_bc", DataType::Float64, true),
        Field::new("se_rb", DataType::Float64, true),
        Field::new("pv_rb", DataType::Float64, true),
        Field::new("ci_rb_l", DataType::Float64, true),
        Field::new("ci_rb_r", DataType::Float64, true),
        Field::new("weight", DataType::Float64, true),
        Field::new("n_h_l", DataType::Int64, true),
        Field::new("n_h_r", DataType::Int64, true),
    ]))
}

fn rdmc_port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(None)
        .add_output_port(Some(rdmc_output_schema()))
}

#[derive(Clone)]
pub struct RdMcNode {
    meta: NodePorts,
    config: RdMcNodeConfig,
}
impl RdMcNode {
    pub fn new(config: RdMcNodeConfig) -> Self {
        Self {
            meta: rdmc_port_layout(),
            config,
        }
    }
}

pub struct RdMcNodeFactory;
impl NodeFactory for RdMcNodeFactory {
    fn kind(&self) -> &'static str {
        RDMC_NODE_KIND
    }
    fn desc(&self) -> &'static str {
        "Multi-cutoff RD estimation."
    }
    fn doc(&self) -> &'static str {
        "Point estimation and robust bias-corrected inference for multi-cutoff RD designs."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(RdMcNodeConfig)
    }
    fn ports(&self) -> NodePorts {
        rdmc_port_layout()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        Ok(Box::new(RdMcNode::new(serde_json::from_value(spec)?)))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let cfg = parse_spec::<RdMcNodeConfig>(spec, RDMC_NODE_KIND)?;
        let input = ctx
            .input_vars
            .first()
            .map(|s| s.as_str())
            .unwrap_or("__missing_input");
        let out = ctx.output_var.to_string();
        let code = vec![
            "# rdmc: multi-cutoff RD".to_string(),
            "library(rdmulti)".to_string(),
            format!(
                "{out} <- rdmc(Y = {input}${}, X = {input}${}, C = {input}${})",
                r_str(&cfg.y),
                r_str(&cfg.x),
                r_str(&cfg.c)
            ),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["rdmulti".into()]
    }
}

#[async_trait]
impl DagNode for RdMcNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        RDMC_NODE_KIND
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
        let input = inputs.first().ok_or(RdNodeError::EmptyInput)?;
        let batches: Vec<RecordBatch> =
            input
                .data
                .clone()
                .collect()
                .await
                .map_err(|e| DagError::NodeError {
                    node_type: RDMC_NODE_KIND.into(),
                    msg: format!("collect failed: {e}"),
                })?;
        if batches.is_empty() {
            return Err(RdNodeError::EmptyInput.into());
        }
        let cfg = &self.config;
        let y = extract_f64(&batches, &cfg.y)?;
        let x = extract_f64(&batches, &cfg.x)?;
        let c = extract_f64(&batches, &cfg.c)?;
        let result = rdmulti::rdmc(&rdmulti::RdMcConfig {
            y,
            x,
            c,
            p: cfg.p,
            kernel: rdrobust::Kernel::parse(&cfg.kernel),
            bwselect: cfg.bwselect.clone(),
            vce: cfg.vce.clone(),
            level: cfg.level,
            ..Default::default()
        })
        .map_err(|e| DagError::NodeError {
            node_type: RDMC_NODE_KIND.into(),
            msg: e.to_string(),
        })?;
        let batch = build_rdmc_result(&result)?;
        let df = node_ctx
            .session()
            .read_batch(batch)
            .map_err(RdNodeError::ReadBatch)?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

fn build_rdmc_result(r: &rdmulti::RdMcResult) -> Result<RecordBatch, RdNodeError> {
    let mut cutoffs = Vec::new();
    let mut tau_cl = Vec::new();
    let mut tau_bc = Vec::new();
    let mut se_rb = Vec::new();
    let mut pv_rb = Vec::new();
    let mut ci_l = Vec::new();
    let mut ci_r = Vec::new();
    let mut weight = Vec::new();
    let mut n_h_l = Vec::new();
    let mut n_h_r = Vec::new();
    for c in &r.cutoffs {
        cutoffs.push(format!("{:.3}", c.cutoff));
        tau_cl.push(Some(c.tau_cl));
        tau_bc.push(Some(c.tau_bc));
        se_rb.push(Some(c.se_rb));
        pv_rb.push(Some(c.pv_rb));
        ci_l.push(Some(c.ci_rb[0]));
        ci_r.push(Some(c.ci_rb[1]));
        weight.push(Some(c.weight));
        n_h_l.push(Some(c.n_h_l as i64));
        n_h_r.push(Some(c.n_h_r as i64));
    }
    // Weighted row
    cutoffs.push("Weighted".into());
    tau_cl.push(Some(r.weighted.tau_cl));
    tau_bc.push(Some(r.weighted.tau_bc));
    se_rb.push(Some(r.weighted.se_rb));
    pv_rb.push(Some(r.weighted.pv_rb));
    ci_l.push(Some(r.weighted.ci_rb[0]));
    ci_r.push(Some(r.weighted.ci_rb[1]));
    weight.push(None);
    n_h_l.push(None);
    n_h_r.push(None);
    // Pooled row
    cutoffs.push("Pooled".into());
    tau_cl.push(Some(r.pooled.tau_cl));
    tau_bc.push(Some(r.pooled.tau_bc));
    se_rb.push(Some(r.pooled.se_rb));
    pv_rb.push(Some(r.pooled.pv_rb));
    ci_l.push(Some(r.pooled.ci_rb[0]));
    ci_r.push(Some(r.pooled.ci_rb[1]));
    weight.push(None);
    n_h_l.push(Some(r.pooled.n_h_l as i64));
    n_h_r.push(Some(r.pooled.n_h_r as i64));

    Ok(RecordBatch::try_new(
        rdmc_output_schema(),
        vec![
            Arc::new(StringArray::from(cutoffs)),
            Arc::new(Float64Array::from(tau_cl)),
            Arc::new(Float64Array::from(tau_bc)),
            Arc::new(Float64Array::from(se_rb)),
            Arc::new(Float64Array::from(pv_rb)),
            Arc::new(Float64Array::from(ci_l)),
            Arc::new(Float64Array::from(ci_r)),
            Arc::new(Float64Array::from(weight)),
            Arc::new(Int64Array::from(n_h_l)),
            Arc::new(Int64Array::from(n_h_r)),
        ],
    )?)
}

// =====================================================================
// rddensity: Manipulation testing
// =====================================================================

pub const RDDENSITY_NODE_KIND: &str = "rddensity";

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RdDensityNodeConfig {
    pub x: String,
    #[serde(default = "default_cutoff")]
    pub cutoff: f64,
    #[serde(default = "default_p2")]
    pub p: usize,
    #[serde(default = "default_kernel")]
    pub kernel: String,
    #[serde(default = "default_vce_density")]
    pub vce: String,
}

fn default_p2() -> usize {
    2
}
fn default_vce_density() -> String {
    "jackknife".into()
}

fn rddensity_output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("test_type", DataType::Utf8, false),
        Field::new("t_stat", DataType::Float64, true),
        Field::new("p_value", DataType::Float64, true),
        Field::new("density_left", DataType::Float64, true),
        Field::new("density_right", DataType::Float64, true),
        Field::new("density_diff", DataType::Float64, true),
        Field::new("n", DataType::Int64, true),
        Field::new("n_left", DataType::Int64, true),
        Field::new("n_right", DataType::Int64, true),
        Field::new("h_left", DataType::Float64, true),
        Field::new("h_right", DataType::Float64, true),
    ]))
}

fn rddensity_port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(None)
        .add_output_port(Some(rddensity_output_schema()))
}

#[derive(Clone)]
pub struct RdDensityNode {
    meta: NodePorts,
    config: RdDensityNodeConfig,
}
impl RdDensityNode {
    pub fn new(config: RdDensityNodeConfig) -> Self {
        Self {
            meta: rddensity_port_layout(),
            config,
        }
    }
}

pub struct RdDensityNodeFactory;
impl NodeFactory for RdDensityNodeFactory {
    fn kind(&self) -> &'static str {
        RDDENSITY_NODE_KIND
    }
    fn desc(&self) -> &'static str {
        "Manipulation testing via density discontinuity."
    }
    fn doc(&self) -> &'static str {
        "Tests whether the density of the running variable is continuous at the cutoff using local polynomial density estimation."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(RdDensityNodeConfig)
    }
    fn ports(&self) -> NodePorts {
        rddensity_port_layout()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        Ok(Box::new(RdDensityNode::new(serde_json::from_value(spec)?)))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let cfg = parse_spec::<RdDensityNodeConfig>(spec, RDDENSITY_NODE_KIND)?;
        let input = ctx
            .input_vars
            .first()
            .map(|s| s.as_str())
            .unwrap_or("__missing_input");
        let out = ctx.output_var.to_string();
        let code = vec![
            "# rddensity: manipulation testing".to_string(),
            "library(rddensity)".to_string(),
            format!(
                "{out} <- rddensity(X = {input}${}, c = {})",
                r_str(&cfg.x),
                cfg.cutoff
            ),
            format!("print(summary({out}))"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["rddensity".into()]
    }
}

#[async_trait]
impl DagNode for RdDensityNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        RDDENSITY_NODE_KIND
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
        let input = inputs.first().ok_or(RdNodeError::EmptyInput)?;
        let batches: Vec<RecordBatch> =
            input
                .data
                .clone()
                .collect()
                .await
                .map_err(|e| DagError::NodeError {
                    node_type: RDDENSITY_NODE_KIND.into(),
                    msg: format!("collect failed: {e}"),
                })?;
        if batches.is_empty() {
            return Err(RdNodeError::EmptyInput.into());
        }
        let cfg = &self.config;
        let x = extract_f64(&batches, &cfg.x)?;
        let kernel = match cfg.kernel.to_lowercase().as_str() {
            "uniform" | "uni" => rddensity::Kernel::Uniform,
            "epanechnikov" | "epa" => rddensity::Kernel::Epanechnikov,
            _ => rddensity::Kernel::Triangular,
        };
        let vce = if cfg.vce.to_lowercase() == "plugin" {
            rddensity::Vce::Plugin
        } else {
            rddensity::Vce::Jackknife
        };
        let result = rddensity::rddensity(&rddensity::RdDensityConfig {
            x,
            c: cfg.cutoff,
            p: cfg.p,
            kernel,
            vce,
            ..Default::default()
        })
        .map_err(|e| DagError::NodeError {
            node_type: RDDENSITY_NODE_KIND.into(),
            msg: e.to_string(),
        })?;

        let test_types = vec!["Jackknife", "Binomial"];
        let t_stats = vec![Some(result.t_stat), None];
        let p_vals = vec![Some(result.p_value), result.bino_pval];
        let densities_l = vec![Some(result.hat[0]); 2];
        let densities_r = vec![Some(result.hat[1]); 2];
        let densities_d = vec![Some(result.hat[2]); 2];
        let ns = vec![Some(result.n as i64); 2];
        let nls = vec![Some(result.n_left as i64); 2];
        let nrs = vec![Some(result.n_right as i64); 2];
        let hls = vec![Some(result.h_left); 2];
        let hrs = vec![Some(result.h_right); 2];

        let batch = RecordBatch::try_new(
            rddensity_output_schema(),
            vec![
                Arc::new(StringArray::from(test_types)),
                Arc::new(Float64Array::from(t_stats)),
                Arc::new(Float64Array::from(p_vals)),
                Arc::new(Float64Array::from(densities_l)),
                Arc::new(Float64Array::from(densities_r)),
                Arc::new(Float64Array::from(densities_d)),
                Arc::new(Int64Array::from(ns)),
                Arc::new(Int64Array::from(nls)),
                Arc::new(Int64Array::from(nrs)),
                Arc::new(Float64Array::from(hls)),
                Arc::new(Float64Array::from(hrs)),
            ],
        )
        .map_err(RdNodeError::Arrow)?;
        let df = node_ctx
            .session()
            .read_batch(batch)
            .map_err(RdNodeError::ReadBatch)?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

// =====================================================================
// rdrandinf: Randomization inference
// =====================================================================

pub const RDRANDINF_NODE_KIND: &str = "rdrandinf";

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RdRandInfNodeConfig {
    pub y: String,
    pub r: String,
    #[serde(default = "default_cutoff")]
    pub cutoff: f64,
    #[serde(default)]
    pub wl: Option<f64>,
    #[serde(default)]
    pub wr: Option<f64>,
    #[serde(default = "default_statistic")]
    pub statistic: String,
    #[serde(default = "default_reps")]
    pub reps: usize,
    #[serde(default = "default_seed")]
    pub seed: u64,
}

fn default_statistic() -> String {
    "diffmeans".into()
}
fn default_reps() -> usize {
    1000
}
fn default_seed() -> u64 {
    42
}

fn rdrandinf_output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("statistic", DataType::Utf8, false),
        Field::new("obs_stat", DataType::Float64, true),
        Field::new("p_value", DataType::Float64, true),
        Field::new("asy_pvalue", DataType::Float64, true),
        Field::new("n_window", DataType::Int64, true),
        Field::new("n_treat", DataType::Int64, true),
        Field::new("n_ctrl", DataType::Int64, true),
        Field::new("wl", DataType::Float64, true),
        Field::new("wr", DataType::Float64, true),
    ]))
}

fn rdrandinf_port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(None)
        .add_output_port(Some(rdrandinf_output_schema()))
}

#[derive(Clone)]
pub struct RdRandInfNode {
    meta: NodePorts,
    config: RdRandInfNodeConfig,
}
impl RdRandInfNode {
    pub fn new(config: RdRandInfNodeConfig) -> Self {
        Self {
            meta: rdrandinf_port_layout(),
            config,
        }
    }
}

pub struct RdRandInfNodeFactory;
impl NodeFactory for RdRandInfNodeFactory {
    fn kind(&self) -> &'static str {
        RDRANDINF_NODE_KIND
    }
    fn desc(&self) -> &'static str {
        "Randomization inference for RD designs."
    }
    fn doc(&self) -> &'static str {
        "Fisherian exact p-values via permutation testing within a window around the cutoff."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(RdRandInfNodeConfig)
    }
    fn ports(&self) -> NodePorts {
        rdrandinf_port_layout()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        Ok(Box::new(RdRandInfNode::new(serde_json::from_value(spec)?)))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let cfg = parse_spec::<RdRandInfNodeConfig>(spec, RDRANDINF_NODE_KIND)?;
        let input = ctx
            .input_vars
            .first()
            .map(|s| s.as_str())
            .unwrap_or("__missing_input");
        let out = ctx.output_var.to_string();
        let code = vec![
            "# rdrandinf: randomization inference".to_string(),
            "library(rdlocrand)".to_string(),
            format!(
                "{out} <- rdrandinf(Y = {input}${}, R = {input}${}, cutoff = {})",
                r_str(&cfg.y),
                r_str(&cfg.r),
                cfg.cutoff
            ),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["rdlocrand".into()]
    }
}

#[async_trait]
impl DagNode for RdRandInfNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        RDRANDINF_NODE_KIND
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
        let input = inputs.first().ok_or(RdNodeError::EmptyInput)?;
        let batches: Vec<RecordBatch> =
            input
                .data
                .clone()
                .collect()
                .await
                .map_err(|e| DagError::NodeError {
                    node_type: RDRANDINF_NODE_KIND.into(),
                    msg: format!("collect failed: {e}"),
                })?;
        if batches.is_empty() {
            return Err(RdNodeError::EmptyInput.into());
        }
        let cfg = &self.config;
        let y = extract_f64(&batches, &cfg.y)?;
        let r = extract_f64(&batches, &cfg.r)?;
        let result = rdlocrand::rdrandinf(&rdlocrand::RdRandInfConfig {
            y,
            r,
            cutoff: cfg.cutoff,
            wl: cfg.wl.unwrap_or(f64::NEG_INFINITY),
            wr: cfg.wr.unwrap_or(f64::INFINITY),
            statistic: cfg.statistic.clone(),
            reps: cfg.reps,
            seed: cfg.seed,
            ..Default::default()
        })
        .map_err(|e| DagError::NodeError {
            node_type: RDRANDINF_NODE_KIND.into(),
            msg: e.to_string(),
        })?;

        let stats = vec![cfg.statistic.clone()];
        let obs = vec![Some(result.obs_stat)];
        let pv = vec![Some(result.p_value)];
        let asy_pv = vec![Some(result.asy_pvalue)];
        let nw = vec![Some(result.n_window as i64)];
        let nt = vec![Some(result.n_treat as i64)];
        let nc = vec![Some(result.n_ctrl as i64)];
        let wl = vec![Some(result.window.0)];
        let wr = vec![Some(result.window.1)];

        let batch = RecordBatch::try_new(
            rdrandinf_output_schema(),
            vec![
                Arc::new(StringArray::from(stats)),
                Arc::new(Float64Array::from(obs)),
                Arc::new(Float64Array::from(pv)),
                Arc::new(Float64Array::from(asy_pv)),
                Arc::new(Int64Array::from(nw)),
                Arc::new(Int64Array::from(nt)),
                Arc::new(Int64Array::from(nc)),
                Arc::new(Float64Array::from(wl)),
                Arc::new(Float64Array::from(wr)),
            ],
        )
        .map_err(RdNodeError::Arrow)?;
        let df = node_ctx
            .session()
            .read_batch(batch)
            .map_err(RdNodeError::ReadBatch)?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}
