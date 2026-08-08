//! Deep learning DAG nodes — MLP classifier, Autoencoder.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch, UInt32Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::node::{DagNode, NodeInput, NodePorts};
use super::common;
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

async fn collect_batches(inputs: &[NodeInput]) -> Result<Vec<RecordBatch>, DagError> {
    let input = inputs.first().ok_or(DagError::NodeError {
        node_type: "ml_deep".into(), msg: "no input".into()
    })?;
    input.data.clone().collect().await.map_err(|e| DagError::NodeError {
        node_type: "ml_deep".into(), msg: format!("collect: {e}"),
    })
}

fn emit_batch(ctx: &NodeCtx, batch: RecordBatch) -> Result<PortOutputs, DagError> {
    let df = ctx.session().read_batch(batch).map_err(|e| DagError::NodeError {
        node_type: "ml_deep".into(), msg: format!("read_batch: {e}"),
    })?;
    let mut res = PortOutputs::new();
    res.insert(0, df);
    Ok(res)
}

// ═══════════════════════════════════════════════════════════════════════
// MLP Classifier
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct MlpSpec {
    pub features: Vec<String>,
    pub label_column: String,
    #[serde(default = "d_hidden")]
    pub hidden_sizes: Vec<usize>,
    #[serde(default = "d_act")]
    pub activation: String,
    #[serde(default = "d_lr")]
    pub learning_rate: f64,
    #[serde(default = "d_epochs")]
    pub n_epochs: usize,
    #[serde(default = "d_seed")]
    pub seed: u64,
}
fn d_hidden() -> Vec<usize> { vec![16] }
fn d_act() -> String { "relu".into() }
fn d_lr() -> f64 { 0.01 }
fn d_epochs() -> usize { 200 }
fn d_seed() -> u64 { 42 }

pub struct MlpFactory;
impl NodeFactory for MlpFactory {
    fn kind(&self) -> &'static str { "ml_mlp" }
    fn desc(&self) -> &'static str { "Multilayer Perceptron classifier." }
    fn doc(&self) -> &'static str { "MLP: feedforward neural network for binary classification. Configurable hidden layers, activation (relu/sigmoid/tanh), learning rate, epochs." }
    fn spec_schema(&self) -> schemars::Schema { schema_for!(MlpSpec) }
    fn ports(&self) -> NodePorts { NodePorts::new().add_input_port(None).add_output_port(None) }
    fn build(&self, spec: serde_json::Value, _ctx: NodeCtx) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: MlpSpec = serde_json::from_value(spec)?;
        Ok(Box::new(MlpNode {
            features: s.features, label_column: s.label_column,
            hidden_sizes: s.hidden_sizes, activation: s.activation,
            learning_rate: s.learning_rate, n_epochs: s.n_epochs, seed: s.seed, meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct MlpNode {
    features: Vec<String>, label_column: String,
    hidden_sizes: Vec<usize>, activation: String,
    learning_rate: f64, n_epochs: usize, seed: u64, meta: NodePorts,
}

#[async_trait]
impl DagNode for MlpNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new(self.clone()) }
    fn kind(&self) -> &'static str { "ml_mlp" }
    fn as_any(&self) -> &dyn std::any::Any { self }
    async fn execute(&mut self, ctx: &NodeCtx, inputs: &[NodeInput], _r: &dag_core::dag::node_event::NodeReporter) -> Result<PortOutputs, DagError> {
        let batches = collect_batches(inputs).await?;
        let data = common::extract_matrix(&batches, &self.features).map_err(|e| DagError::NodeError { node_type: "ml_mlp".into(), msg: e.to_string() })?;
        let labels_f = common::extract_numeric_column(&batches, &self.label_column).map_err(|e| DagError::NodeError { node_type: "ml_mlp".into(), msg: e.to_string() })?;
        let labels: Vec<usize> = labels_f.into_iter().map(|v| v as usize).collect();
        let act = match self.activation.as_str() {
            "sigmoid" => ml::deep::Activation::Sigmoid,
            "tanh" => ml::deep::Activation::Tanh,
            _ => ml::deep::Activation::ReLU,
        };
        let opts = ml::deep::MlpOptions {
            hidden_sizes: self.hidden_sizes.clone(),
            activation: act,
            learning_rate: self.learning_rate,
            n_epochs: self.n_epochs,
            seed: self.seed,
        };
        let model = ml::deep::mlp_fit(&data, &labels, &opts)
            .map_err(|e| DagError::NodeError { node_type: "ml_mlp".into(), msg: e.to_string() })?;
        let preds = ml::deep::mlp_predict(&model, &data);
        let probs = ml::deep::mlp_predict_proba(&model, &data);
        let schema = batches.first().unwrap().schema();
        let mut fields: Vec<Arc<Field>> = schema.fields().iter().cloned().collect();
        let mut arrays: Vec<Arc<dyn Array>> = (0..schema.fields().len()).map(|i| batches.first().unwrap().column(i).clone()).collect();
        fields.push(Arc::new(Field::new("prediction", DataType::UInt32, false)));
        arrays.push(Arc::new(UInt32Array::from(preds.iter().map(|&p| p as u32).collect::<Vec<_>>())));
        fields.push(Arc::new(Field::new("probability", DataType::Float64, false)));
        arrays.push(Arc::new(Float64Array::from(probs)));
        emit_batch(ctx, RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| DagError::NodeError { node_type: "ml_mlp".into(), msg: e.to_string() })?)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Autoencoder
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AutoencoderSpec {
    pub features: Vec<String>,
    #[serde(default = "d_latent")]
    pub latent_dim: usize,
    #[serde(default = "d_ae_epochs")]
    pub n_epochs: usize,
    #[serde(default = "d_ae_lr")]
    pub learning_rate: f64,
    #[serde(default = "d_seed")]
    pub seed: u64,
}
fn d_latent() -> usize { 2 }
fn d_ae_epochs() -> usize { 100 }
fn d_ae_lr() -> f64 { 0.01 }

pub struct AutoencoderFactory;
impl NodeFactory for AutoencoderFactory {
    fn kind(&self) -> &'static str { "ml_autoencoder" }
    fn desc(&self) -> &'static str { "Linear autoencoder for dimensionality reduction." }
    fn doc(&self) -> &'static str { "Autoencoder: learns a compressed latent representation by training encoder-decoder weight matrices to reconstruct input. Outputs ae_0, ae_1, ... latent coordinates." }
    fn spec_schema(&self) -> schemars::Schema { schema_for!(AutoencoderSpec) }
    fn ports(&self) -> NodePorts { NodePorts::new().add_input_port(None).add_output_port(None) }
    fn build(&self, spec: serde_json::Value, _ctx: NodeCtx) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: AutoencoderSpec = serde_json::from_value(spec)?;
        Ok(Box::new(AutoencoderNode {
            features: s.features, latent_dim: s.latent_dim,
            n_epochs: s.n_epochs, learning_rate: s.learning_rate, seed: s.seed, meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct AutoencoderNode {
    features: Vec<String>, latent_dim: usize,
    n_epochs: usize, learning_rate: f64, seed: u64, meta: NodePorts,
}

#[async_trait]
impl DagNode for AutoencoderNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new(self.clone()) }
    fn kind(&self) -> &'static str { "ml_autoencoder" }
    fn as_any(&self) -> &dyn std::any::Any { self }
    async fn execute(&mut self, ctx: &NodeCtx, inputs: &[NodeInput], _r: &dag_core::dag::node_event::NodeReporter) -> Result<PortOutputs, DagError> {
        let batches = collect_batches(inputs).await?;
        let data = common::extract_matrix(&batches, &self.features).map_err(|e| DagError::NodeError { node_type: "ml_autoencoder".into(), msg: e.to_string() })?;
        let result = ml::deep::autoencoder(&data, self.latent_dim, self.n_epochs, self.learning_rate, self.seed)
            .map_err(|e| DagError::NodeError { node_type: "ml_autoencoder".into(), msg: e.to_string() })?;

        let schema = batches.first().unwrap().schema();
        let mut fields: Vec<Arc<Field>> = schema.fields().iter().cloned().collect();
        let mut arrays: Vec<Arc<dyn Array>> = (0..schema.fields().len()).map(|i| batches.first().unwrap().column(i).clone()).collect();
        for d in 0..self.latent_dim {
            let col: Vec<f64> = result.encoded.iter().map(|row| row[d]).collect();
            fields.push(Arc::new(Field::new(format!("ae_{d}"), DataType::Float64, true)));
            arrays.push(Arc::new(Float64Array::from(col)));
        }
        // Add reconstruction error as a constant column
        let err_col = vec![result.reconstruction_error; result.encoded.len()];
        fields.push(Arc::new(Field::new("reconstruction_error", DataType::Float64, true)));
        arrays.push(Arc::new(Float64Array::from(err_col)));

        emit_batch(ctx, RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| DagError::NodeError { node_type: "ml_autoencoder".into(), msg: e.to_string() })?)
    }
}
