//! Phase 3 DAG nodes: `dl_autoencoder_train`, `dl_deephit_train`, `dl_rnn_train`, `dl_embed`.

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
use super::train_nodes::{build_training_log_batch, EarlyStoppingSpec, TaskTypeSpec,
    d_act, d_opt, d_lr, d_epochs, d_batch, d_seed, d_std};
use dl::*;

// ═══════════════════════════════════════════════════════════════════════
// dl_autoencoder_train
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AutoEncoderTrainSpec {
    pub features: Vec<String>,
    #[serde(default = "d_ae_kind")]
    pub task_type: String, // "autoencoder" | "vae"
    #[serde(default = "d_enc_sizes")]
    pub encoder_sizes: Vec<usize>,
    #[serde(default = "d_dec_sizes")]
    pub decoder_sizes: Vec<usize>,
    #[serde(default = "d_latent")]
    pub latent_dim: usize,
    #[serde(default = "d_act")]
    pub activation: String,
    #[serde(default)]
    pub dropout: f64,
    #[serde(default = "d_loss")]
    pub loss: String, // "mse" | "bce" | "huber"
    #[serde(default = "d_beta")]
    pub beta: f64,
    #[serde(default = "d_kl_warmup")]
    pub kl_warmup_epochs: usize,
    #[serde(default = "d_opt")]
    pub optimizer: String,
    #[serde(default = "d_lr")]
    pub learning_rate: f64,
    #[serde(default = "d_epochs")]
    pub n_epochs: usize,
    #[serde(default = "d_batch")]
    pub batch_size: usize,
    #[serde(default = "d_seed")]
    pub seed: u64,
    #[serde(default = "d_std")]
    pub standardize_features: bool,
}

fn d_ae_kind() -> String { "autoencoder".into() }
fn d_enc_sizes() -> Vec<usize> { vec![128, 64] }
fn d_dec_sizes() -> Vec<usize> { vec![64, 128] }
fn d_latent() -> usize { 16 }
fn d_loss() -> String { "mse".into() }
fn d_beta() -> f64 { 1.0 }
fn d_kl_warmup() -> usize { 10 }

impl AutoEncoderTrainSpec {
    fn to_config(&self) -> Result<AutoEncoderConfig, DagError> {
        let kind = match self.task_type.as_str() {
            "vae" => AeKind::Vae,
            _ => AeKind::Autoencoder,
        };
        let activation = Activation::from_str(&self.activation).ok_or_else(|| {
            common::err("dl_autoencoder_train", format!("unknown activation: {}", self.activation))
        })?;
        let loss = match self.loss.as_str() {
            "bce" => AeLoss::Bce,
            "huber" => AeLoss::Huber,
            _ => AeLoss::Mse,
        };
        let opt_kind = OptimizerKind::from_str(&self.optimizer).ok_or_else(|| {
            common::err("dl_autoencoder_train", format!("unknown optimizer: {}", self.optimizer))
        })?;
        Ok(AutoEncoderConfig {
            kind,
            encoder_sizes: self.encoder_sizes.clone(),
            decoder_sizes: self.decoder_sizes.clone(),
            latent_dim: self.latent_dim,
            activation,
            dropout: self.dropout,
            loss,
            beta: self.beta,
            kl_warmup_epochs: self.kl_warmup_epochs,
            train: TrainConfig {
                optimizer: OptimizerConfig { kind: opt_kind, lr: self.learning_rate, ..Default::default() },
                n_epochs: self.n_epochs,
                batch_size: self.batch_size,
                standardize: self.standardize_features,
                seed: self.seed,
                ..Default::default()
            },
        })
    }
}

pub struct AutoEncoderTrainFactory;
impl NodeFactory for AutoEncoderTrainFactory {
    fn kind(&self) -> &'static str { "dl_autoencoder_train" }
    fn desc(&self) -> &'static str { "Deep autoencoder / VAE for dimensionality reduction." }
    fn doc(&self) -> &'static str {
        "dl_autoencoder_train: trains an autoencoder or variational autoencoder for unsupervised \
        feature learning. Outputs latent representations (port 0), DLModelArtifact (port 1), \
        and training log (port 2)."
    }
    fn spec_schema(&self) -> schemars::Schema { schema_for!(AutoEncoderTrainSpec) }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(None).add_input_port(None)
            .add_output_port(None).add_output_port(None).add_output_port(None)
    }
    fn build(&self, spec: serde_json::Value, _ctx: NodeCtx) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: AutoEncoderTrainSpec = serde_json::from_value(spec)?;
        Ok(Box::new(AutoEncoderTrainNode { spec: s, meta: self.ports() }))
    }
}

#[derive(Clone)]
struct AutoEncoderTrainNode {
    spec: AutoEncoderTrainSpec,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for AutoEncoderTrainNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new(self.clone()) }
    fn kind(&self) -> &'static str { "dl_autoencoder_train" }
    fn as_any(&self) -> &dyn std::any::Any { self }

    async fn execute(&mut self, ctx: &NodeCtx, inputs: &[NodeInput], _r: &dag_core::dag::node_event::NodeReporter) -> Result<PortOutputs, DagError> {
        let train_batches = common::collect_port(inputs, 0, "dl_autoencoder_train").await?;
        let x = common::extract_tensor(&train_batches, &self.spec.features)?;
        let config = self.spec.to_config()?;
        let result = train_autoencoder(&x, None, &config).map_err(|e| common::err("dl_autoencoder_train", e))?;

        // Port 0: latent + reconstruction.
        let (_schema, mut fields, mut arrays) = common::concat_input(&train_batches)?;
        let n = result.latent.nrows();
        for d in 0..result.latent.ncols() {
            let col: Vec<f64> = (0..n).map(|i| result.latent.at(i, d)).collect();
            fields.push(Arc::new(Field::new(format!("latent_{d}"), DataType::Float64, false)));
            arrays.push(Arc::new(Float64Array::from(col)));
        }
        let pred_batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays)
            .map_err(|e| common::err("dl_autoencoder_train", format!("build batch: {e}")))?;

        // Port 1: artifact.
        let checkpoint = serde_json::to_string(&result.model).map_err(|e| common::err("dl_autoencoder_train", e.to_string()))?;
        let artifact = DLModelArtifact {
            backend: "faer".into(), architecture: Architecture::Autoencoder,
            task_type: ArtifactTaskType::Reconstruction,
            checkpoint_json: checkpoint,
            feature_names: self.spec.features.clone(),
            label_column: None, time_column: None, event_column: None, scaler_json: None,
            training_meta: TrainingMeta {
                n_epochs_run: result.training_log.len(), best_epoch: None,
                best_val_metric: None, total_params: result.model.n_params(),
            },
        };
        let artifact_bytes = serde_json::to_vec(&artifact).map_err(|e| common::err("dl_autoencoder_train", e.to_string()))?;
        let artifact_batch = make_artifact_batch(&artifact_bytes, "autoencoder")?;
        let log_batch = build_training_log_batch(&result.training_log)?;

        emit_three(ctx, pred_batch, artifact_batch, log_batch, "dl_autoencoder_train")
    }
}

// ═══════════════════════════════════════════════════════════════════════
// dl_deephit_train
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DeepHitTrainSpec {
    pub features: Vec<String>,
    pub time_column: String,
    pub event_column: String,
    #[serde(default = "d_dh_hidden")]
    pub hidden_sizes: Vec<usize>,
    #[serde(default = "d_act")]
    pub activation: String,
    #[serde(default = "d_dh_dropout")]
    pub dropout: f64,
    #[serde(default = "d_n_bins")]
    pub n_time_bins: usize,
    #[serde(default = "d_bins_method")]
    pub time_bins_method: String,
    #[serde(default = "d_n_causes")]
    pub n_causes: usize,
    #[serde(default = "d_alpha")]
    pub loss_alpha: f64,
    #[serde(default = "d_dh_beta")]
    pub loss_beta: f64,
    #[serde(default = "d_gamma")]
    pub loss_gamma: f64,
    #[serde(default = "d_opt")]
    pub optimizer: String,
    #[serde(default = "d_lr")]
    pub learning_rate: f64,
    #[serde(default = "d_epochs")]
    pub n_epochs: usize,
    #[serde(default = "d_batch")]
    pub batch_size: usize,
    #[serde(default = "d_seed")]
    pub seed: u64,
    #[serde(default = "d_std")]
    pub standardize_features: bool,
}

fn d_dh_hidden() -> Vec<usize> { vec![128, 64] }
fn d_dh_dropout() -> f64 { 0.2 }
fn d_n_bins() -> usize { 10 }
fn d_bins_method() -> String { "quantile".into() }
fn d_n_causes() -> usize { 1 }
fn d_alpha() -> f64 { 1.0 }
fn d_dh_beta() -> f64 { 0.5 }
fn d_gamma() -> f64 { 1.0 }

impl DeepHitTrainSpec {
    fn to_config(&self) -> Result<DeepHitConfig, DagError> {
        let activation = Activation::from_str(&self.activation).ok_or_else(|| {
            common::err("dl_deephit_train", format!("unknown activation: {}", self.activation))
        })?;
        let opt_kind = OptimizerKind::from_str(&self.optimizer).ok_or_else(|| {
            common::err("dl_deephit_train", format!("unknown optimizer: {}", self.optimizer))
        })?;
        Ok(DeepHitConfig {
            hidden_sizes: self.hidden_sizes.clone(),
            activation, dropout: self.dropout,
            n_time_bins: self.n_time_bins,
            time_bins_method: self.time_bins_method.clone(),
            n_causes: self.n_causes,
            loss_alpha: self.loss_alpha, loss_beta: self.loss_beta, loss_gamma: self.loss_gamma,
            train: TrainConfig {
                optimizer: OptimizerConfig { kind: opt_kind, lr: self.learning_rate, ..Default::default() },
                n_epochs: self.n_epochs, batch_size: self.batch_size,
                standardize: self.standardize_features, seed: self.seed,
                ..Default::default()
            },
        })
    }
}

pub struct DeepHitTrainFactory;
impl NodeFactory for DeepHitTrainFactory {
    fn kind(&self) -> &'static str { "dl_deephit_train" }
    fn desc(&self) -> &'static str { "DeepHit: discrete-time neural survival with competing risks." }
    fn doc(&self) -> &'static str {
        "dl_deephit_train: trains a DeepHit model for survival analysis with competing risks. \
        Directly estimates cause-specific cumulative incidence functions without the PH assumption. \
        Outputs risk scores (port 0), DLModelArtifact (port 1), training log (port 2)."
    }
    fn spec_schema(&self) -> schemars::Schema { schema_for!(DeepHitTrainSpec) }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(None).add_input_port(None)
            .add_output_port(None).add_output_port(None).add_output_port(None)
    }
    fn build(&self, spec: serde_json::Value, _ctx: NodeCtx) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: DeepHitTrainSpec = serde_json::from_value(spec)?;
        Ok(Box::new(DeepHitTrainNode { spec: s, meta: self.ports() }))
    }
}

#[derive(Clone)]
struct DeepHitTrainNode {
    spec: DeepHitTrainSpec,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for DeepHitTrainNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new(self.clone()) }
    fn kind(&self) -> &'static str { "dl_deephit_train" }
    fn as_any(&self) -> &dyn std::any::Any { self }

    async fn execute(&mut self, ctx: &NodeCtx, inputs: &[NodeInput], _r: &dag_core::dag::node_event::NodeReporter) -> Result<PortOutputs, DagError> {
        let train_batches = common::collect_port(inputs, 0, "dl_deephit_train").await?;
        let x = common::extract_tensor(&train_batches, &self.spec.features)?;
        let times = common::extract_numeric_column(&train_batches, &self.spec.time_column)?;
        let events_f = common::extract_numeric_column(&train_batches, &self.spec.event_column)?;
        let events: Vec<usize> = events_f.into_iter().map(|v| v as usize).collect();

        let config = self.spec.to_config()?;
        let result = train_deephit(&x, &times, &events, None, &config)
            .map_err(|e| common::err("dl_deephit_train", e))?;

        // Port 0: risk scores.
        let (_schema, mut fields, mut arrays) = common::concat_input(&train_batches)?;
        let n = result.risk_scores.nrows();
        let risks: Vec<f64> = (0..n).map(|i| result.risk_scores.at(i, 0)).collect();
        fields.push(Arc::new(Field::new("pred_risk_score", DataType::Float64, false)));
        arrays.push(Arc::new(Float64Array::from(risks)));
        let pred_batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays)
            .map_err(|e| common::err("dl_deephit_train", format!("build batch: {e}")))?;

        // Port 1: artifact.
        let checkpoint = serde_json::to_string(&result.model).map_err(|e| common::err("dl_deephit_train", e.to_string()))?;
        let artifact = DLModelArtifact {
            backend: "faer".into(), architecture: Architecture::Deephit,
            task_type: ArtifactTaskType::Survival,
            checkpoint_json: checkpoint,
            feature_names: self.spec.features.clone(),
            label_column: None,
            time_column: Some(self.spec.time_column.clone()),
            event_column: Some(self.spec.event_column.clone()),
            scaler_json: None,
            training_meta: TrainingMeta {
                n_epochs_run: result.training_log.len(), best_epoch: None,
                best_val_metric: None, total_params: result.model.n_params(),
            },
        };
        let artifact_bytes = serde_json::to_vec(&artifact).map_err(|e| common::err("dl_deephit_train", e.to_string()))?;
        let artifact_batch = make_artifact_batch(&artifact_bytes, "deephit")?;
        let log_batch = build_training_log_batch(&result.training_log)?;

        emit_three(ctx, pred_batch, artifact_batch, log_batch, "dl_deephit_train")
    }
}

// ═══════════════════════════════════════════════════════════════════════
// dl_rnn_train
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RnnTrainSpec {
    /// Sequence features: each Vec is one feature's values across time steps.
    pub sequence_features: Vec<Vec<String>>,
    #[serde(default)]
    pub static_features: Vec<String>,
    pub label_column: String,
    pub task_type: TaskTypeSpec,
    #[serde(default = "d_cell")]
    pub cell_type: String,
    #[serde(default = "d_rnn_hidden")]
    pub hidden_size: usize,
    #[serde(default = "d_rnn_layers")]
    pub n_layers: usize,
    #[serde(default)]
    pub bidirectional: bool,
    #[serde(default)]
    pub dropout: f64,
    #[serde(default = "d_rnn_pooling")]
    pub pooling: String,
    #[serde(default = "d_opt")]
    pub optimizer: String,
    #[serde(default = "d_lr")]
    pub learning_rate: f64,
    #[serde(default = "d_epochs")]
    pub n_epochs: usize,
    #[serde(default = "d_batch")]
    pub batch_size: usize,
    #[serde(default = "d_seed")]
    pub seed: u64,
}

fn d_cell() -> String { "lstm".into() }
fn d_rnn_hidden() -> usize { 64 }
fn d_rnn_layers() -> usize { 1 }
fn d_rnn_pooling() -> String { "last".into() }

impl RnnTrainSpec {
    fn to_config(&self) -> Result<RnnConfig, DagError> {
        let cell = match self.cell_type.as_str() {
            "gru" => CellType::Gru,
            "rnn" => CellType::Rnn,
            _ => CellType::Lstm,
        };
        let pooling = match self.pooling.as_str() {
            "mean" => SeqPooling::Mean,
            "max" => SeqPooling::Max,
            _ => SeqPooling::Last,
        };
        let task = match self.task_type {
            TaskTypeSpec::Classification => dl::TaskType::Classification,
            TaskTypeSpec::Regression => dl::TaskType::Regression,
        };
        let opt_kind = OptimizerKind::from_str(&self.optimizer).ok_or_else(|| {
            common::err("dl_rnn_train", format!("unknown optimizer: {}", self.optimizer))
        })?;
        Ok(RnnConfig {
            cell_type: cell, hidden_size: self.hidden_size, n_layers: self.n_layers,
            bidirectional: self.bidirectional, dropout: self.dropout, pooling,
            task_type: task,
            train: TrainConfig {
                optimizer: OptimizerConfig { kind: opt_kind, lr: self.learning_rate, ..Default::default() },
                n_epochs: self.n_epochs, batch_size: self.batch_size, seed: self.seed,
                ..Default::default()
            },
        })
    }
}

pub struct RnnTrainFactory;
impl NodeFactory for RnnTrainFactory {
    fn kind(&self) -> &'static str { "dl_rnn_train" }
    fn desc(&self) -> &'static str { "RNN/LSTM/GRU for longitudinal/sequence data." }
    fn doc(&self) -> &'static str {
        "dl_rnn_train: trains a recurrent neural network (LSTM/GRU/RNN) for longitudinal or \
        sequence data. Supports bidirectional processing and multiple pooling strategies. \
        Outputs predictions (port 0), DLModelArtifact (port 1), training log (port 2)."
    }
    fn spec_schema(&self) -> schemars::Schema { schema_for!(RnnTrainSpec) }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(None).add_input_port(None)
            .add_output_port(None).add_output_port(None).add_output_port(None)
    }
    fn build(&self, spec: serde_json::Value, _ctx: NodeCtx) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: RnnTrainSpec = serde_json::from_value(spec)?;
        Ok(Box::new(RnnTrainNode { spec: s, meta: self.ports() }))
    }
}

#[derive(Clone)]
struct RnnTrainNode {
    spec: RnnTrainSpec,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for RnnTrainNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new(self.clone()) }
    fn kind(&self) -> &'static str { "dl_rnn_train" }
    fn as_any(&self) -> &dyn std::any::Any { self }

    async fn execute(&mut self, ctx: &NodeCtx, inputs: &[NodeInput], _r: &dag_core::dag::node_event::NodeReporter) -> Result<PortOutputs, DagError> {
        let train_batches = common::collect_port(inputs, 0, "dl_rnn_train").await?;

        // Collect all column names: sequence features first (flattened), then static.
        let mut all_cols: Vec<String> = Vec::new();
        for feat_times in &self.spec.sequence_features {
            all_cols.extend(feat_times.iter().cloned());
        }
        all_cols.extend(self.spec.static_features.iter().cloned());

        let x = common::extract_tensor(&train_batches, &all_cols)?;
        let y = common::extract_numeric_column(&train_batches, &self.spec.label_column)?;
        let y_tensor = Tensor::from_rows(y.len(), 1, &y);

        let n_seq_features = self.spec.sequence_features.len();
        let n_static = self.spec.static_features.len();
        let seq_len = self.spec.sequence_features.first().map(|v| v.len()).unwrap_or(1);

        let config = self.spec.to_config()?;
        let result = train_rnn(&x, &y_tensor, None, &config, n_seq_features, n_static, seq_len)
            .map_err(|e| common::err("dl_rnn_train", e))?;

        // Port 0: predictions.
        let (_schema, mut fields, mut arrays) = common::concat_input(&train_batches)?;
        let n = result.predictions.nrows();
        match config.task_type {
            dl::TaskType::Classification => {
                let probs: Vec<f64> = (0..n).map(|i| result.predictions.at(i, 0)).collect();
                fields.push(Arc::new(Field::new("pred_probability", DataType::Float64, false)));
                arrays.push(Arc::new(Float64Array::from(probs)));
            }
            dl::TaskType::Regression => {
                let vals: Vec<f64> = (0..n).map(|i| result.predictions.at(i, 0)).collect();
                fields.push(Arc::new(Field::new("pred_value", DataType::Float64, false)));
                arrays.push(Arc::new(Float64Array::from(vals)));
            }
        }
        let pred_batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays)
            .map_err(|e| common::err("dl_rnn_train", format!("build batch: {e}")))?;

        // Port 1: artifact.
        let checkpoint = serde_json::to_string(&result.model).map_err(|e| common::err("dl_rnn_train", e.to_string()))?;
        let artifact = DLModelArtifact {
            backend: "faer".into(), architecture: Architecture::Rnn,
            task_type: match config.task_type {
                dl::TaskType::Classification => ArtifactTaskType::Classification,
                dl::TaskType::Regression => ArtifactTaskType::Regression,
            },
            checkpoint_json: checkpoint,
            feature_names: all_cols,
            label_column: Some(self.spec.label_column.clone()),
            time_column: None, event_column: None, scaler_json: None,
            training_meta: TrainingMeta {
                n_epochs_run: result.training_log.len(), best_epoch: None,
                best_val_metric: None, total_params: result.model.n_params(),
            },
        };
        let artifact_bytes = serde_json::to_vec(&artifact).map_err(|e| common::err("dl_rnn_train", e.to_string()))?;
        let artifact_batch = make_artifact_batch(&artifact_bytes, "rnn")?;
        let log_batch = build_training_log_batch(&result.training_log)?;

        emit_three(ctx, pred_batch, artifact_batch, log_batch, "dl_rnn_train")
    }
}

// ═══════════════════════════════════════════════════════════════════════
// dl_embed
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct EmbedSpec {
    /// Which layer's output to extract: "latent" | "pre_pooling" | "attention_weights".
    #[serde(default = "d_embed_layer")]
    pub layer: String,
    #[serde(default = "d_batch")]
    pub batch_size: usize,
}

fn d_embed_layer() -> String { "latent".into() }

pub struct EmbedFactory;
impl NodeFactory for EmbedFactory {
    fn kind(&self) -> &'static str { "dl_embed" }
    fn desc(&self) -> &'static str { "Extract intermediate representations from a DL model." }
    fn doc(&self) -> &'static str {
        "dl_embed: extracts latent representations or intermediate layer outputs from a trained \
        DL model. Useful for visualization, clustering, or downstream analysis."
    }
    fn spec_schema(&self) -> schemars::Schema { schema_for!(EmbedSpec) }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(None)  // 0: model artifact
            .add_input_port(None)  // 1: data
            .add_output_port(None) // 0: embeddings
    }
    fn build(&self, spec: serde_json::Value, _ctx: NodeCtx) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: EmbedSpec = serde_json::from_value(spec)?;
        Ok(Box::new(EmbedNode { spec: s, meta: self.ports() }))
    }
}

#[derive(Clone)]
struct EmbedNode {
    spec: EmbedSpec,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for EmbedNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new(self.clone()) }
    fn kind(&self) -> &'static str { "dl_embed" }
    fn as_any(&self) -> &dyn std::any::Any { self }

    async fn execute(&mut self, ctx: &NodeCtx, inputs: &[NodeInput], _r: &dag_core::dag::node_event::NodeReporter) -> Result<PortOutputs, DagError> {
        let artifact_batches = common::collect_port(inputs, 0, "dl_embed").await?;
        let data_batches = common::collect_port(inputs, 1, "dl_embed").await?;

        // Extract artifact.
        let batch = artifact_batches.first().ok_or(common::err("dl_embed", "no artifact"))?;
        let idx = batch.schema().index_of("artifact_bytes").map_err(|_| common::err("dl_embed", "no artifact_bytes"))?;
        let binary_col = batch.column(idx).as_any().downcast_ref::<arrow_array::BinaryArray>()
            .ok_or_else(|| common::err("dl_embed", "artifact_bytes not Binary"))?;
        let bytes = binary_col.value(0);
        let artifact: DLModelArtifact = serde_json::from_slice(bytes)
            .map_err(|e| common::err("dl_embed", format!("deserialize: {e}")))?;

        let x = common::extract_tensor(&data_batches, &artifact.feature_names)?;

        let (_schema, mut fields, mut arrays) = common::concat_input(&data_batches)?;

        match artifact.architecture {
            Architecture::Autoencoder => {
                let mut model: AutoEncoderModel = serde_json::from_str(&artifact.checkpoint_json)
                    .map_err(|e| common::err("dl_embed", format!("deserialize AE: {e}")))?;
                let latent = predict_autoencoder_latent(&mut model, &x);
                let n = latent.nrows();
                for d in 0..latent.ncols() {
                    let col: Vec<f64> = (0..n).map(|i| latent.at(i, d)).collect();
                    fields.push(Arc::new(Field::new(format!("embed_{d}"), DataType::Float64, false)));
                    arrays.push(Arc::new(Float64Array::from(col)));
                }
            }
            Architecture::Mlp => {
                let mut model: MlpModel = serde_json::from_str(&artifact.checkpoint_json)
                    .map_err(|e| common::err("dl_embed", format!("deserialize MLP: {e}")))?;
                // Extract last hidden layer output as embeddings.
                let x_scaled = model.scaler.as_ref().map(|s| s.transform(&x)).unwrap_or_else(|| x.clone());
                let n_hidden = model.layers.len().saturating_sub(1);
                if n_hidden > 0 {
                    let mut h = x_scaled;
                    for l in 0..n_hidden {
                        h = model.layers[l].forward(&h);
                        if l < model.activations.len() {
                            h = model.activations[l].forward(&h);
                        }
                    }
                    for d in 0..h.ncols() {
                        let col: Vec<f64> = (0..h.nrows()).map(|i| h.at(i, d)).collect();
                        fields.push(Arc::new(Field::new(format!("embed_{d}"), DataType::Float64, false)));
                        arrays.push(Arc::new(Float64Array::from(col)));
                    }
                }
            }
            _ => {
                // For other architectures, fall back to predict output.
                return Err(common::err("dl_embed", format!("embed not yet supported for {:?}", artifact.architecture)));
            }
        }

        let out_batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays)
            .map_err(|e| common::err("dl_embed", format!("build batch: {e}")))?;
        common::emit_batch(ctx, out_batch, "dl_embed")
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Shared helpers
// ═══════════════════════════════════════════════════════════════════════

fn make_artifact_batch(bytes: &[u8], arch: &str) -> Result<RecordBatch, DagError> {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("artifact_bytes", DataType::Binary, false),
            Field::new("architecture", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(arrow_array::BinaryArray::from(vec![bytes])),
            Arc::new(arrow_array::StringArray::from(vec![arch])),
        ],
    ).map_err(|e| DagError::NodeError { node_type: "dl".into(), msg: format!("build artifact batch: {e}") })
}

fn emit_three(
    ctx: &NodeCtx,
    b0: RecordBatch, b1: RecordBatch, b2: RecordBatch,
    node_type: &str,
) -> Result<PortOutputs, DagError> {
    let df0 = ctx.session().read_batch(b0).map_err(|e| common::err(node_type, format!("read_batch(0): {e}")))?;
    let df1 = ctx.session().read_batch(b1).map_err(|e| common::err(node_type, format!("read_batch(1): {e}")))?;
    let df2 = ctx.session().read_batch(b2).map_err(|e| common::err(node_type, format!("read_batch(2): {e}")))?;
    let mut res = PortOutputs::new();
    res.insert(0, df0);
    res.insert(1, df1);
    res.insert(2, df2);
    Ok(res)
}
