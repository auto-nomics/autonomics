//! Shared configuration types used across all model architectures.

use serde::{Deserialize, Serialize};

// ═══════════════════════════════════════════════════════════════════════
// Activation
// ═══════════════════════════════════════════════════════════════════════

/// Activation function selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Activation {
    Relu,
    Gelu,
    Selu,
    Tanh,
    Sigmoid,
    LeakyRelu,
    Elu,
}

impl Activation {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "relu" => Some(Activation::Relu),
            "gelu" => Some(Activation::Gelu),
            "selu" => Some(Activation::Selu),
            "tanh" => Some(Activation::Tanh),
            "sigmoid" => Some(Activation::Sigmoid),
            "leaky_relu" => Some(Activation::LeakyRelu),
            "elu" => Some(Activation::Elu),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Activation::Relu => "relu",
            Activation::Gelu => "gelu",
            Activation::Selu => "selu",
            Activation::Tanh => "tanh",
            Activation::Sigmoid => "sigmoid",
            Activation::LeakyRelu => "leaky_relu",
            Activation::Elu => "elu",
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Task type
// ═══════════════════════════════════════════════════════════════════════

/// Task type determines output head and loss.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskType {
    Classification,
    Regression,
}

// ═══════════════════════════════════════════════════════════════════════
// Optimizer
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OptimizerKind {
    Sgd,
    Adam,
    Adamw,
    Rmsprop,
}

impl OptimizerKind {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "sgd" => Some(OptimizerKind::Sgd),
            "adam" => Some(OptimizerKind::Adam),
            "adamw" => Some(OptimizerKind::Adamw),
            "rmsprop" => Some(OptimizerKind::Rmsprop),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizerConfig {
    pub kind: OptimizerKind,
    pub lr: f64,
    pub weight_decay: f64,
    pub momentum: f64,
    pub beta1: f64,
    pub beta2: f64,
    pub eps: f64,
}

impl Default for OptimizerConfig {
    fn default() -> Self {
        Self {
            kind: OptimizerKind::Adam,
            lr: 0.001,
            weight_decay: 0.0,
            momentum: 0.9,
            beta1: 0.9,
            beta2: 0.999,
            eps: 1e-8,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// LR Scheduler
// ═══════════════════════════════════════════════════════════════════════

pub use crate::scheduler::SchedulerConfig;

// ═══════════════════════════════════════════════════════════════════════
// Training config
// ═══════════════════════════════════════════════════════════════════════

/// Shared training hyper-parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainConfig {
    pub optimizer: OptimizerConfig,
    pub scheduler: SchedulerConfig,
    pub n_epochs: usize,
    pub batch_size: usize,
    pub gradient_clip_norm: Option<f64>,
    pub early_stopping: Option<EarlyStoppingConfig>,
    pub standardize: bool,
    pub seed: u64,
}

impl Default for TrainConfig {
    fn default() -> Self {
        Self {
            optimizer: OptimizerConfig::default(),
            scheduler: SchedulerConfig::None,
            n_epochs: 100,
            batch_size: 32,
            gradient_clip_norm: None,
            early_stopping: None,
            standardize: true,
            seed: 42,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EarlyStoppingConfig {
    pub metric: String,
    pub patience: usize,
    pub mode: String, // "min" | "max"
}

/// Per-epoch training log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochLog {
    pub epoch: usize,
    pub train_loss: f64,
    pub val_loss: Option<f64>,
    pub val_metric: Option<f64>,
    pub lr: f64,
}

// ═══════════════════════════════════════════════════════════════════════
// Serializable layer weights (used by all model structs)
// ═══════════════════════════════════════════════════════════════════════

/// Flat weight + bias for a single Linear layer, serde-serializable.
///
/// `weight` is row-major `[out_features * in_features]` (Burn convention:
/// weight matrix shape is `[out_features, in_features]`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerWeights {
    pub weight: Vec<f64>,
    pub bias: Vec<f64>,
    pub in_features: usize,
    pub out_features: usize,
}

impl LayerWeights {
    pub fn n_params(&self) -> usize {
        self.in_features * self.out_features + self.out_features
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Transformer-specific enums
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Pooling {
    Cls,
    Mean,
    Max,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PositionalEncoding {
    None,
    Sinusoidal,
    Learnable,
}

// ═══════════════════════════════════════════════════════════════════════
// AutoEncoder-specific enums
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AeKind {
    Autoencoder,
    Vae,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AeLoss {
    Mse,
    Bce,
    Huber,
}

// ═══════════════════════════════════════════════════════════════════════
// RNN-specific enums
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CellType {
    Rnn,
    Lstm,
    Gru,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SeqPooling {
    Last,
    Mean,
    Max,
}
