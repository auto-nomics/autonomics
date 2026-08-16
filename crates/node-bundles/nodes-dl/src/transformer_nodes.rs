//! `dl_transformer_train`, `dl_autoencoder_train` nodes.

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
use super::train_nodes::{EarlyStoppingSpec, build_training_log_batch};
use dl::{
    Architecture, ArtifactTaskType, DLModelArtifact, OptimizerConfig, OptimizerKind, Pooling,
    PositionalEncoding, SchedulerConfig, TaskType, Tensor, TrainConfig, TrainingMeta,
    TransformerConfig, predict_transformer, train_transformer,
};

// ═══════════════════════════════════════════════════════════════════════
// dl_transformer_train
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TransformerTrainSpec {
    pub features: Vec<String>,
    pub label_column: String,
    pub task_type: TaskTypeSpec,
    #[serde(default = "d_d_model")]
    pub d_model: usize,
    #[serde(default = "d_n_heads")]
    pub n_heads: usize,
    #[serde(default = "d_n_layers")]
    pub n_layers: usize,
    #[serde(default = "d_d_ff")]
    pub d_ff: usize,
    #[serde(default = "d_dropout")]
    pub dropout: f64,
    #[serde(default)]
    pub attention_dropout: f64,
    #[serde(default = "d_pooling")]
    pub pooling: String,
    #[serde(default = "d_pos_enc")]
    pub positional_encoding: String,
    // Training config.
    #[serde(default = "d_opt")]
    pub optimizer: String,
    #[serde(default = "d_lr")]
    pub learning_rate: f64,
    #[serde(default)]
    pub weight_decay: f64,
    #[serde(default = "d_epochs")]
    pub n_epochs: usize,
    #[serde(default = "d_batch")]
    pub batch_size: usize,
    #[serde(default = "d_seed")]
    pub seed: u64,
    #[serde(default)]
    pub early_stopping: Option<EarlyStoppingSpec>,
    #[serde(default = "d_std")]
    pub standardize_features: bool,
}

use super::train_nodes::{
    TaskTypeSpec, d_batch, d_d_ff, d_d_model, d_dropout, d_epochs, d_lr, d_n_heads, d_n_layers,
    d_opt, d_pooling, d_pos_enc, d_seed, d_std,
};

impl TransformerTrainSpec {
    fn to_config(&self) -> Result<TransformerConfig, DagError> {
        let opt_kind = OptimizerKind::from_str(&self.optimizer).ok_or_else(|| {
            common::err(
                "dl_transformer_train",
                format!("unknown optimizer: {}", self.optimizer),
            )
        })?;
        let pooling = match self.pooling.as_str() {
            "cls" => Pooling::Cls,
            "max" => Pooling::Max,
            _ => Pooling::Mean,
        };
        let pos_enc = match self.positional_encoding.as_str() {
            "none" => PositionalEncoding::None,
            "learnable" => PositionalEncoding::Learnable,
            _ => PositionalEncoding::Sinusoidal,
        };
        let task = match self.task_type {
            TaskTypeSpec::Classification => TaskType::Classification,
            TaskTypeSpec::Regression => TaskType::Regression,
        };

        Ok(TransformerConfig {
            d_model: self.d_model,
            n_heads: self.n_heads,
            n_layers: self.n_layers,
            d_ff: self.d_ff,
            dropout: self.dropout,
            attention_dropout: self.attention_dropout,
            pooling,
            positional_encoding: pos_enc,
            task_type: task,
            train: TrainConfig {
                optimizer: OptimizerConfig {
                    kind: opt_kind,
                    lr: self.learning_rate,
                    weight_decay: self.weight_decay,
                    ..Default::default()
                },
                scheduler: SchedulerConfig::None,
                n_epochs: self.n_epochs,
                batch_size: self.batch_size,
                gradient_clip_norm: None,
                early_stopping: self.early_stopping.as_ref().map(|es| {
                    dl::mlp::EarlyStoppingConfig {
                        metric: es.metric.clone(),
                        patience: es.patience,
                        mode: es.mode.clone(),
                    }
                }),
                standardize: self.standardize_features,
                seed: self.seed,
            },
        })
    }
}

pub struct TransformerTrainFactory;
impl NodeFactory for TransformerTrainFactory {
    fn kind(&self) -> &'static str {
        "dl_transformer_train"
    }
    fn desc(&self) -> &'static str {
        "Transformer encoder for tabular data."
    }
    fn doc(&self) -> &'static str {
        "dl_transformer_train: trains a Transformer encoder model for tabular classification/regression. \
        Each feature is projected to a d_model-dimensional token, multi-head self-attention captures \
        feature interactions.\n\
        Ports: in[0]=training data, in[1]=validation data (optional).\n\
        out[0]=training predictions, out[1]=model artifact (artifact_bytes column), \
        out[2]=training log."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(TransformerTrainSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(None)
            .add_input_port(None)
            .add_output_port(None)
            .add_output_port(None)
            .add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: TransformerTrainSpec = serde_json::from_value(spec)?;
        Ok(Box::new(TransformerTrainNode {
            spec: s,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct TransformerTrainNode {
    spec: TransformerTrainSpec,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for TransformerTrainNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "dl_transformer_train"
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
        let train_batches = common::collect_port(inputs, 0, "dl_transformer_train").await?;
        let val_batches = if inputs.iter().any(|i| i.port == 1) {
            Some(common::collect_port(inputs, 1, "dl_transformer_train").await?)
        } else {
            None
        };

        let x = common::extract_tensor(&train_batches, &self.spec.features)?;
        let y = common::extract_numeric_column(&train_batches, &self.spec.label_column)?;
        let y_tensor = Tensor::from_rows(y.len(), 1, &y);

        let val = if let Some(vb) = &val_batches {
            let xv = common::extract_tensor(vb, &self.spec.features)?;
            let yv = common::extract_numeric_column(vb, &self.spec.label_column)?;
            Some((xv, Tensor::from_rows(yv.len(), 1, &yv)))
        } else {
            None
        };

        let config = self.spec.to_config()?;
        let result = train_transformer(
            &x,
            &y_tensor,
            val.as_ref().map(|(xv, yv)| (xv, yv)),
            &config,
        )
        .map_err(|e| common::err("dl_transformer_train", e))?;

        // Port 0: training predictions.
        let (_schema, mut fields, mut arrays) = common::concat_input(&train_batches)?;
        let n_preds = result.predictions.nrows();
        match config.task_type {
            TaskType::Classification => {
                let probs: Vec<f64> = (0..n_preds).map(|i| result.predictions.at(i, 0)).collect();
                fields.push(Arc::new(Field::new(
                    "pred_probability",
                    DataType::Float64,
                    false,
                )));
                arrays.push(Arc::new(Float64Array::from(probs)));
            }
            TaskType::Regression => {
                let vals: Vec<f64> = (0..n_preds).map(|i| result.predictions.at(i, 0)).collect();
                fields.push(Arc::new(Field::new("pred_value", DataType::Float64, false)));
                arrays.push(Arc::new(Float64Array::from(vals)));
            }
        }
        let pred_batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays)
            .map_err(|e| common::err("dl_transformer_train", format!("build pred batch: {e}")))?;

        // Port 1: model artifact.
        let checkpoint_json = serde_json::to_string(&result.model)
            .map_err(|e| common::err("dl_transformer_train", format!("serialize model: {e}")))?;
        let artifact = DLModelArtifact {
            backend: "faer".into(),
            architecture: Architecture::Transformer,
            task_type: match config.task_type {
                TaskType::Classification => ArtifactTaskType::Classification,
                TaskType::Regression => ArtifactTaskType::Regression,
            },
            checkpoint_json,
            feature_names: self.spec.features.clone(),
            label_column: Some(self.spec.label_column.clone()),
            time_column: None,
            event_column: None,
            scaler_json: None,
            training_meta: {
                let (best_epoch, best_val_metric) =
                    common::best_epoch_from_log(&result.training_log);
                TrainingMeta {
                    n_epochs_run: result.training_log.len(),
                    best_epoch,
                    best_val_metric,
                    total_params: result.model.n_params(),
                }
            },
        };
        let artifact_bytes = serde_json::to_vec(&artifact)
            .map_err(|e| common::err("dl_transformer_train", format!("serialize artifact: {e}")))?;

        let artifact_batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("artifact_bytes", DataType::Binary, false),
                Field::new("architecture", DataType::Utf8, false),
            ])),
            vec![
                Arc::new(arrow_array::BinaryArray::from(vec![
                    artifact_bytes.as_slice(),
                ])),
                Arc::new(arrow_array::StringArray::from(vec!["transformer"])),
            ],
        )
        .map_err(|e| common::err("dl_transformer_train", format!("build artifact batch: {e}")))?;

        // Port 2: training log.
        let log_batch = build_training_log_batch(&result.training_log)?;

        let df0 = ctx
            .session()
            .read_batch(pred_batch)
            .map_err(|e| common::err("dl_transformer_train", format!("read_batch(0): {e}")))?;
        let df1 = ctx
            .session()
            .read_batch(artifact_batch)
            .map_err(|e| common::err("dl_transformer_train", format!("read_batch(1): {e}")))?;
        let df2 = ctx
            .session()
            .read_batch(log_batch)
            .map_err(|e| common::err("dl_transformer_train", format!("read_batch(2): {e}")))?;

        let mut res = PortOutputs::new();
        res.insert(0, df0);
        res.insert(1, df1);
        res.insert(2, df2);
        Ok(res)
    }
}
