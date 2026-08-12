//! DeepSurv — neural network with Cox partial likelihood loss.
//!
//! Replaces the linear predictor in a Cox model with a flexible neural
//! network, maintaining the proportional hazards assumption while capturing
//! nonlinear effects.

use rand::SeedableRng;
use rand::seq::SliceRandom;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::layers::{Activation, ActivationLayer, Dropout, InitScheme, Linear};
use crate::losses::cox_partial_likelihood_loss;
use crate::mlp::{extract_rows, clip_gradients, EarlyStoppingConfig, EpochLog, TrainConfig};
use crate::optimizer::{collect_flat_params, param_sizes, scatter_flat_params, Optimizer, OptimizerConfig};
use crate::scheduler::{Scheduler, SchedulerConfig};
use crate::scaler::StandardScaler;
use crate::survival;
use crate::tensor::Tensor;

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
    pub layers: Vec<Linear>,
    pub activations: Vec<ActivationLayer>,
    pub dropouts: Vec<Option<Dropout>>,
    pub scaler: Option<StandardScaler>,
    pub config: DeepSurvConfig,
    pub n_features: usize,
    pub training_log: Vec<EpochLog>,
}

pub struct DeepSurvTrainOutput {
    pub model: DeepSurvModel,
    pub risk_scores: Tensor,
    pub training_log: Vec<EpochLog>,
}

impl DeepSurvModel {
    pub fn new(n_features: usize, config: &DeepSurvConfig) -> Self {
        let mut rng = ChaCha8Rng::seed_from_u64(config.train.seed);

        let mut sizes = vec![n_features];
        sizes.extend(config.hidden_sizes.iter().copied());
        sizes.push(1); // single risk score output

        let n_layers = sizes.len() - 1;
        let mut layers = Vec::with_capacity(n_layers);
        let mut activations = Vec::with_capacity(n_layers - 1);
        let mut dropouts = Vec::with_capacity(n_layers - 1);

        for l in 0..n_layers {
            let init = if l == 0 { InitScheme::Xavier } else { InitScheme::He };
            layers.push(Linear::new(sizes[l], sizes[l + 1], &mut rng, init));
            if l < n_layers - 1 {
                activations.push(ActivationLayer::new(config.activation));
                dropouts.push(if config.dropout > 0.0 {
                    Some(Dropout::new(config.dropout))
                } else {
                    None
                });
            }
        }

        Self {
            layers,
            activations,
            dropouts,
            scaler: None,
            config: config.clone(),
            n_features,
            training_log: Vec::new(),
        }
    }

    /// Forward pass → risk scores (batch, 1).
    pub fn forward(&mut self, x: &Tensor, training: bool, rng: &mut ChaCha8Rng) -> Tensor {
        let mut h = x.clone();
        let n_layers = self.layers.len();

        for l in 0..n_layers {
            h = self.layers[l].forward(&h);
            if l < n_layers - 1 {
                h = self.activations[l].forward(&h);
                if let Some(drop) = self.dropouts.get_mut(l).and_then(|o| o.as_mut()) {
                    if training { drop.train(); } else { drop.eval(); }
                    h = drop.forward(&h, rng);
                }
            }
        }
        h
    }

    /// Forward + backward with Cox partial likelihood loss.
    fn forward_backward(
        &mut self,
        x: &Tensor,
        times: &[f64],
        events: &[usize],
        rng: &mut ChaCha8Rng,
    ) -> (f64, Vec<(Vec<f64>, Vec<f64>)>) {
        let risk_scores = self.forward(x, true, rng);
        let (loss, grad_out) = cox_partial_likelihood_loss(&risk_scores, times, events);

        let n_layers = self.layers.len();
        let mut layer_grads: Vec<(Vec<f64>, Vec<f64>)> = vec![(Vec::new(), Vec::new()); n_layers];

        let mut grad = grad_out;
        for l in (0..n_layers).rev() {
            if l < n_layers - 1 {
                if let Some(drop) = self.dropouts.get(l).and_then(|o| o.as_ref()) {
                    grad = drop.backward(&grad);
                }
                grad = self.activations[l].backward(&grad);
            }
            let (gi, gw, gb) = self.layers[l].backward(&grad);
            grad = gi;
            layer_grads[l] = (gw, gb);
        }

        (loss, layer_grads)
    }

    pub fn n_params(&self) -> usize {
        self.layers.iter().map(|l| l.n_params()).sum()
    }
}

/// Train a DeepSurv model.
///
/// - `x`: features `(n, n_features)`.
/// - `times`: survival times.
/// - `events`: event indicators (1 = event, 0 = censored).
/// - `val`: optional validation `(x_val, times_val, events_val)`.
pub fn train_deepsurv(
    x: &Tensor,
    times: &[f64],
    events: &[usize],
    val: Option<(&Tensor, &[f64], &[usize])>,
    config: &DeepSurvConfig,
) -> Result<DeepSurvTrainOutput, String> {
    let (nrows, ncols) = x.shape();
    if nrows == 0 {
        return Err("empty training data".into());
    }

    let scaler = if config.train.standardize {
        Some(StandardScaler::fit(x))
    } else {
        None
    };
    let x_train = scaler.as_ref().map(|s| s.transform(x)).unwrap_or_else(|| x.clone());

    let mut model = DeepSurvModel::new(ncols, config);
    let sizes = param_sizes(&model.layers);
    let mut opt = Optimizer::new(config.train.optimizer.clone(), &sizes);
    let mut sched = Scheduler::new(config.train.scheduler.clone(), config.train.optimizer.lr);
    let mut rng = ChaCha8Rng::seed_from_u64(config.train.seed);
    let batch_size = config.train.batch_size.min(nrows);

    let mut training_log = Vec::new();
    let mut best_state: Option<(Vec<Vec<f64>>, usize)> = None;
    let mut best_metric = f64::NEG_INFINITY; // C-index, higher = better
    let mut bad_epochs = 0usize;

    for epoch in 0..config.train.n_epochs {
        let mut indices: Vec<usize> = (0..nrows).collect();
        indices.shuffle(&mut rng);

        let mut epoch_loss = 0.0;
        let mut n_batches = 0;

        for chunk in indices.chunks(batch_size) {
            let x_batch = extract_rows(&x_train, chunk);
            let t_batch: Vec<f64> = chunk.iter().map(|&i| times[i]).collect();
            let e_batch: Vec<usize> = chunk.iter().map(|&i| events[i]).collect();

            opt.read_params(&collect_flat_params(&model.layers));

            let (loss, layer_grads) =
                model.forward_backward(&x_batch, &t_batch, &e_batch, &mut rng);
            epoch_loss += loss;
            n_batches += 1;

            let mut flat_grads: Vec<Vec<f64>> = Vec::new();
            for (gw, gb) in &layer_grads {
                flat_grads.push(gw.clone());
                flat_grads.push(gb.clone());
            }

            if let Some(max_norm) = config.train.gradient_clip_norm {
                clip_gradients(&mut flat_grads, max_norm);
            }

            opt.step(&flat_grads);
            scatter_flat_params(&mut model.layers, &opt.write_params());
        }

        let train_loss = epoch_loss / n_batches as f64;

        // Validation C-index.
        let (val_loss, val_metric) = if let Some((xv, tv, ev)) = val {
            let xv_s = scaler.as_ref().map(|s| s.transform(xv)).unwrap_or_else(|| xv.clone());
            let preds = model.forward(&xv_s, false, &mut rng);
            let risks: Vec<f64> = (0..preds.nrows()).map(|i| preds.at(i, 0)).collect();
            let ci = survival::c_index(&risks, tv, ev);
            (Some(train_loss), Some(ci))
        } else {
            (None, None)
        };

        let lr = sched.lr();
        training_log.push(EpochLog { epoch, train_loss, val_loss, val_metric, lr });
        sched.step();

        if let Some(ref es) = config.train.early_stopping {
            let metric_val = match es.metric.as_str() {
                "val_loss" => val_loss,
                "val_cindex" | "val_metric" => val_metric,
                _ => val_metric,
            };
            if let Some(mv) = metric_val {
                let improved = if es.mode == "max" { mv > best_metric } else { mv < best_metric };
                if improved {
                    best_metric = mv;
                    bad_epochs = 0;
                    best_state = Some((collect_flat_params(&model.layers), epoch));
                } else {
                    bad_epochs += 1;
                }
                if bad_epochs >= es.patience {
                    break;
                }
            }
        }
    }

    if let Some((best_params, _)) = best_state {
        scatter_flat_params(&mut model.layers, &best_params);
    }
    model.scaler = scaler;
    model.training_log = training_log.clone();

    let risk_scores = model.forward(&x_train, false, &mut rng);

    Ok(DeepSurvTrainOutput { model, risk_scores, training_log })
}

/// Predict risk scores from a fitted DeepSurv model.
pub fn predict_deepsurv(model: &mut DeepSurvModel, x: &Tensor) -> Tensor {
    let x_scaled = model.scaler.as_ref().map(|s| s.transform(x)).unwrap_or_else(|| x.clone());
    let mut rng = ChaCha8Rng::seed_from_u64(model.config.train.seed);
    model.forward(&x_scaled, false, &mut rng)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimizer::OptimizerKind;
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
                n_epochs: 50,
                batch_size: 16,
                ..Default::default()
            },
        }
    }

    /// Generate synthetic survival data with a known nonlinear risk function.
    fn make_data(n: usize) -> (Tensor, Vec<f64>, Vec<usize>) {
        let mut x_data = Vec::new();
        let mut times = Vec::new();
        let mut events = Vec::new();
        let mut rng = ChaCha8Rng::seed_from_u64(123);

        for _ in 0..n {
            let x1: f64 = rng.random::<f64>() * 4.0 - 2.0;
            let x2: f64 = rng.random::<f64>() * 4.0 - 2.0;
            // Risk = x1^2 + x2^2 (higher → event sooner).
            let risk = x1 * x1 + x2 * x2;
            // Exponential time with hazard proportional to exp(risk).
            let u: f64 = rng.random::<f64>();
            let baseline = -u.ln() / 0.5;
            let t = baseline / risk.exp().max(0.1);
            times.push(t.max(0.01));
            // Event if time < 5 (administrative censoring at 5).
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

        // Should have trained and produced risk scores.
        assert_eq!(result.risk_scores.nrows(), 80);

        // Training C-index should be above 0.5 (better than random).
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
