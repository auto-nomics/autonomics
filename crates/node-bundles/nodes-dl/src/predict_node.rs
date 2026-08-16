//! `dl_predict` — universal DL inference node.

use std::sync::Arc;

use arrow_array::{Float64Array, RecordBatch, UInt32Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

use super::common;
use dl::{
    Architecture, ArtifactTaskType, DLModelArtifact, DeepHitModel, DeepSurvModel, MlpModel,
    RnnModel, TransformerModel, predict_deephit, predict_deepsurv, predict_mlp, predict_rnn,
    predict_transformer,
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
        predictions. Supports MLP, Transformer, RNN, DeepSurv, and DeepHit architectures. \
        For classification models outputs pred_probability and prediction columns; \
        for survival models outputs pred_risk_score; for regression outputs pred_value."
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
#[allow(dead_code)] // Parsed spec is retained to validate input during construction.
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
                push_classification_or_regression(
                    &preds,
                    &artifact.task_type,
                    &mut fields,
                    &mut arrays,
                    NODE,
                    "MLP",
                )?;
            }
            Architecture::Transformer => {
                let mut model: TransformerModel =
                    serde_json::from_str(&artifact.checkpoint_json)
                        .map_err(|e| common::err(NODE, format!("deserialize Transformer: {e}")))?;
                let preds = predict_transformer(&mut model, &x);
                push_classification_or_regression(
                    &preds,
                    &artifact.task_type,
                    &mut fields,
                    &mut arrays,
                    NODE,
                    "Transformer",
                )?;
            }
            Architecture::Rnn => {
                let mut model: RnnModel = serde_json::from_str(&artifact.checkpoint_json)
                    .map_err(|e| common::err(NODE, format!("deserialize RNN: {e}")))?;
                let preds = predict_rnn(&mut model, &x);
                push_classification_or_regression(
                    &preds,
                    &artifact.task_type,
                    &mut fields,
                    &mut arrays,
                    NODE,
                    "RNN",
                )?;
            }
            Architecture::Deepsurv => {
                let mut model: DeepSurvModel = serde_json::from_str(&artifact.checkpoint_json)
                    .map_err(|e| common::err(NODE, format!("deserialize DeepSurv: {e}")))?;
                let risks = predict_deepsurv(&mut model, &x);
                push_risk_scores(&risks, &mut fields, &mut arrays);
            }
            Architecture::Deephit => {
                let mut model: DeepHitModel = serde_json::from_str(&artifact.checkpoint_json)
                    .map_err(|e| common::err(NODE, format!("deserialize DeepHit: {e}")))?;
                let risks = predict_deephit(&mut model, &x);
                push_risk_scores(&risks, &mut fields, &mut arrays);
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

/// Push classification (pred_probability + prediction) or regression (pred_value) columns.
fn push_classification_or_regression(
    preds: &dl::Tensor,
    task_type: &ArtifactTaskType,
    fields: &mut Vec<Arc<Field>>,
    arrays: &mut Vec<Arc<dyn arrow_array::Array>>,
    node: &str,
    arch_name: &str,
) -> Result<(), DagError> {
    match task_type {
        ArtifactTaskType::Classification => {
            let probs: Vec<f64> = (0..preds.nrows()).map(|i| preds.at(i, 0)).collect();
            fields.push(Arc::new(Field::new(
                "pred_probability",
                DataType::Float64,
                false,
            )));
            arrays.push(Arc::new(Float64Array::from(probs.clone())));
            // Also add discrete prediction column (threshold 0.5) for downstream nodes.
            let class_preds: Vec<u32> = probs
                .iter()
                .map(|&p| if p >= 0.5 { 1 } else { 0 })
                .collect();
            fields.push(Arc::new(Field::new("prediction", DataType::UInt32, false)));
            arrays.push(Arc::new(UInt32Array::from(class_preds)));
        }
        ArtifactTaskType::Regression => {
            let vals: Vec<f64> = (0..preds.nrows()).map(|i| preds.at(i, 0)).collect();
            fields.push(Arc::new(Field::new("pred_value", DataType::Float64, false)));
            arrays.push(Arc::new(Float64Array::from(vals)));
        }
        _ => {
            return Err(common::err(
                node,
                format!("{arch_name} model has unexpected task type: {task_type:?}"),
            ));
        }
    }
    Ok(())
}

/// Push a `pred_risk_score` column from survival model predictions.
fn push_risk_scores(
    risks: &dl::Tensor,
    fields: &mut Vec<Arc<Field>>,
    arrays: &mut Vec<Arc<dyn arrow_array::Array>>,
) {
    let vals: Vec<f64> = (0..risks.nrows()).map(|i| risks.at(i, 0)).collect();
    fields.push(Arc::new(Field::new(
        "pred_risk_score",
        DataType::Float64,
        false,
    )));
    arrays.push(Arc::new(Float64Array::from(vals)));
}
