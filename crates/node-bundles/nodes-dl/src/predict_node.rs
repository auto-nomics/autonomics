//! `dl_predict` — universal DL inference node.

use std::sync::Arc;

use arrow_array::{Float64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

use super::common;
use dl::{
    Architecture, DLModelArtifact, DeepSurvModel, MlpModel, TransformerModel, predict_deepsurv,
    predict_mlp, predict_transformer,
};

const NODE: &str = "dl_predict";

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PredictSpec {
    /// Output type: probability, logit, risk_score, or value.
    #[serde(default = "d_output_type")]
    pub output_type: String,
    /// Batch size for inference.
    #[serde(default = "d_batch")]
    pub batch_size: usize,
}

fn d_output_type() -> String {
    "probability".into()
}
fn d_batch() -> usize {
    512
}

pub struct PredictFactory;
impl NodeFactory for PredictFactory {
    fn kind(&self) -> &'static str {
        NODE
    }
    fn desc(&self) -> &'static str {
        "Universal deep learning inference."
    }
    fn doc(&self) -> &'static str {
        "dl_predict: accepts a DLModelArtifact (port 0) and new data (port 1), produces \
        predictions. For classification models outputs probabilities; for survival models \
        outputs risk scores; for regression outputs predicted values."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(PredictSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(None) // 0: model artifact
            .add_input_port(None) // 1: prediction data
            .add_output_port(None) // 0: predictions
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: PredictSpec = serde_json::from_value(spec)?;
        Ok(Box::new(PredictNode {
            spec: s,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct PredictNode {
    spec: PredictSpec,
    meta: NodePorts,
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
        NODE
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
        // Port 0: artifact bytes.
        let artifact_batches = common::collect_port(inputs, 0, NODE).await?;
        let artifact = extract_artifact(&artifact_batches)?;

        // Port 1: prediction data.
        let data_batches = common::collect_port(inputs, 1, NODE).await?;
        let x = common::extract_tensor(&data_batches, &artifact.feature_names)?;

        let (_schema, mut fields, mut arrays) = common::concat_input(&data_batches)?;

        match artifact.architecture {
            Architecture::Mlp => {
                let mut model: MlpModel = serde_json::from_str(&artifact.checkpoint_json)
                    .map_err(|e| common::err(NODE, format!("deserialize MLP: {e}")))?;
                let preds = predict_mlp(&mut model, &x);
                match artifact.task_type {
                    dl::ArtifactTaskType::Classification => {
                        let probs: Vec<f64> = (0..preds.nrows()).map(|i| preds.at(i, 0)).collect();
                        fields.push(Arc::new(Field::new(
                            "pred_probability",
                            DataType::Float64,
                            false,
                        )));
                        arrays.push(Arc::new(Float64Array::from(probs)));
                    }
                    dl::ArtifactTaskType::Regression => {
                        let vals: Vec<f64> = (0..preds.nrows()).map(|i| preds.at(i, 0)).collect();
                        fields.push(Arc::new(Field::new("pred_value", DataType::Float64, false)));
                        arrays.push(Arc::new(Float64Array::from(vals)));
                    }
                    _ => return Err(common::err(NODE, "MLP model has unexpected task type")),
                }
            }
            Architecture::Deepsurv => {
                let mut model: DeepSurvModel = serde_json::from_str(&artifact.checkpoint_json)
                    .map_err(|e| common::err(NODE, format!("deserialize DeepSurv: {e}")))?;
                let risks = predict_deepsurv(&mut model, &x);
                let vals: Vec<f64> = (0..risks.nrows()).map(|i| risks.at(i, 0)).collect();
                fields.push(Arc::new(Field::new(
                    "pred_risk_score",
                    DataType::Float64,
                    false,
                )));
                arrays.push(Arc::new(Float64Array::from(vals)));
            }
            Architecture::Transformer => {
                let mut model: TransformerModel =
                    serde_json::from_str(&artifact.checkpoint_json)
                        .map_err(|e| common::err(NODE, format!("deserialize Transformer: {e}")))?;
                let preds = predict_transformer(&mut model, &x);
                match artifact.task_type {
                    dl::ArtifactTaskType::Classification => {
                        let probs: Vec<f64> = (0..preds.nrows()).map(|i| preds.at(i, 0)).collect();
                        fields.push(Arc::new(Field::new(
                            "pred_probability",
                            DataType::Float64,
                            false,
                        )));
                        arrays.push(Arc::new(Float64Array::from(probs)));
                    }
                    dl::ArtifactTaskType::Regression => {
                        let vals: Vec<f64> = (0..preds.nrows()).map(|i| preds.at(i, 0)).collect();
                        fields.push(Arc::new(Field::new("pred_value", DataType::Float64, false)));
                        arrays.push(Arc::new(Float64Array::from(vals)));
                    }
                    _ => {
                        return Err(common::err(
                            NODE,
                            "Transformer model has unexpected task type",
                        ));
                    }
                }
            }
            _ => {
                return Err(common::err(
                    NODE,
                    format!("unsupported architecture: {:?}", artifact.architecture),
                ));
            }
        }

        let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays)
            .map_err(|e| common::err(NODE, format!("build output batch: {e}")))?;
        common::emit_batch(ctx, batch, NODE)
    }
}

/// Extract a DLModelArtifact from a single-row batch with `artifact_bytes` column.
fn extract_artifact(batches: &[RecordBatch]) -> Result<DLModelArtifact, DagError> {
    let batch = batches
        .first()
        .ok_or(common::err(NODE, "no artifact input"))?;
    let idx = batch
        .schema()
        .index_of("artifact_bytes")
        .map_err(|_| common::err(NODE, "artifact_bytes column not found"))?;
    let col = batch.column(idx);
    let binary_col = col
        .as_any()
        .downcast_ref::<arrow_array::BinaryArray>()
        .ok_or_else(|| common::err(NODE, "artifact_bytes is not Binary"))?;
    let bytes = binary_col.value(0);
    serde_json::from_slice(bytes)
        .map_err(|e| common::err(NODE, format!("deserialize artifact: {e}")))
}
