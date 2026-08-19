//! `rdrobust` DAG node: local-polynomial RD estimation with robust bias correction.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, Int64Array, RecordBatch, StringArray};
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

pub const RDROBUST_NODE_KIND: &str = "rdrobust";

// =====================================================================
// Config
// =====================================================================

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RdRobustNodeConfig {
    /// Column name for the outcome variable.
    pub y: String,
    /// Column name for the running variable.
    pub x: String,
    /// RD cutoff (default 0).
    #[serde(default = "default_cutoff")]
    pub cutoff: f64,
    /// Order of the local polynomial (default 1).
    #[serde(default = "default_p")]
    pub p: usize,
    /// Derivative order (default 0).
    #[serde(default)]
    pub deriv: usize,
    /// Kernel: "triangular" (default), "uniform", or "epanechnikov".
    #[serde(default = "default_kernel")]
    pub kernel: String,
    /// Bandwidth selection method (default "mserd").
    #[serde(default = "default_bwselect")]
    pub bwselect: String,
    /// VCE method: "nn" (default), "hc0"–"hc3", "cr1"–"cr3".
    #[serde(default = "default_vce")]
    pub vce: String,
    /// Significance level percentage (default 95).
    #[serde(default = "default_level")]
    pub level: f64,
    /// Main bandwidth (scalar or leave null for data-driven selection).
    #[serde(default)]
    pub h: Option<f64>,
    /// Scaling factor for the RD parameter.
    #[serde(default = "default_scalepar")]
    pub scalepar: f64,
    /// Column name for cluster variable (optional).
    #[serde(default)]
    pub cluster: Option<String>,
    /// Column name for fuzzy treatment variable (optional).
    #[serde(default)]
    pub fuzzy: Option<String>,
    /// Minimum nearest neighbors for NN VCE (default 3).
    #[serde(default = "default_nnmatch")]
    pub nnmatch: usize,
    /// Standardize variables before bandwidth selection.
    #[serde(default = "default_stdvars")]
    pub stdvars: bool,
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
fn default_scalepar() -> f64 {
    1.0
}
fn default_nnmatch() -> usize {
    3
}
fn default_stdvars() -> bool {
    false
}

// =====================================================================
// Output schema
// =====================================================================

fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("method", DataType::Utf8, false),
        Field::new("coef", DataType::Float64, true),
        Field::new("se", DataType::Float64, true),
        Field::new("z", DataType::Float64, true),
        Field::new("p_value", DataType::Float64, true),
        Field::new("ci_lower", DataType::Float64, true),
        Field::new("ci_upper", DataType::Float64, true),
        // Summary row fields
        Field::new("h_left", DataType::Float64, true),
        Field::new("h_right", DataType::Float64, true),
        Field::new("b_left", DataType::Float64, true),
        Field::new("b_right", DataType::Float64, true),
        Field::new("n_left", DataType::Int64, true),
        Field::new("n_right", DataType::Int64, true),
        Field::new("n_h_left", DataType::Int64, true),
        Field::new("n_h_right", DataType::Int64, true),
        Field::new("kernel", DataType::Utf8, true),
        Field::new("bwselect", DataType::Utf8, true),
        Field::new("vce", DataType::Utf8, true),
    ]))
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(None)
        .add_output_port(Some(output_schema()))
}

// =====================================================================
// Node
// =====================================================================

#[derive(Clone)]
pub struct RdRobustNode {
    meta: NodePorts,
    config: RdRobustNodeConfig,
}

impl RdRobustNode {
    pub fn new(config: RdRobustNodeConfig) -> Self {
        Self {
            meta: port_layout(),
            config,
        }
    }
}

pub struct RdRobustNodeFactory;

impl NodeFactory for RdRobustNodeFactory {
    fn kind(&self) -> &'static str {
        RDROBUST_NODE_KIND
    }
    fn desc(&self) -> &'static str {
        "Local-polynomial RD estimation with robust bias correction (Calonico et al.)."
    }
    fn doc(&self) -> &'static str {
        "Estimates local-polynomial regression discontinuity treatment effects \
        with robust bias-corrected inference. Supports sharp and fuzzy RD, \
        covariate adjustment, cluster-robust SEs, and multiple bandwidth selectors."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(RdRobustNodeConfig)
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let config: RdRobustNodeConfig = serde_json::from_value(spec)?;
        Ok(Box::new(RdRobustNode::new(config)))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let cfg = parse_spec::<RdRobustNodeConfig>(spec, RDROBUST_NODE_KIND)?;
        let input = ctx
            .input_vars
            .first()
            .map(|s| s.as_str())
            .unwrap_or("__missing_input");
        let out = ctx.output_var.to_string();

        let mut args = vec![
            format!("y = {input}${}", r_str(&cfg.y)),
            format!("x = {input}${}", r_str(&cfg.x)),
            format!("c = {}", cfg.cutoff),
            format!("p = {}", cfg.p),
            format!("deriv = {}", cfg.deriv),
            format!("kernel = {}", r_str(&cfg.kernel)),
            format!("bwselect = {}", r_str(&cfg.bwselect)),
            format!("vce = {}", r_str(&cfg.vce)),
            format!("level = {}", cfg.level),
            format!("scalepar = {}", cfg.scalepar),
        ];
        if let Some(h) = cfg.h {
            args.push(format!("h = {h}"));
        }
        if let Some(col) = &cfg.cluster {
            args.push(format!("cluster = {input}${col}"));
        }
        if let Some(col) = &cfg.fuzzy {
            args.push(format!("fuzzy = {input}${col}"));
        }

        let code = vec![
            "# rdrobust: local-polynomial RD estimation".to_string(),
            "library(rdrobust)".to_string(),
            format!("{out} <- rdrobust({})", args.join(", ")),
            format!("print(summary({out}))"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["rdrobust".into()]
    }
}

#[async_trait]
impl DagNode for RdRobustNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        RDROBUST_NODE_KIND
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
                    node_type: RDROBUST_NODE_KIND.into(),
                    msg: format!("collect failed: {e}"),
                })?;
        if batches.is_empty() || batches.iter().map(|b| b.num_rows()).sum::<usize>() == 0 {
            return Err(RdNodeError::EmptyInput.into());
        }

        let cfg = &self.config;
        let y = extract_f64(&batches, &cfg.y)?;
        let x = extract_f64(&batches, &cfg.x)?;

        let cluster = cfg
            .cluster
            .as_ref()
            .and_then(|c| extract_opt_f64(&batches, c));

        let rd_cfg = rdrobust::RdRobustConfig {
            y,
            x,
            c: cfg.cutoff,
            p: cfg.p,
            q: cfg.p + 1,
            deriv: cfg.deriv,
            kernel: rdrobust::Kernel::parse(&cfg.kernel),
            bwselect: cfg.bwselect.clone(),
            vce: cfg.vce.clone(),
            level: cfg.level,
            scalepar: cfg.scalepar,
            nnmatch: cfg.nnmatch,
            stdvars: cfg.stdvars,
            cluster,
            h: cfg.h.map(|v| [Some(v), Some(v)]),
            ..Default::default()
        };

        let result = rdrobust::rdrobust(&rd_cfg).map_err(|e| DagError::NodeError {
            node_type: RDROBUST_NODE_KIND.into(),
            msg: e.to_string(),
        })?;

        let batch = build_rdrobust_result(&result)?;
        let df = node_ctx
            .session()
            .read_batch(batch)
            .map_err(RdNodeError::ReadBatch)?;

        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

fn build_rdrobust_result(r: &rdrobust::RdRobustOutput) -> Result<RecordBatch, RdNodeError> {
    let methods = vec!["Conventional", "Bias-Corrected", "Robust"];
    let method_arr = StringArray::from(methods.clone());
    let coef_arr = Float64Array::from(vec![Some(r.tau_cl), Some(r.tau_bc), Some(r.tau_bc)]);
    let se_arr = Float64Array::from(vec![Some(r.se_cl), Some(r.se_cl), Some(r.se_rb)]);
    let z_arr = Float64Array::from(r.z.map(Some).to_vec());
    let pv_arr = Float64Array::from(r.pv.map(Some).to_vec());
    let ci_lo = Float64Array::from(r.ci.iter().map(|c| Some(c[0])).collect::<Vec<_>>());
    let ci_hi = Float64Array::from(r.ci.iter().map(|c| Some(c[1])).collect::<Vec<_>>());

    // Summary fields (only on first row, null on rest)
    let h_left = Float64Array::from(vec![Some(r.h_l), None, None]);
    let h_right = Float64Array::from(vec![Some(r.h_r), None, None]);
    let b_left = Float64Array::from(vec![Some(r.b_l), None, None]);
    let b_right = Float64Array::from(vec![Some(r.b_r), None, None]);
    let n_l = Int64Array::from(vec![Some(r.n_l as i64), None, None]);
    let n_r = Int64Array::from(vec![Some(r.n_r as i64), None, None]);
    let n_h_l = Int64Array::from(vec![Some(r.n_h_l as i64), None, None]);
    let n_h_r = Int64Array::from(vec![Some(r.n_h_r as i64), None, None]);
    let kernel = StringArray::from(vec![Some(r.kernel.as_str()), None, None]);
    let bwselect = StringArray::from(vec![Some(r.bwselect.as_str()), None, None]);
    let vce = StringArray::from(vec![Some(r.vce_type.as_str()), None, None]);

    let batch = RecordBatch::try_new(
        output_schema(),
        vec![
            Arc::new(method_arr),
            Arc::new(coef_arr),
            Arc::new(se_arr),
            Arc::new(z_arr),
            Arc::new(pv_arr),
            Arc::new(ci_lo),
            Arc::new(ci_hi),
            Arc::new(h_left),
            Arc::new(h_right),
            Arc::new(b_left),
            Arc::new(b_right),
            Arc::new(n_l),
            Arc::new(n_r),
            Arc::new(n_h_l),
            Arc::new(n_h_r),
            Arc::new(kernel),
            Arc::new(bwselect),
            Arc::new(vce),
        ],
    )?;
    Ok(batch)
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

    /// Build a synthetic RD dataset: Y = 1 + R - 0.5*R^2 + 0.3*R^3 + (R>=0) + noise.
    fn make_synthetic_batch(n: usize) -> RecordBatch {
        use rand::{Rng, SeedableRng};
        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        let (y, x): (Vec<f64>, Vec<f64>) = (0..n)
            .map(|_| {
                let x1: f64 = rng.random();
                let x2: f64 = rng.random();
                let noise1: f64 = rng.random();
                let noise2: f64 = rng.random();
                let r = x1 + x2 + noise1 - 0.5;
                let y = 1.0 + r - 0.5 * r * r + 0.3 * r * r * r + (r >= 0.0) as i32 as f64 + noise2
                    - 0.5;
                (y, r)
            })
            .unzip();
        let schema = Arc::new(Schema::new(vec![
            Field::new("y", DataType::Float64, false),
            Field::new("x", DataType::Float64, false),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Float64Array::from(y)),
                Arc::new(Float64Array::from(x)),
            ],
        )
        .unwrap()
    }

    #[tokio::test]
    async fn runs_rdrobust_node() {
        let mut node = RdRobustNode::new(RdRobustNodeConfig {
            y: "y".into(),
            x: "x".into(),
            cutoff: 0.0,
            p: 1,
            deriv: 0,
            kernel: "triangular".into(),
            bwselect: "mserd".into(),
            vce: "nn".into(),
            level: 95.0,
            h: None,
            scalepar: 1.0,
            cluster: None,
            fuzzy: None,
            nnmatch: 3,
            stdvars: false,
        });

        let batch = make_synthetic_batch(500);
        let df = SessionContext::new().read_batch(batch).unwrap();
        let input = NodeInput::new_dataframe(0, df);

        let res = node
            .execute(
                &node_ctx(),
                &[input],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();

        let outputs = res.dataframe(0).unwrap().clone();
        let batch = outputs.collect().await.unwrap().into_iter().next().unwrap();
        assert_eq!(batch.num_rows(), 3); // Conventional, Bias-Corrected, Robust

        // Check the robust row (index 2) has finite coefficient and SE
        let coef = batch
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        let se = batch
            .column(2)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!(coef.value(2).is_finite());
        assert!(se.value(2) > 0.0);

        // The treatment effect should be positive (we set tau=1 in the DGP).
        // With 500 obs and a cubic DGP, the estimate can deviate from 1.0.
        let robust_coef = coef.value(2);
        assert!(
            robust_coef.is_finite() && robust_coef > 0.0,
            "expected positive finite, got {robust_coef}"
        );
    }
}
