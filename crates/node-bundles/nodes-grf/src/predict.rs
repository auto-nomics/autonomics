//! Predict DAG node — run a trained forest on new data (or OOB).

use arrow_array::RecordBatch;
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use grf::nodes::{PredictForestOutput, PredictForestSpec};

use crate::common;

/// Serialized spec for the predict node.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PredictNodeSpec {
    /// Column names of the test-X batch to use. Empty = all Float64 columns.
    #[serde(default)]
    pub x_column_names: Vec<String>,
    /// Index (0-based) of the outcome column Y within the training batch, as
    /// packed into the grf data matrix. Only needed when the training batch
    /// carries Y (default-prediction-strategy forests). Default 0.
    #[serde(default)]
    pub train_outcome_index: usize,
    /// Request variance estimates (requires ci_group_size >= 2 at training).
    #[serde(default)]
    pub estimate_variance: bool,
    /// Run OOB prediction on the training set (port 2 / test batch ignored).
    #[serde(default)]
    pub oob: bool,
    /// Thread override. 0 = hardware default.
    #[serde(default)]
    pub num_threads: u32,
}

pub struct PredictNodeFactory;
impl NodeFactory for PredictNodeFactory {
    fn kind(&self) -> &'static str {
        "grf_predict_forest"
    }
    fn desc(&self) -> &'static str {
        "Predict with a trained forest on new data."
    }
    fn doc(&self) -> &'static str {
        "grf_predict_forest: takes a forest-exchange batch (port 0), the training data (port 1), and optional test data (port 2); emits predictions (port 0). When oob=true, runs OOB prediction and ignores port 2."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(PredictNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(None) // forest
            .add_input_port(None) // train X
            .add_input_port(None) // test X
            .add_output_port(None) // predictions
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _c: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        Ok(Box::new(PredictNode {
            spec: serde_json::from_value(spec)?,
            meta: self.ports(),
        }))
    }
}

pub struct PredictNode {
    spec: PredictNodeSpec,
    meta: NodePorts,
}
impl Clone for PredictNode {
    fn clone(&self) -> Self {
        Self {
            spec: self.spec.clone(),
            meta: self.meta.clone(),
        }
    }
}

#[async_trait]
impl DagNode for PredictNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "grf_predict_forest"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let node = self.kind();
        // Do all `.await`s up front (all values Send). The forest blob is
        // `!Send`, so it is deserialized only in the synchronous tail below —
        // it is never held across an await, keeping the future `Send`.
        let train_batches = common::collect_port(node, inputs, 1).await?;
        if train_batches.is_empty() {
            return Err(dag_err(node, "input port 1 (training data) not connected"));
        }
        let test_batches = if self.spec.oob {
            None
        } else {
            let tb = common::collect_port(node, inputs, 2).await?;
            if tb.is_empty() { None } else { Some(tb) }
        };
        let forest_batches = common::collect_port(node, inputs, 0).await?;
        let fb = forest_batches
            .first()
            .ok_or_else(|| dag_err(node, "input port 0 (forest) not connected"))?;
        let forest = common::decode_forest(node, fb)?;

        // Build the predict spec from our node spec.
        let pred_spec = PredictForestSpec {
            x_column_names: self.spec.x_column_names.clone(),
            estimate_variance: self.spec.estimate_variance,
            oob: self.spec.oob,
            num_threads: self.spec.num_threads,
        };
        let out = pred_spec
            .predict(
                &forest,
                &train_batches,
                self.spec.train_outcome_index,
                test_batches.as_deref(),
            )
            .map_err(|e| dag_err(node, &e.to_string()))?;

        let batch = predictions_to_batch(node, &out)?;
        common::emit(ctx, node, batch)
    }
}

/// Reshape flat column-major predictions (pred_length × n_samples) into a
/// RecordBatch with one Float64 column per prediction output (plus variance /
/// error columns when requested).
fn predictions_to_batch(node: &str, out: &PredictForestOutput) -> Result<RecordBatch, DagError> {
    let n = out.n_samples();
    let pred_length = out.pred_length.max(1);
    let mut fields: Vec<Field> = (0..pred_length)
        .map(|k| Field::new(format!("pred_{k}"), DataType::Float64, false))
        .collect();
    let mut cols: Vec<std::sync::Arc<dyn arrow_array::Array>> = Vec::with_capacity(pred_length);
    for k in 0..pred_length {
        let mut data = Vec::with_capacity(n);
        for i in 0..n {
            let idx = k * n + i;
            data.push(out.predictions.get(idx).copied().unwrap_or(f64::NAN));
        }
        cols.push(std::sync::Arc::new(arrow_array::Float64Array::from(data)));
    }
    if let Some(v) = &out.variance {
        fields.push(Field::new("variance", DataType::Float64, true));
        cols.push(std::sync::Arc::new(arrow_array::Float64Array::from(
            v.clone(),
        )));
    }
    if let Some(v) = &out.debiased_error {
        fields.push(Field::new("debiased_error", DataType::Float64, true));
        cols.push(std::sync::Arc::new(arrow_array::Float64Array::from(
            v.clone(),
        )));
    }
    if let Some(v) = &out.excess_error {
        fields.push(Field::new("excess_error", DataType::Float64, true));
        cols.push(std::sync::Arc::new(arrow_array::Float64Array::from(
            v.clone(),
        )));
    }
    RecordBatch::try_new(std::sync::Arc::new(Schema::new(fields)), cols)
        .map_err(|e| dag_err(node, &format!("predict batch: {e}")))
}

fn dag_err(node: &str, msg: &str) -> DagError {
    DagError::NodeError {
        node_type: node.into(),
        msg: msg.to_string(),
    }
}
