//! hierNet DAG node — hierarchical interaction discovery via L1 + ADMM.

use std::sync::Arc;

use arrow_array::{Float64Array, Int32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
};
use hierint::hiernet;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct HierNetNodeSpec {
    /// Predictor column names (must be continuous).
    pub predictors: Vec<String>,
    /// Outcome column name.
    pub outcome_column: String,
    /// Response type: "gaussian" or "logistic".
    #[serde(default = "default_family")]
    pub family: String,
    /// Enforce strong hierarchy via ADMM.
    #[serde(default)]
    pub strong: bool,
    /// Include diagonal quadratic terms.
    #[serde(default = "default_true")]
    pub diagonal: bool,
    /// Number of lambda values on path.
    #[serde(default = "default_n_lam")]
    pub n_lam: usize,
    /// Smallest lambda as fraction of max.
    #[serde(default = "default_flmin")]
    pub flmin: f64,
}

fn default_family() -> String { "gaussian".into() }
fn default_true() -> bool { true }
fn default_n_lam() -> usize { 20 }
fn default_flmin() -> f64 { 0.01 }

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_output_port(None)
        .add_output_port(None)
        .add_input_port(None)
}

pub struct HierNetNodeFactory;
impl NodeFactory for HierNetNodeFactory {
    fn kind(&self) -> &'static str { "hiernet" }
    fn desc(&self) -> &'static str {
        "Hierarchical interaction discovery via L1-penalized regression (hierNet)."
    }
    fn doc(&self) -> &'static str {
        "Fits an L1-penalized model with hierarchy constraints on interactions. \
        Supports weak and strong hierarchy, Gaussian and logistic loss. Continuous predictors only."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(HierNetNodeSpec).into()
    }
    fn ports(&self) -> NodePorts { port_layout() }

    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: HierNetNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(HierNetNode { spec, meta: port_layout() }))
    }
}

#[derive(Clone)]
pub struct HierNetNode {
    spec: HierNetNodeSpec,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for HierNetNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new(self.clone()) }
    fn kind(&self) -> &'static str { "hiernet" }
    fn as_any(&self) -> &dyn std::any::Any { self }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or(DagError::NodeError {
            node_type: "hiernet".into(),
            msg: "no input data".into(),
        })?;
        let batches = input.data.clone().collect().await.map_err(|e| DagError::NodeError {
            node_type: "hiernet".into(),
            msg: format!("collect failed: {e}"),
        })?;
        if batches.is_empty() {
            return Err(DagError::NodeError {
                node_type: "hiernet".into(),
                msg: "empty input".into(),
            });
        }
        let schema = batches[0].schema().clone();
        let n = batches.iter().map(|b| b.num_rows()).sum::<usize>();

        let y = dag_core::arrow_util::extract_numeric_lenient(
            &batches, &self.spec.outcome_column,
        ).map_err(|e| DagError::NodeError {
            node_type: "hiernet".into(),
            msg: e.to_string(),
        })?;

        // Build column-major X matrix
        let p = self.spec.predictors.len();
        let mut x = vec![0.0; p * n];
        for (j, col_name) in self.spec.predictors.iter().enumerate() {
            let vals = dag_core::arrow_util::extract_numeric_lenient(
                &batches, col_name,
            ).map_err(|e| DagError::NodeError {
                node_type: "hiernet".into(),
                msg: e.to_string(),
            })?;
            for i in 0..n { x[j * n + i] = vals[i]; }
        }

        let family = match self.spec.family.as_str() {
            "gaussian" => hiernet::HierNetFamily::Gaussian,
            "logistic" => hiernet::HierNetFamily::Logistic,
            other => return Err(DagError::NodeError {
                node_type: "hiernet".into(),
                msg: format!("unknown family: {other}"),
            }),
        };

        let config = hiernet::HierNetConfig {
            family,
            strong: self.spec.strong,
            diagonal: self.spec.diagonal,
            n_lam: self.spec.n_lam,
            flmin: self.spec.flmin,
            ..Default::default()
        };

        let path = hiernet::fit_path(&x, &y, &config).map_err(|e| DagError::NodeError {
            node_type: "hiernet".into(),
            msg: e.to_string(),
        })?;

        // Build Port 0: coefficients
        let port0_batch = build_coefs_batch(&path, p);
        let ctx = node_ctx.session();
        let df0 = ctx.read_batch(port0_batch).map_err(|e| DagError::NodeError {
            node_type: "hiernet".into(),
            msg: format!("read_batch failed: {e}"),
        })?;

        // Build Port 1: lambda path
        let port1_batch = build_lambda_batch(&path, p);
        let df1 = ctx.read_batch(port1_batch).map_err(|e| DagError::NodeError {
            node_type: "hiernet".into(),
            msg: format!("read_batch failed: {e}"),
        })?;

        let mut res = PortOutputs::new();
        res.insert(0, df0);
        res.insert(1, df1);
        Ok(res)
    }
}

fn build_coefs_batch(path: &hiernet::HierNetPath, p: usize) -> RecordBatch {
    let mut types: Vec<String> = Vec::new();
    let mut var1s: Vec<i32> = Vec::new();
    let mut var2s: Vec<Option<i32>> = Vec::new();
    let mut coefs: Vec<f64> = Vec::new();
    let mut lam_idxs: Vec<i32> = Vec::new();

    for (li, fit) in path.fits.iter().enumerate() {
        for j in 0..p {
            let b = fit.coefs.bp[j] - fit.coefs.bn[j];
            if b.abs() > 1e-8 {
                types.push("main".into());
                var1s.push(j as i32);
                var2s.push(None);
                coefs.push(b);
                lam_idxs.push(li as i32);
            }
        }
        for j in 0..p - 1 {
            for k in j + 1..p {
                let th = (fit.coefs.th[j + p * k] + fit.coefs.th[k + p * j]) / 2.0;
                if th.abs() > 1e-8 {
                    types.push("interaction".into());
                    var1s.push(j as i32);
                    var2s.push(Some(k as i32));
                    coefs.push(th);
                    lam_idxs.push(li as i32);
                }
            }
        }
    }

    let var2_final: Int32Array = var2s.into_iter().collect();

    let schema = Arc::new(Schema::new(vec![
        Field::new("type", DataType::Utf8, false),
        Field::new("var1", DataType::Int32, false),
        Field::new("var2", DataType::Int32, true),
        Field::new("coefficient", DataType::Float64, false),
        Field::new("lambda_idx", DataType::Int32, false),
    ]));

    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(types)),
        Arc::new(Int32Array::from(var1s)),
        Arc::new(var2_final),
        Arc::new(Float64Array::from(coefs)),
        Arc::new(Int32Array::from(lam_idxs)),
    ]).unwrap()
}

fn build_lambda_batch(path: &hiernet::HierNetPath, p: usize) -> RecordBatch {
    let mut lambdas = Vec::new();
    let mut objs = Vec::new();
    let mut n_mains = Vec::new();
    let mut n_inters = Vec::new();

    for fit in &path.fits {
        lambdas.push(fit.lam);
        objs.push(fit.obj);
        let main = fit.coefs.bp.iter().zip(&fit.coefs.bn)
            .filter(|(bp, bn)| (*bp - *bn).abs() > 1e-6).count();
        n_mains.push(main as i32);
        let mut inter = 0;
        for j in 0..p - 1 {
            for k in j + 1..p {
                if (fit.coefs.th[j + p * k] + fit.coefs.th[k + p * j]).abs() > 1e-6 { inter += 1; }
            }
        }
        n_inters.push(inter as i32);
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("lambda", DataType::Float64, false),
        Field::new("obj", DataType::Float64, false),
        Field::new("n_main", DataType::Int32, false),
        Field::new("n_interactions", DataType::Int32, false),
    ]));

    RecordBatch::try_new(schema, vec![
        Arc::new(Float64Array::from(lambdas)),
        Arc::new(Float64Array::from(objs)),
        Arc::new(Int32Array::from(n_mains)),
        Arc::new(Int32Array::from(n_inters)),
    ]).unwrap()
}
