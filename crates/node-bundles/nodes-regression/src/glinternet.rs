//! glinternet DAG node — hierarchical interaction discovery via group-lasso.

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
use hierint::glinternet;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GlinternetNodeSpec {
    /// Predictor column names.
    pub predictors: Vec<String>,
    /// Outcome column name.
    pub outcome_column: String,
    /// Number of levels per predictor (1 = continuous, >1 = categorical).
    pub num_levels: Vec<usize>,
    /// Response type: "gaussian" or "binomial".
    #[serde(default = "default_family")]
    pub family: String,
    /// Number of λ values on the path.
    #[serde(default = "default_n_lambda")]
    pub n_lambda: usize,
    /// Smallest λ as fraction of λ_max.
    #[serde(default = "default_lambda_min_ratio")]
    pub lambda_min_ratio: f64,
}

fn default_family() -> String { "gaussian".into() }
fn default_n_lambda() -> usize { 50 }
fn default_lambda_min_ratio() -> f64 { 0.01 }

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_output_port(None)
        .add_output_port(None)
        .add_input_port(None)
}

pub struct GlinternetNodeFactory;
impl NodeFactory for GlinternetNodeFactory {
    fn kind(&self) -> &'static str { "glinternet" }
    fn desc(&self) -> &'static str {
        "Hierarchical interaction discovery via group-lasso (glinternet)."
    }
    fn doc(&self) -> &'static str {
        "Fits a linear pairwise-interaction model that satisfies strong hierarchy. \
        Supports categorical and continuous predictors under Gaussian or logistic loss."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(GlinternetNodeSpec).into()
    }
    fn ports(&self) -> NodePorts { port_layout() }

    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: GlinternetNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(GlinternetNode {
            spec,
            meta: port_layout(),
        }))
    }
}

#[derive(Clone)]
pub struct GlinternetNode {
    spec: GlinternetNodeSpec,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for GlinternetNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new(self.clone()) }
    fn kind(&self) -> &'static str { "glinternet" }
    fn as_any(&self) -> &dyn std::any::Any { self }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or(DagError::NodeError {
            node_type: "glinternet".into(),
            msg: "no input data".into(),
        })?;
        let batches = input.data.clone().collect().await.map_err(|e| DagError::NodeError {
            node_type: "glinternet".into(),
            msg: format!("collect failed: {e}"),
        })?;
        if batches.is_empty() {
            return Err(DagError::NodeError {
                node_type: "glinternet".into(),
                msg: "empty input".into(),
            });
        }
        let schema = batches[0].schema().clone();
        let n = batches.iter().map(|b| b.num_rows()).sum::<usize>();

        // Extract outcome
        let y = dag_core::arrow_util::extract_numeric_lenient(
            &batches, &self.spec.outcome_column,
        ).map_err(|e| DagError::NodeError {
            node_type: "glinternet".into(),
            msg: e.to_string(),
        })?;

        // Extract predictors and split into cat/cont
        let num_levels = &self.spec.num_levels;
        let mut x_cat: Vec<usize> = Vec::new();
        let mut z: Vec<f64> = Vec::new();

        for (col_idx, col_name) in self.spec.predictors.iter().enumerate() {
            let vals = dag_core::arrow_util::extract_numeric_lenient(
                &batches, col_name,
            ).map_err(|e| DagError::NodeError {
                node_type: "glinternet".into(),
                msg: e.to_string(),
            })?;
            if num_levels[col_idx] > 1 {
                for v in &vals { x_cat.push(*v as usize); }
            } else {
                z.extend_from_slice(&vals);
            }
        }

        let family = match self.spec.family.as_str() {
            "gaussian" => glinternet::Family::Gaussian,
            "binomial" => glinternet::Family::Binomial,
            other => return Err(DagError::NodeError {
                node_type: "glinternet".into(),
                msg: format!("unknown family: {other}"),
            }),
        };

        let config = glinternet::GlinternetConfig {
            family,
            n_lambda: self.spec.n_lambda,
            lambda_min_ratio: self.spec.lambda_min_ratio,
            ..Default::default()
        };

        let fit = glinternet::fit(&x_cat, &z, &y, num_levels, &config).map_err(|e| DagError::NodeError {
            node_type: "glinternet".into(),
            msg: e.to_string(),
        })?;

        // Build Port 0: results
        let port0_batch = build_results_batch(&fit, num_levels);
        let ctx = node_ctx.session();
        let df0 = ctx.read_batch(port0_batch).map_err(|e| DagError::NodeError {
            node_type: "glinternet".into(),
            msg: format!("read_batch failed: {e}"),
        })?;

        // Build Port 1: lambda path
        let port1_batch = build_lambda_batch(&fit);
        let df1 = ctx.read_batch(port1_batch).map_err(|e| DagError::NodeError {
            node_type: "glinternet".into(),
            msg: format!("read_batch failed: {e}"),
        })?;

        let mut res = PortOutputs::new();
        res.insert(0, df0);
        res.insert(1, df1);
        Ok(res)
    }
}

fn build_results_batch(fit: &glinternet::GlinternetFit, num_levels: &[usize]) -> RecordBatch {
    let cat_orig: Vec<usize> = (0..num_levels.len()).filter(|&i| num_levels[i] > 1).collect();
    let cont_orig: Vec<usize> = (0..num_levels.len()).filter(|&i| num_levels[i] == 1).collect();

    let mut types: Vec<String> = Vec::new();
    let mut var1s: Vec<i32> = Vec::new();
    let mut var2s: Vec<Option<i32>> = Vec::new();
    let mut lambda_idxs: Vec<i32> = Vec::new();
    let mut lambdas: Vec<f64> = Vec::new();

    for (li, active) in fit.active_set.iter().enumerate() {
        let lam = fit.lambda[li];
        let mut push = |t: &str, v1: usize, v2: Option<usize>| {
            types.push(t.into());
            var1s.push(v1 as i32);
            var2s.push(v2.map(|v| v as i32));
            lambda_idxs.push(li as i32);
            lambdas.push(lam);
        };
        if let Some(ref cat) = active.cat {
            for &[ci] in cat { push("main_cat", cat_orig.get(ci-1).copied().unwrap_or(ci-1), None); }
        }
        if let Some(ref cont) = active.cont {
            for &[ci] in cont { push("main_cont", cont_orig.get(ci-1).copied().unwrap_or(ci-1), None); }
        }
        if let Some(ref catcat) = active.catcat {
            for &[ci, cj] in catcat {
                push("catcat", cat_orig.get(ci-1).copied().unwrap_or(ci-1),
                    Some(cat_orig.get(cj-1).copied().unwrap_or(cj-1)));
            }
        }
        if let Some(ref cc) = active.contcont {
            for &[ci, cj] in cc {
                push("contcont", cont_orig.get(ci-1).copied().unwrap_or(ci-1),
                    Some(cont_orig.get(cj-1).copied().unwrap_or(cj-1)));
            }
        }
        if let Some(ref cct) = active.catcont {
            for &[ci, cj] in cct {
                push("catcont", cat_orig.get(ci-1).copied().unwrap_or(ci-1),
                    Some(cont_orig.get(cj-1).copied().unwrap_or(cj-1)));
            }
        }
    }

    let var2_final: Int32Array = var2s.into_iter().collect();

    let schema = Arc::new(Schema::new(vec![
        Field::new("type", DataType::Utf8, false),
        Field::new("var1", DataType::Int32, false),
        Field::new("var2", DataType::Int32, true),
        Field::new("lambda_idx", DataType::Int32, false),
        Field::new("lambda", DataType::Float64, false),
    ]));

    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(types)),
        Arc::new(Int32Array::from(var1s)),
        Arc::new(var2_final),
        Arc::new(Int32Array::from(lambda_idxs)),
        Arc::new(Float64Array::from(lambdas)),
    ]).unwrap()
}

fn build_lambda_batch(fit: &glinternet::GlinternetFit) -> RecordBatch {
    let mut lambdas = Vec::new();
    let mut objs = Vec::new();
    let mut n_mains = Vec::new();
    let mut n_inters = Vec::new();

    for (i, active) in fit.active_set.iter().enumerate() {
        let nv = active.n_vars();
        lambdas.push(fit.lambda[i]);
        objs.push(fit.obj_value[i]);
        n_mains.push((nv[0] + nv[1]) as i32);
        n_inters.push((nv[2] + nv[3] + nv[4]) as i32);
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("lambda", DataType::Float64, false),
        Field::new("obj_value", DataType::Float64, false),
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
