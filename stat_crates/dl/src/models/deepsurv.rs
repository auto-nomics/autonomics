//! DeepSurv — neural network with Cox partial likelihood loss.
//!
//! Same architecture as MLP; uses the negative Cox partial log-likelihood
//! as the training loss. Burn autodiff computes all gradients.

use burn::module::AutodiffModule;
use burn::optim::{Adam, GradientsParams, Optimizer};
use rand::SeedableRng;
use rand::seq::SliceRandom;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::backend::{self, B, Backend};
use crate::configs::{
    Activation, EpochLog, LayerWeights, OptimizerKind, SchedulerConfig, TrainConfig,
};
use crate::data;
use crate::models::burn_net::{self, BurnMlp};
use crate::scaler::StandardScaler;
use crate::scheduler::Scheduler;
use crate::survival;
use crate::tensor::Tensor;

// Re-export configs needed by nodes-dl.
pub use crate::configs::{EarlyStoppingConfig, OptimizerConfig};

/// Configuration for a DeepSurv model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeepSurvConfig {
    pub hidden_sizes: Vec<usize>,
    pub activation: Activation,
    pub dropout: f64,
    #[serde(flatten)]
    pub train: TrainConfig,
}

impl Default for DeepSurvConfig {
    fn default() -> Self {
        Self {
            hidden_sizes: vec![64, 32],
            activation: Activation::Relu,
            dropout: 0.1,
            train: TrainConfig::default(),
        }
    }
}

/// Fitted DeepSurv model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeepSurvModel {
    pub layers: Vec<LayerWeights>,
    pub config: DeepSurvConfig,
    pub n_features: usize,
    pub scaler: Option<StandardScaler>,
    pub training_log: Vec<EpochLog>,
}

impl DeepSurvModel {
    pub fn n_params(&self) -> usize {
        self.layers.iter().map(|l| l.n_params()).sum()
    }
}

pub struct DeepSurvTrainOutput {
    pub model: DeepSurvModel,
    pub risk_scores: Tensor,
    pub training_log: Vec<EpochLog>,
}

// ═══════════════════════════════════════════════════════════════════════
// Training
// ═══════════════════════════════════════════════════════════════════════

pub fn train_deepsurv(
    x: &Tensor,
    times: &[f64],
    events: &[usize],
    val: Option<(&Tensor, &[f64], &[usize])>,
    config: &DeepSurvConfig,
) -> Result<DeepSurvTrainOutput, String> {
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

    let mut sizes = vec![ncols];
    sizes.extend(config.hidden_sizes.iter().copied());
    sizes.push(1);
    let mut model = BurnMlp::<Backend>::new(&device, &sizes);

    let adam_config = burn_net::create_adam(&config.train.optimizer);
    let mut optim = adam_config.init::<Backend, BurnMlp<Backend>>();
    let mut sched = Scheduler::new(config.train.scheduler.clone(), config.train.optimizer.lr);
    let mut rng = ChaCha8Rng::seed_from_u64(config.train.seed);
    let batch_size = config.train.batch_size.min(nrows);

    let mut training_log: Vec<EpochLog> = Vec::new();
    let mut best_weights: Option<Vec<LayerWeights>> = None;
    let mut best_metric = f64::NEG_INFINITY;
    let mut bad_epochs = 0usize;

    for epoch in 0..config.train.n_epochs {
        let mut indices: Vec<usize> = (0..nrows).collect();
        indices.shuffle(&mut rng);

        let mut epoch_loss = 0.0f64;
        let mut n_batches = 0;

        for chunk in indices.chunks(batch_size) {
            let x_batch = data::rows_to_burn(&x_train, chunk, &device);
            let t_batch: Vec<f64> = chunk.iter().map(|&i| times[i]).collect();
            let e_batch: Vec<usize> = chunk.iter().map(|&i| events[i]).collect();

            let output = model.forward(x_batch, config.activation);
            let loss = burn_net::cox_loss(&output, &t_batch, &e_batch);

            epoch_loss += burn_net::read_scalar_1d(&loss);
            n_batches += 1;

            let grads = loss.backward();
            let grads = GradientsParams::from_grads(grads, &model);
            model = optim.step(config.train.optimizer.lr, model, grads);
        }

        let train_loss = epoch_loss / n_batches as f64;

        // Validation C-index.
        let (val_loss, val_metric) = if let Some((xv, tv, ev)) = val {
            let xv_s = scaler
                .as_ref()
                .map(|s| s.transform(xv))
                .unwrap_or_else(|| xv.clone());
            let infer_model = model.valid();
            let x_burn = data::f64_to_burn_infer(&xv_s, &device);
            let preds = infer_model.forward(x_burn, config.activation);
            let pred_data = preds.to_data();
            let pred_slice = pred_data.as_slice::<f32>().unwrap();
            let risks: Vec<f64> = pred_slice.iter().map(|&v| v as f64).collect();
            let ci = survival::c_index(&risks, tv, ev);
            (Some(train_loss), Some(ci))
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

        if let Some(ref es) = config.train.early_stopping {
            let metric_val = match es.metric.as_str() {
                "val_cindex" | "val_metric" => val_metric,
                _ => val_metric,
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

    if let Some(best) = best_weights {
        model = BurnMlp::<Backend>::from_weights(&best, &device);
    }

    let weights = model.to_weights();

    let ds_model = DeepSurvModel {
        layers: weights,
        config: config.clone(),
        n_features: ncols,
        scaler,
        training_log: training_log.clone(),
    };

    // Compute training risk scores.
    let infer_model = model.valid();
    let x_burn = data::f64_to_burn_infer(&x_train, &device);
    let raw = infer_model.forward(x_burn, config.activation);
    let risk_scores = data::burn2d_to_tensor(raw);

    Ok(DeepSurvTrainOutput {
        model: ds_model,
        risk_scores,
        training_log,
    })
}

/// Predict risk scores.
pub fn predict_deepsurv(model: &mut DeepSurvModel, x: &Tensor) -> Tensor {
    let device = backend::device();
    let x_scaled = model
        .scaler
        .as_ref()
        .map(|s| s.transform(x))
        .unwrap_or_else(|| x.clone());

    let infer_model = BurnMlp::<B>::from_weights(&model.layers, &device);
    let x_burn = data::f64_to_burn_infer(&x_scaled, &device);
    let raw = infer_model.forward(x_burn, model.config.activation);
    data::burn2d_to_tensor(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::configs::{OptimizerConfig, OptimizerKind, SchedulerConfig};
    use rand::Rng;

    fn make_config() -> DeepSurvConfig {
        DeepSurvConfig {
            hidden_sizes: vec![16, 8],
            activation: Activation::Relu,
            dropout: 0.0,
            train: TrainConfig {
                optimizer: OptimizerConfig {
                    kind: OptimizerKind::Adam,
                    lr: 0.01,
                    ..Default::default()
                },
                scheduler: SchedulerConfig::None,
                n_epochs: 80,
                batch_size: 16,
                ..Default::default()
            },
        }
    }

    fn make_data(n: usize) -> (Tensor, Vec<f64>, Vec<usize>) {
        let mut x_data = Vec::new();
        let mut times = Vec::new();
        let mut events = Vec::new();
        let mut rng = ChaCha8Rng::seed_from_u64(123);

        for _ in 0..n {
            let x1: f64 = rng.random::<f64>() * 4.0 - 2.0;
            let x2: f64 = rng.random::<f64>() * 4.0 - 2.0;
            let risk = x1 * x1 + x2 * x2;
            let u: f64 = rng.random::<f64>();
            let baseline = -u.ln() / 0.5;
            let t = baseline / risk.exp().max(0.1);
            times.push(t.max(0.01));
            if t < 5.0 {
                events.push(1);
            } else {
                events.push(0);
                *times.last_mut().unwrap() = 5.0;
            }
            x_data.push(x1);
            x_data.push(x2);
        }
        (Tensor::from_rows(n, 2, &x_data), times, events)
    }

    #[test]
    fn test_deepsurv_trains_and_predicts() {
        let (x, times, events) = make_data(80);
        let config = make_config();
        let result = train_deepsurv(&x, &times, &events, None, &config).unwrap();

        assert_eq!(result.risk_scores.nrows(), 80);

        let risks: Vec<f64> = (0..80).map(|i| result.risk_scores.at(i, 0)).collect();
        let ci = survival::c_index(&risks, &times, &events);
        assert!(ci > 0.55, "training C-index too low: {ci:.3}");
    }

    #[test]
    fn test_deepsurv_serialization() {
        let (x, times, events) = make_data(30);
        let config = make_config();
        let result = train_deepsurv(&x, &times, &events, None, &config).unwrap();

        let json = serde_json::to_string(&result.model).unwrap();
        let restored: DeepSurvModel = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.n_features, 2);
        assert_eq!(restored.layers.len(), result.model.layers.len());
    }
}
