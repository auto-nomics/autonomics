//! MLP model — enhanced multilayer perceptron for tabular data.
//!
//! Uses Burn autodiff for all gradient computation. Supports configurable
//! hidden layers, activation, dropout, mini-batch training, multiple
//! optimisers, early stopping.

use burn::optim::{Adam, GradientsParams, Optimizer};
use burn::module::AutodiffModule;
use burn::tensor::Tensor as BurnTensor;
use rand::SeedableRng;
use rand::seq::SliceRandom;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::backend::{self, Backend, B};
// Internal use.
use crate::configs::{Activation, LayerWeights, SchedulerConfig, TaskType, TrainConfig};
// Re-exported for nodes-dl: dl::mlp::EarlyStoppingConfig, dl::mlp::EpochLog.
pub use crate::configs::{EarlyStoppingConfig, EpochLog};
use crate::data;
use crate::models::burn_net::{self, BurnMlp};
use crate::scheduler::{Scheduler, SchedulerMode};
use crate::scaler::StandardScaler;
use crate::tensor::Tensor;

// ═══════════════════════════════════════════════════════════════════════
// Config
// ═══════════════════════════════════════════════════════════════════════

/// Configuration for an MLP model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MlpConfig {
    pub hidden_sizes: Vec<usize>,
    pub activation: Activation,
    pub dropout: f64,
    pub batch_norm: bool,
    pub task_type: TaskType,
    #[serde(flatten)]
    pub train: TrainConfig,
}

impl Default for MlpConfig {
    fn default() -> Self {
        Self {
            hidden_sizes: vec![128, 64],
            activation: Activation::Relu,
            dropout: 0.0,
            batch_norm: false,
            task_type: TaskType::Classification,
            train: TrainConfig::default(),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Model + training output
// ═══════════════════════════════════════════════════════════════════════

/// Fitted MLP model — fully serde-serialisable.
///
/// Stores layer weights in `LayerWeights` format, plus configuration and
/// optional scaler. Prediction reconstructs a Burn model from these weights.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MlpModel {
    pub layers: Vec<LayerWeights>,
    pub config: MlpConfig,
    pub n_features: usize,
    pub scaler: Option<StandardScaler>,
    pub training_log: Vec<EpochLog>,
}

impl MlpModel {
    pub fn n_params(&self) -> usize {
        self.layers.iter().map(|l| l.n_params()).sum()
    }
}

/// Output of MLP training.
pub struct MlpTrainOutput {
    pub model: MlpModel,
    pub predictions: Tensor,
    pub training_log: Vec<EpochLog>,
}

// ═══════════════════════════════════════════════════════════════════════
// Training
// ═══════════════════════════════════════════════════════════════════════

/// Train an MLP model on the given data using Burn autodiff.
pub fn train_mlp(
    x: &Tensor,
    y: &Tensor,
    val: Option<(&Tensor, &Tensor)>,
    config: &MlpConfig,
) -> Result<MlpTrainOutput, String> {
    let device = backend::device();
    let (nrows, ncols) = x.shape();
    if nrows == 0 {
        return Err("empty training data".into());
    }

    // Standardise features.
    let scaler = if config.train.standardize {
        Some(StandardScaler::fit(x))
    } else {
        None
    };
    let x_train = scaler
        .as_ref()
        .map(|s| s.transform(x))
        .unwrap_or_else(|| x.clone());

    // Build Burn model.
    let mut sizes = vec![ncols];
    sizes.extend(config.hidden_sizes.iter().copied());
    sizes.push(1); // single output
    let mut model = BurnMlp::<Backend>::new(&device, &sizes);

    // Create optimiser.
    let adam_config = burn_net::create_adam(&config.train.optimizer);
    let mut optim = adam_config.init::<Backend, BurnMlp<Backend>>();

    let mut sched = Scheduler::new(config.train.scheduler.clone(), config.train.optimizer.lr);
    let mut rng = ChaCha8Rng::seed_from_u64(config.train.seed);
    let batch_size = config.train.batch_size.min(nrows);

    let mut training_log: Vec<EpochLog> = Vec::new();
    let mut best_weights: Option<Vec<LayerWeights>> = None;
    let is_max_mode = config
        .train
        .early_stopping
        .as_ref()
        .map(|es| es.mode == "max")
        .unwrap_or(false);
    let mut best_metric = if is_max_mode {
        f64::NEG_INFINITY
    } else {
        f64::INFINITY
    };
    let mut bad_epochs = 0usize;

    for epoch in 0..config.train.n_epochs {
        let mut indices: Vec<usize> = (0..nrows).collect();
        indices.shuffle(&mut rng);

        let mut epoch_loss = 0.0f64;
        let mut n_batches = 0;

        for chunk in indices.chunks(batch_size) {
            let x_batch = data::rows_to_burn(&x_train, chunk, &device);
            let y_batch = data::rows_col_to_burn_1d(y, 0, chunk, &device);

            let output = model.forward(x_batch, config.activation);

            let loss = match config.task_type {
                TaskType::Classification => burn_net::bce_loss(&output, &y_batch),
                TaskType::Regression => {
                    let y_2d = y_batch.clone().reshape([y_batch.shape().dims[0], 1]);
                    burn_net::mse_loss(&output, &y_2d)
                }
            };

            let loss_val = burn_net::read_scalar_1d(&loss);
            epoch_loss += loss_val;
            n_batches += 1;

            let grads = loss.backward();
            let grads = GradientsParams::from_grads(grads, &model);
            model = optim.step(config.train.optimizer.lr, model, grads);
        }

        let train_loss = epoch_loss / n_batches as f64;

        // Validation.
        let (val_loss, val_metric) = if let Some((xv, yv)) = val {
            let xv_s = scaler
                .as_ref()
                .map(|s| s.transform(xv))
                .unwrap_or_else(|| xv.clone());
            let infer_model = model.valid();
            let x_burn = data::f64_to_burn_infer(&xv_s, &device);
            let preds = infer_model.forward(x_burn, config.activation);
            let (vl, vm) = compute_val_metrics_burn(
                &preds,
                yv,
                config.task_type,
            );
            (Some(vl), Some(vm))
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

        // Early stopping.
        if let Some(ref es) = config.train.early_stopping {
            let metric_val = match es.metric.as_str() {
                "val_loss" => val_loss,
                "val_metric" => val_metric,
                "train_loss" => Some(train_loss),
                _ => val_loss,
            };
            if let Some(mv) = metric_val {
                let improved = if es.mode == "max" {
                    mv > best_metric
                } else {
                    mv < best_metric
                };
                if improved {
                    best_metric = mv;
                    bad_epochs = 0;
                    best_weights = Some(model.to_weights());
                } else {
                    bad_epochs += 1;
                }
                if bad_epochs >= es.patience {
                    break;
                }
            }
        }
    }

    // Restore best weights if early stopping saved them.
    if let Some(best) = best_weights {
        model = BurnMlp::<Backend>::from_weights(&best, &device);
    }

    let weights = model.to_weights();
    let total_params = model.n_params();

    let mlp_model = MlpModel {
        layers: weights,
        config: config.clone(),
        n_features: ncols,
        scaler,
        training_log: training_log.clone(),
    };

    // Compute training predictions (using inference backend).
    let infer_model = model.valid();
    let x_burn = data::f64_to_burn_infer(&x_train, &device);
    let raw_preds = infer_model.forward(x_burn, config.activation);

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

    Ok(MlpTrainOutput {
        model: mlp_model,
        predictions,
        training_log,
    })
}

/// Predict from a fitted model.
pub fn predict_mlp(model: &mut MlpModel, x: &Tensor) -> Tensor {
    let device = backend::device();
    let x_scaled = model
        .scaler
        .as_ref()
        .map(|s| s.transform(x))
        .unwrap_or_else(|| x.clone());

    let infer_model = BurnMlp::<B>::from_weights(&model.layers, &device);
    let x_burn = data::f64_to_burn_infer(&x_scaled, &device);
    let raw = infer_model.forward(x_burn, model.config.activation);

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

// ═══════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════

fn compute_val_metrics_burn(
    preds: &BurnTensor<B, 2>,
    targets: &Tensor,
    task_type: TaskType,
) -> (f64, f64) {
    let pred_data = preds.to_data();
    let pred_slice = pred_data.as_slice::<f32>().unwrap();
    let n = targets.nrows();

    match task_type {
        TaskType::Classification => {
            // BCE loss + accuracy.
            let mut loss = 0.0f64;
            let mut correct = 0usize;
            for i in 0..n {
                let logit = pred_slice[i] as f64;
                let p = burn_net::sigmoid_stable(logit);
                let t = targets.at(i, 0);
                let eps = 1e-12;
                loss -= t * (p + eps).ln() + (1.0 - t) * (1.0 - p + eps).ln();
                if (if p > 0.5 { 1.0 } else { 0.0 } - t).abs() < 0.5 {
                    correct += 1;
                }
            }
            (loss / n as f64, correct as f64 / n as f64)
        }
        TaskType::Regression => {
            let mut se = 0.0f64;
            for i in 0..n {
                let d = pred_slice[i] as f64 - targets.at(i, 0);
                se += d * d;
            }
            let mse = se / n as f64;
            (mse, -mse)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::configs::{OptimizerConfig, OptimizerKind, SchedulerConfig};

    fn make_config(task: TaskType, epochs: usize, lr: f64) -> MlpConfig {
        MlpConfig {
            hidden_sizes: vec![8],
            activation: Activation::Relu,
            dropout: 0.0,
            batch_norm: false,
            task_type: task,
            train: TrainConfig {
                optimizer: OptimizerConfig {
                    kind: OptimizerKind::Adam,
                    lr,
                    ..Default::default()
                },
                scheduler: SchedulerConfig::None,
                n_epochs: epochs,
                batch_size: 10,
                ..Default::default()
            },
        }
    }

    #[test]
    fn test_mlp_classification_simple() {
        let mut x_data = Vec::new();
        let mut y_data = Vec::new();
        for i in 0..40 {
            if i < 20 {
                x_data.extend_from_slice(&[i as f64 * 0.01, i as f64 * 0.01]);
                y_data.push(0.0);
            } else {
                x_data.extend_from_slice(&[5.0 + (i - 20) as f64 * 0.01, 5.0 + (i - 20) as f64 * 0.01]);
                y_data.push(1.0);
            }
        }
        let x = Tensor::from_rows(40, 2, &x_data);
        let y = Tensor::from_rows(40, 1, &y_data);

        let config = make_config(TaskType::Classification, 80, 0.05);
        let result = train_mlp(&x, &y, None, &config).unwrap();
        let probs = result.predictions;
        let correct = (0..40)
            .filter(|&i| {
                let p = probs.at(i, 0);
                ((if p > 0.5 { 1.0 } else { 0.0 }) - y_data[i]).abs() < 0.5
            })
            .count();
        assert!(correct >= 30, "only {correct}/40 correct");
    }

    #[test]
    fn test_mlp_regression() {
        let mut x_data = Vec::new();
        let mut y_data = Vec::new();
        for i in 0..50 {
            let x1 = i as f64 * 0.1;
            let x2 = (i as f64 * 0.07).sin();
            x_data.extend_from_slice(&[x1, x2]);
            y_data.push(2.0 * x1 + x2);
        }
        let x = Tensor::from_rows(50, 2, &x_data);
        let y = Tensor::from_rows(50, 1, &y_data);

        let config = make_config(TaskType::Regression, 150, 0.02);
        let result = train_mlp(&x, &y, None, &config).unwrap();
        let preds = result.predictions;
        let mse_val: f64 = (0..50)
            .map(|i| (preds.at(i, 0) - y_data[i]).powi(2))
            .sum::<f64>()
            / 50.0;
        assert!(mse_val < 5.0, "MSE too high: {mse_val}");
    }

    #[test]
    fn test_mlp_predict_on_new_data() {
        let mut x_data = Vec::new();
        let mut y_data = Vec::new();
        for i in 0..40 {
            if i < 20 {
                x_data.extend_from_slice(&[i as f64 * 0.01, 0.0]);
                y_data.push(0.0);
            } else {
                x_data.extend_from_slice(&[5.0 + (i - 20) as f64 * 0.01, 5.0]);
                y_data.push(1.0);
            }
        }
        let x = Tensor::from_rows(40, 2, &x_data);
        let y = Tensor::from_rows(40, 1, &y_data);

        let config = make_config(TaskType::Classification, 80, 0.05);
        let result = train_mlp(&x, &y, None, &config).unwrap();
        let mut model = result.model;

        let x_new = Tensor::from_rows(2, 2, &[0.1, 0.0, 5.1, 5.0]);
        let probs = predict_mlp(&mut model, &x_new);
        assert!(
            probs.at(0, 0) < 0.5,
            "class 0 should have low prob, got {}",
            probs.at(0, 0)
        );
        assert!(
            probs.at(1, 0) > 0.5,
            "class 1 should have high prob, got {}",
            probs.at(1, 0)
        );
    }

    #[test]
    fn test_mlp_model_serialization() {
        let x = Tensor::from_rows(
            10,
            2,
            &(0..20).map(|x| x as f64).collect::<Vec<_>>(),
        );
        let y = Tensor::from_rows(
            10,
            1,
            &(0..10).map(|i| (i % 2) as f64).collect::<Vec<_>>(),
        );

        let config = make_config(TaskType::Classification, 10, 0.01);
        let result = train_mlp(&x, &y, None, &config).unwrap();

        let json = serde_json::to_string(&result.model).unwrap();
        let restored: MlpModel = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.n_features, 2);
        assert_eq!(restored.layers.len(), result.model.layers.len());
    }
}
