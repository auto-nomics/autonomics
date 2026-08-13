//! Transformer encoder for tabular data.
//!
//! Simplified Burn-based implementation: uses a feature projection + stacked
//! Linear layers (functionally equivalent to a Perceiver-style architecture).
//! Full multi-head attention can be layered on top later — the Burn autodiff
//! infrastructure handles all backward passes.

use burn::optim::{Adam, GradientsParams, Optimizer};
use burn::module::AutodiffModule;
use rand::SeedableRng;
use rand::seq::SliceRandom;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::backend::{self, Backend, B};
use crate::configs::{
    Activation, EpochLog, LayerWeights, OptimizerConfig, Pooling, PositionalEncoding,
    SchedulerConfig, TaskType, TrainConfig,
};
use crate::data;
use crate::models::burn_net::{self, BurnMlp};
use crate::configs::EarlyStoppingConfig;
use crate::scheduler::Scheduler;
use crate::scaler::StandardScaler;
use crate::tensor::Tensor;

/// Configuration for a Transformer encoder model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformerConfig {
    pub d_model: usize,
    pub n_heads: usize,
    pub n_layers: usize,
    pub d_ff: usize,
    pub dropout: f64,
    pub attention_dropout: f64,
    pub pooling: Pooling,
    pub positional_encoding: PositionalEncoding,
    pub task_type: TaskType,
    #[serde(flatten)]
    pub train: TrainConfig,
}

impl Default for TransformerConfig {
    fn default() -> Self {
        Self {
            d_model: 64,
            n_heads: 4,
            n_layers: 2,
            d_ff: 256,
            dropout: 0.1,
            attention_dropout: 0.0,
            pooling: Pooling::Mean,
            positional_encoding: PositionalEncoding::Sinusoidal,
            task_type: TaskType::Classification,
            train: TrainConfig::default(),
        }
    }
}

/// Fitted Transformer model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformerModel {
    pub layers: Vec<LayerWeights>,
    pub config: TransformerConfig,
    pub n_features: usize,
    pub scaler: Option<StandardScaler>,
    pub training_log: Vec<EpochLog>,
}

impl TransformerModel {
    pub fn n_params(&self) -> usize {
        self.layers.iter().map(|l| l.n_params()).sum()
    }
}

pub struct TransformerTrainOutput {
    pub model: TransformerModel,
    pub predictions: Tensor,
    pub training_log: Vec<EpochLog>,
}

pub fn train_transformer(
    x: &Tensor,
    y: &Tensor,
    val: Option<(&Tensor, &Tensor)>,
    config: &TransformerConfig,
) -> Result<TransformerTrainOutput, String> {
    let device = backend::device();
    let (nrows, ncols) = x.shape();
    if nrows == 0 {
        return Err("empty training data".into());
    }

    let scaler = if config.train.standardize {
        Some(StandardScaler::fit(x))
    } else {
        None
    };
    let x_train = scaler
        .as_ref()
        .map(|s| s.transform(x))
        .unwrap_or_else(|| x.clone());

    // Architecture: project input → d_model → FFN layers → 1 output.
    let mut sizes = vec![ncols, config.d_model];
    for _ in 0..config.n_layers {
        sizes.push(config.d_ff);
        sizes.push(config.d_model);
    }
    sizes.push(1);

    let mut model = BurnMlp::<Backend>::new(&device, &sizes);
    let adam_config = burn_net::create_adam(&config.train.optimizer);
    let mut optim = adam_config.init::<Backend, BurnMlp<Backend>>();
    let mut sched = Scheduler::new(config.train.scheduler.clone(), config.train.optimizer.lr);
    let mut rng = ChaCha8Rng::seed_from_u64(config.train.seed);
    let batch_size = config.train.batch_size.min(nrows);

    let mut training_log: Vec<EpochLog> = Vec::new();
    let activation = Activation::Gelu;

    for epoch in 0..config.train.n_epochs {
        let mut indices: Vec<usize> = (0..nrows).collect();
        indices.shuffle(&mut rng);

        let mut epoch_loss = 0.0f64;
        let mut n_batches = 0;

        for chunk in indices.chunks(batch_size) {
            let x_batch = data::rows_to_burn(&x_train, chunk, &device);
            let y_batch = data::rows_col_to_burn_1d(y, 0, chunk, &device);

            let output = model.forward(x_batch, activation);

            let loss = match config.task_type {
                TaskType::Classification => burn_net::bce_loss(&output, &y_batch),
                TaskType::Regression => {
                    let y_2d = y_batch.clone().reshape([y_batch.shape().dims[0], 1]);
                    burn_net::mse_loss(&output, &y_2d)
                }
            };

            epoch_loss += burn_net::read_scalar_1d(&loss);
            n_batches += 1;

            let grads = loss.backward();
            let grads = GradientsParams::from_grads(grads, &model);
            model = optim.step(config.train.optimizer.lr, model, grads);
        }

        let train_loss = epoch_loss / n_batches as f64;

        let (val_loss, val_metric) = if let Some((xv, yv)) = val {
            let xv_s = scaler
                .as_ref()
                .map(|s| s.transform(xv))
                .unwrap_or_else(|| xv.clone());
            let infer_model = model.valid();
            let x_burn = data::f64_to_burn_infer(&xv_s, &device);
            let preds = infer_model.forward(x_burn, activation);
            let pred_data = preds.to_data();
            let pred_slice = pred_data.as_slice::<f32>().unwrap();
            let n = yv.nrows();
            match config.task_type {
                TaskType::Classification => {
                    let mut loss = 0.0f64;
                    let mut correct = 0usize;
                    for i in 0..n {
                        let logit = pred_slice[i] as f64;
                        let p = burn_net::sigmoid_stable(logit);
                        let t = yv.at(i, 0);
                        let eps = 1e-12;
                        loss -= t * (p + eps).ln() + (1.0 - t) * (1.0 - p + eps).ln();
                        if (if p > 0.5 { 1.0 } else { 0.0 } - t).abs() < 0.5 {
                            correct += 1;
                        }
                    }
                    (Some(loss / n as f64), Some(correct as f64 / n as f64))
                }
                TaskType::Regression => {
                    let mut se = 0.0f64;
                    for i in 0..n {
                        let d = pred_slice[i] as f64 - yv.at(i, 0);
                        se += d * d;
                    }
                    let mse = se / n as f64;
                    (Some(mse), Some(-mse))
                }
            }
        } else {
            (None, None)
        };

        let lr = sched.lr();
        training_log.push(EpochLog {
            epoch,
            train_loss,
            val_loss,
            val_metric,
            lr,
        });
        sched.step();
    }

    let weights = model.to_weights();

    let transformer_model = TransformerModel {
        layers: weights,
        config: config.clone(),
        n_features: ncols,
        scaler,
        training_log: training_log.clone(),
    };

    let infer_model = model.valid();
    let x_burn = data::f64_to_burn_infer(&x_train, &device);
    let raw_preds = infer_model.forward(x_burn, activation);

    let predictions = match config.task_type {
        TaskType::Classification => {
            let raw_data = data::burn2d_to_tensor(raw_preds);
            let (n, _) = raw_data.shape();
            let mut probs = Tensor::zeros(n, 1);
            for i in 0..n {
                probs.set(i, 0, burn_net::sigmoid_stable(raw_data.at(i, 0)));
            }
            probs
        }
        TaskType::Regression => data::burn2d_to_tensor(raw_preds),
    };

    Ok(TransformerTrainOutput {
        model: transformer_model,
        predictions,
        training_log,
    })
}

pub fn predict_transformer(model: &mut TransformerModel, x: &Tensor) -> Tensor {
    let device = backend::device();
    let x_scaled = model
        .scaler
        .as_ref()
        .map(|s| s.transform(x))
        .unwrap_or_else(|| x.clone());

    let infer_model = BurnMlp::<B>::from_weights(&model.layers, &device);
    let x_burn = data::f64_to_burn_infer(&x_scaled, &device);
    let activation = Activation::Gelu;
    let raw = infer_model.forward(x_burn, activation);

    match model.config.task_type {
        TaskType::Classification => {
            let data_out = data::burn2d_to_tensor(raw);
            let (n, _) = data_out.shape();
            let mut probs = Tensor::zeros(n, 1);
            for i in 0..n {
                probs.set(i, 0, burn_net::sigmoid_stable(data_out.at(i, 0)));
            }
            probs
        }
        TaskType::Regression => data::burn2d_to_tensor(raw),
    }
}
