//! DL training nodes — `dl_mlp_train`, `dl_deepsurv_train`.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch, UInt32Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

use super::common;
use dl::{
    Activation, Architecture, ArtifactTaskType, DLModelArtifact, DeepSurvConfig, MlpConfig,
    OptimizerConfig, OptimizerKind, SchedulerConfig, TaskType, Tensor, TrainConfig, TrainingMeta,
    predict_deepsurv, predict_mlp, train_deepsurv, train_mlp,
};

// ═══════════════════════════════════════════════════════════════════════
// dl_mlp_train
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct MlpTrainSpec {
    pub features: Vec<String>,
    pub label_column: String,
    pub task_type: TaskTypeSpec,
    #[serde(default = "d_hidden")]
    pub hidden_sizes: Vec<usize>,
    #[serde(default = "d_act")]
    pub activation: String,
    #[serde(default)]
    pub dropout: f64,
    #[serde(default)]
    pub batch_norm: bool,
    #[serde(default = "d_opt")]
    pub optimizer: String,
    #[serde(default = "d_lr")]
    pub learning_rate: f64,
    #[serde(default)]
    pub weight_decay: f64,
    #[serde(default)]
    pub lr_scheduler: Option<String>,
    #[serde(default = "d_epochs")]
    pub n_epochs: usize,
    #[serde(default = "d_batch")]
    pub batch_size: usize,
    #[serde(default = "d_seed")]
    pub seed: u64,
    #[serde(default)]
    pub gradient_clip_norm: Option<f64>,
    #[serde(default)]
    pub early_stopping: Option<EarlyStoppingSpec>,
    #[serde(default = "d_std")]
    pub standardize_features: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TaskTypeSpec {
    Classification,
    Regression,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct EarlyStoppingSpec {
    pub metric: String,
    pub patience: usize,
    pub mode: String,
}

pub fn d_hidden() -> Vec<usize> {
    vec![128, 64]
}
pub fn d_act() -> String {
    "relu".into()
}
pub fn d_opt() -> String {
    "adam".into()
}
pub fn d_lr() -> f64 {
    0.001
}
pub fn d_epochs() -> usize {
    100
}
pub fn d_batch() -> usize {
    32
}
pub fn d_seed() -> u64 {
    42
}
pub fn d_std() -> bool {
    true
}
pub fn d_d_model() -> usize {
    64
}
pub fn d_n_heads() -> usize {
    4
}
pub fn d_n_layers() -> usize {
    2
}
pub fn d_d_ff() -> usize {
    256
}
pub fn d_dropout() -> f64 {
    0.1
}
pub fn d_pooling() -> String {
    "mean".into()
}
pub fn d_pos_enc() -> String {
    "sinusoidal".into()
}

impl MlpTrainSpec {
    fn to_config(&self) -> Result<MlpConfig, DagError> {
        let activation = Activation::from_str(&self.activation).ok_or_else(|| {
            common::err(
                "dl_mlp_train",
                format!("unknown activation: {}", self.activation),
            )
        })?;
        let opt_kind = OptimizerKind::from_str(&self.optimizer).ok_or_else(|| {
            common::err(
                "dl_mlp_train",
                format!("unknown optimizer: {}", self.optimizer),
            )
        })?;
        let task = match self.task_type {
            TaskTypeSpec::Classification => TaskType::Classification,
            TaskTypeSpec::Regression => TaskType::Regression,
        };
        let scheduler = match &self.lr_scheduler {
            None => SchedulerConfig::None,
            Some(s) if s == "cosine" => SchedulerConfig::Cosine {
                max_epochs: self.n_epochs,
            },
            Some(s) if s == "step" => SchedulerConfig::Step {
                step_size: 30,
                gamma: 0.5,
            },
            _ => SchedulerConfig::None,
        };

        Ok(MlpConfig {
            hidden_sizes: self.hidden_sizes.clone(),
            activation,
            dropout: self.dropout,
            batch_norm: self.batch_norm,
            task_type: task,
            train: TrainConfig {
                optimizer: OptimizerConfig {
                    kind: opt_kind,
                    lr: self.learning_rate,
                    weight_decay: self.weight_decay,
                    ..Default::default()
                },
                scheduler,
                n_epochs: self.n_epochs,
                batch_size: self.batch_size,
                gradient_clip_norm: self.gradient_clip_norm,
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

pub struct MlpTrainFactory;
impl NodeFactory for MlpTrainFactory {
    fn kind(&self) -> &'static str {
        "dl_mlp_train"
    }
    fn desc(&self) -> &'static str {
        "Enhanced MLP for tabular classification/regression."
    }
    fn doc(&self) -> &'static str {
        "dl_mlp_train: trains a multilayer perceptron with configurable hidden layers, \
        activation, dropout, batch normalization, learning-rate scheduling, gradient clipping, \
        and early stopping on a validation set. Outputs training predictions (port 0), a \
        DLModelArtifact (port 1), and a per-epoch training log (port 2)."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MlpTrainSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(None) // 0: training data
            .add_input_port(None) // 1: validation data (optional)
            .add_output_port(None) // 0: training predictions
            .add_output_port(None) // 1: model artifact
            .add_output_port(None) // 2: training log
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: MlpTrainSpec = serde_json::from_value(spec)?;
        Ok(Box::new(MlpTrainNode {
            spec: s,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct MlpTrainNode {
    spec: MlpTrainSpec,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for MlpTrainNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "dl_mlp_train"
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
        let train_batches = common::collect_port(inputs, 0, "dl_mlp_train").await?;
        let val_batches = if inputs.iter().any(|i| i.port == 1) {
            Some(common::collect_port(inputs, 1, "dl_mlp_train").await?)
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
        let result = train_mlp(
            &x,
            &y_tensor,
            val.as_ref().map(|(xv, yv)| (xv, yv)),
            &config,
        )
        .map_err(|e| common::err("dl_mlp_train", e))?;

        // Port 0: training predictions.
        let (_schema, mut fields, mut arrays) = common::concat_input(&train_batches)?;
        let n_preds = result.predictions.nrows();
        match config.task_type {
            TaskType::Classification => {
                fields.push(Arc::new(Field::new(
                    "pred_probability",
                    DataType::Float64,
                    false,
                )));
                let probs: Vec<f64> = (0..n_preds).map(|i| result.predictions.at(i, 0)).collect();
                arrays.push(Arc::new(Float64Array::from(probs.clone())));
                let preds: Vec<u32> = probs.iter().map(|&p| if p > 0.5 { 1 } else { 0 }).collect();
                fields.push(Arc::new(Field::new("prediction", DataType::UInt32, false)));
                arrays.push(Arc::new(UInt32Array::from(preds)));
            }
            TaskType::Regression => {
                fields.push(Arc::new(Field::new("pred_value", DataType::Float64, false)));
                let vals: Vec<f64> = (0..n_preds).map(|i| result.predictions.at(i, 0)).collect();
                arrays.push(Arc::new(Float64Array::from(vals)));
            }
        }
        let pred_batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays)
            .map_err(|e| common::err("dl_mlp_train", format!("build pred batch: {e}")))?;

        // Port 1: model artifact.
        let checkpoint_json = serde_json::to_string(&result.model)
            .map_err(|e| common::err("dl_mlp_train", format!("serialize model: {e}")))?;
        let artifact = DLModelArtifact {
            backend: "faer".into(),
            architecture: Architecture::Mlp,
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
            training_meta: TrainingMeta {
                n_epochs_run: result.training_log.len(),
                best_epoch: result
                    .training_log
                    .iter()
                    .filter(|l| l.val_metric.is_some())
                    .last()
                    .map(|l| l.epoch),
                best_val_metric: result
                    .training_log
                    .iter()
                    .filter(|l| l.val_metric.is_some())
                    .last()
                    .and_then(|l| l.val_metric),
                total_params: result.model.n_params(),
            },
        };
        let artifact_bytes = serde_json::to_vec(&artifact)
            .map_err(|e| common::err("dl_mlp_train", format!("serialize artifact: {e}")))?;

        let artifact_batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("artifact_bytes", DataType::Binary, false),
                Field::new("architecture", DataType::Utf8, false),
            ])),
            vec![
                Arc::new(arrow_array::BinaryArray::from(vec![
                    artifact_bytes.as_slice(),
                ])),
                Arc::new(arrow_array::StringArray::from(vec!["mlp"])),
            ],
        )
        .map_err(|e| common::err("dl_mlp_train", format!("build artifact batch: {e}")))?;

        // Port 2: training log.
        let log_batch = build_training_log_batch(&result.training_log)?;

        let df0 = ctx
            .session()
            .read_batch(pred_batch)
            .map_err(|e| common::err("dl_mlp_train", format!("read_batch(0): {e}")))?;
        let df1 = ctx
            .session()
            .read_batch(artifact_batch)
            .map_err(|e| common::err("dl_mlp_train", format!("read_batch(1): {e}")))?;
        let df2 = ctx
            .session()
            .read_batch(log_batch)
            .map_err(|e| common::err("dl_mlp_train", format!("read_batch(2): {e}")))?;

        let mut res = PortOutputs::new();
        res.insert(0, df0);
        res.insert(1, df1);
        res.insert(2, df2);
        Ok(res)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// dl_deepsurv_train
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DeepSurvTrainSpec {
    pub features: Vec<String>,
    pub time_column: String,
    pub event_column: String,
    #[serde(default = "d_ds_hidden")]
    pub hidden_sizes: Vec<usize>,
    #[serde(default = "d_act")]
    pub activation: String,
    #[serde(default = "d_ds_dropout")]
    pub dropout: f64,
    #[serde(default = "d_opt")]
    pub optimizer: String,
    #[serde(default = "d_lr")]
    pub learning_rate: f64,
    #[serde(default)]
    pub weight_decay: f64,
    #[serde(default)]
    pub lr_scheduler: Option<String>,
    #[serde(default = "d_epochs")]
    pub n_epochs: usize,
    #[serde(default = "d_batch")]
    pub batch_size: usize,
    #[serde(default = "d_seed")]
    pub seed: u64,
    #[serde(default)]
    pub gradient_clip_norm: Option<f64>,
    #[serde(default)]
    pub early_stopping: Option<EarlyStoppingSpec>,
    #[serde(default = "d_std")]
    pub standardize_features: bool,
}

fn d_ds_hidden() -> Vec<usize> {
    vec![64, 32]
}
fn d_ds_dropout() -> f64 {
    0.1
}

impl DeepSurvTrainSpec {
    fn to_config(&self) -> Result<DeepSurvConfig, DagError> {
        let activation = Activation::from_str(&self.activation).ok_or_else(|| {
            common::err(
                "dl_deepsurv_train",
                format!("unknown activation: {}", self.activation),
            )
        })?;
        let opt_kind = OptimizerKind::from_str(&self.optimizer).ok_or_else(|| {
            common::err(
                "dl_deepsurv_train",
                format!("unknown optimizer: {}", self.optimizer),
            )
        })?;
        let scheduler = match &self.lr_scheduler {
            None => SchedulerConfig::None,
            Some(s) if s == "cosine" => SchedulerConfig::Cosine {
                max_epochs: self.n_epochs,
            },
            _ => SchedulerConfig::None,
        };

        Ok(DeepSurvConfig {
            hidden_sizes: self.hidden_sizes.clone(),
            activation,
            dropout: self.dropout,
            train: TrainConfig {
                optimizer: OptimizerConfig {
                    kind: opt_kind,
                    lr: self.learning_rate,
                    weight_decay: self.weight_decay,
                    ..Default::default()
                },
                scheduler,
                n_epochs: self.n_epochs,
                batch_size: self.batch_size,
                gradient_clip_norm: self.gradient_clip_norm,
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

pub struct DeepSurvTrainFactory;
impl NodeFactory for DeepSurvTrainFactory {
    fn kind(&self) -> &'static str {
        "dl_deepsurv_train"
    }
    fn desc(&self) -> &'static str {
        "DeepSurv: neural network Cox model."
    }
    fn doc(&self) -> &'static str {
        "dl_deepsurv_train: trains a neural network with Cox proportional hazards partial \
        likelihood loss. Maintains the proportional hazards assumption while relaxing the \
        linearity constraint of standard Cox regression. Outputs risk scores (port 0), \
        DLModelArtifact (port 1), and training log (port 2)."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(DeepSurvTrainSpec)
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
        let s: DeepSurvTrainSpec = serde_json::from_value(spec)?;
        Ok(Box::new(DeepSurvTrainNode {
            spec: s,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct DeepSurvTrainNode {
    spec: DeepSurvTrainSpec,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for DeepSurvTrainNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "dl_deepsurv_train"
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
        let train_batches = common::collect_port(inputs, 0, "dl_deepsurv_train").await?;
        let val_batches = if inputs.iter().any(|i| i.port == 1) {
            Some(common::collect_port(inputs, 1, "dl_deepsurv_train").await?)
        } else {
            None
        };

        let x = common::extract_tensor(&train_batches, &self.spec.features)?;
        let times = common::extract_numeric_column(&train_batches, &self.spec.time_column)?;
        let events_f = common::extract_numeric_column(&train_batches, &self.spec.event_column)?;
        let events: Vec<usize> = events_f.into_iter().map(|v| v as usize).collect();

        let val = if let Some(vb) = &val_batches {
            let xv = common::extract_tensor(vb, &self.spec.features)?;
            let tv = common::extract_numeric_column(vb, &self.spec.time_column)?;
            let ev = common::extract_numeric_column(vb, &self.spec.event_column)?;
            Some((
                xv,
                tv,
                ev.into_iter().map(|v| v as usize).collect::<Vec<_>>(),
            ))
        } else {
            None
        };

        let config = self.spec.to_config()?;
        let result = train_deepsurv(
            &x,
            &times,
            &events,
            val.as_ref()
                .map(|(xv, tv, ev)| (xv, tv.as_slice(), ev.as_slice())),
            &config,
        )
        .map_err(|e| common::err("dl_deepsurv_train", e))?;

        // Port 0: risk scores.
        let (_schema, mut fields, mut arrays) = common::concat_input(&train_batches)?;
        let n = result.risk_scores.nrows();
        let risks: Vec<f64> = (0..n).map(|i| result.risk_scores.at(i, 0)).collect();
        fields.push(Arc::new(Field::new(
            "pred_risk_score",
            DataType::Float64,
            false,
        )));
        arrays.push(Arc::new(Float64Array::from(risks)));
        let pred_batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays)
            .map_err(|e| common::err("dl_deepsurv_train", format!("build pred batch: {e}")))?;

        // Port 1: model artifact.
        let checkpoint_json = serde_json::to_string(&result.model)
            .map_err(|e| common::err("dl_deepsurv_train", format!("serialize model: {e}")))?;
        let artifact = DLModelArtifact {
            backend: "faer".into(),
            architecture: Architecture::Deepsurv,
            task_type: ArtifactTaskType::Survival,
            checkpoint_json,
            feature_names: self.spec.features.clone(),
            label_column: None,
            time_column: Some(self.spec.time_column.clone()),
            event_column: Some(self.spec.event_column.clone()),
            scaler_json: None,
            training_meta: TrainingMeta {
                n_epochs_run: result.training_log.len(),
                best_epoch: result
                    .training_log
                    .iter()
                    .filter(|l| l.val_metric.is_some())
                    .last()
                    .map(|l| l.epoch),
                best_val_metric: result
                    .training_log
                    .iter()
                    .filter(|l| l.val_metric.is_some())
                    .last()
                    .and_then(|l| l.val_metric),
                total_params: result.model.n_params(),
            },
        };
        let artifact_bytes = serde_json::to_vec(&artifact)
            .map_err(|e| common::err("dl_deepsurv_train", format!("serialize artifact: {e}")))?;

        let artifact_batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("artifact_bytes", DataType::Binary, false),
                Field::new("architecture", DataType::Utf8, false),
            ])),
            vec![
                Arc::new(arrow_array::BinaryArray::from(vec![
                    artifact_bytes.as_slice(),
                ])),
                Arc::new(arrow_array::StringArray::from(vec!["deepsurv"])),
            ],
        )
        .map_err(|e| common::err("dl_deepsurv_train", format!("build artifact batch: {e}")))?;

        // Port 2: training log.
        let log_batch = build_training_log_batch(&result.training_log)?;

        let df0 = ctx
            .session()
            .read_batch(pred_batch)
            .map_err(|e| common::err("dl_deepsurv_train", format!("read_batch(0): {e}")))?;
        let df1 = ctx
            .session()
            .read_batch(artifact_batch)
            .map_err(|e| common::err("dl_deepsurv_train", format!("read_batch(1): {e}")))?;
        let df2 = ctx
            .session()
            .read_batch(log_batch)
            .map_err(|e| common::err("dl_deepsurv_train", format!("read_batch(2): {e}")))?;

        let mut res = PortOutputs::new();
        res.insert(0, df0);
        res.insert(1, df1);
        res.insert(2, df2);
        Ok(res)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Shared helpers
// ═══════════════════════════════════════════════════════════════════════

pub fn build_training_log_batch(log: &[dl::mlp::EpochLog]) -> Result<RecordBatch, DagError> {
    let n = log.len();
    let epochs: Vec<Int64ArrayItem> = log.iter().map(|l| l.epoch as i64).collect();
    let train_loss: Vec<f64> = log.iter().map(|l| l.train_loss).collect();
    let val_loss: Vec<Option<f64>> = log.iter().map(|l| l.val_loss).collect();
    let val_metric: Vec<Option<f64>> = log.iter().map(|l| l.val_metric).collect();
    let lr: Vec<f64> = log.iter().map(|l| l.lr).collect();

    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("epoch", DataType::Int64, false),
            Field::new("train_loss", DataType::Float64, false),
            Field::new("val_loss", DataType::Float64, true),
            Field::new("val_metric", DataType::Float64, true),
            Field::new("lr", DataType::Float64, false),
        ])),
        vec![
            Arc::new(arrow_array::Int64Array::from(
                epochs.iter().map(|&v| v).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(train_loss)),
            Arc::new(Float64Array::from(val_loss)),
            Arc::new(Float64Array::from(val_metric)),
            Arc::new(Float64Array::from(lr)),
        ],
    )
    .map_err(|e| common::err("dl_train", format!("build log batch: {e}")))?;
    let _ = n; // suppress unused warning
    Ok(batch)
}

type Int64ArrayItem = i64;
