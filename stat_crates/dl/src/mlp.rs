//! MLP model — enhanced multilayer perceptron for tabular data.
//!
//! Supports: configurable hidden layers, activation, dropout, batch norm,
//! mini-batch training, multiple optimizers, early stopping.
//!
//! Forward pass: Linear → [BatchNorm] → Activation → [Dropout] repeated,
//! final Linear → output head.

use rand::SeedableRng;
use rand::seq::SliceRandom;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::layers::{Activation, ActivationLayer, BatchNorm1d, Dropout, InitScheme, Linear};
use crate::losses::{self, binary_cross_entropy, mse};
use crate::optimizer::{
    collect_flat_params, param_sizes, scatter_flat_params, Optimizer, OptimizerConfig,
};
use crate::scheduler::{Scheduler, SchedulerConfig};
use crate::scaler::StandardScaler;
use crate::tensor::Tensor;

/// Task type determines the output head and loss function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskType {
    Classification,
    Regression,
}

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

/// Shared training hyper-parameters used by all model architectures.
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

/// Fitted MLP model — all weights and configuration needed for prediction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MlpModel {
    pub layers: Vec<Linear>,
    pub activations: Vec<ActivationLayer>,
    pub dropouts: Vec<Option<Dropout>>,
    pub batch_norms: Vec<Option<BatchNorm1d>>,
    pub scaler: Option<StandardScaler>,
    pub config: MlpConfig,
    pub n_features: usize,
    pub training_log: Vec<EpochLog>,
}

/// Output of MLP training.
pub struct MlpTrainOutput {
    pub model: MlpModel,
    pub predictions: Tensor,
    pub training_log: Vec<EpochLog>,
}

impl MlpModel {
    /// Build the model architecture (randomly initialised).
    pub fn new(n_features: usize, config: &MlpConfig) -> Self {
        let mut rng = ChaCha8Rng::seed_from_u64(config.train.seed);

        // Layer sizes: [n_features, h1, h2, ..., 1].
        let mut sizes = vec![n_features];
        sizes.extend(config.hidden_sizes.iter().copied());
        sizes.push(1); // single output

        let n_layers = sizes.len() - 1;
        let mut layers = Vec::with_capacity(n_layers);
        let mut activations = Vec::with_capacity(n_layers - 1);
        let mut dropouts = Vec::with_capacity(n_layers - 1);
        let mut batch_norms = Vec::with_capacity(n_layers - 1);

        for l in 0..n_layers {
            let init = if l == 0 {
                InitScheme::Xavier
            } else {
                InitScheme::He
            };
            layers.push(Linear::new(sizes[l], sizes[l + 1], &mut rng, init));

            if l < n_layers - 1 {
                activations.push(ActivationLayer::new(config.activation));
                dropouts.push(if config.dropout > 0.0 {
                    Some(Dropout::new(config.dropout))
                } else {
                    None
                });
                batch_norms.push(if config.batch_norm {
                    Some(BatchNorm1d::new(sizes[l + 1], &mut rng))
                } else {
                    None
                });
            }
        }

        Self {
            layers,
            activations,
            dropouts,
            batch_norms,
            scaler: None,
            config: config.clone(),
            n_features,
            training_log: Vec::new(),
        }
    }

    /// Forward pass.
    pub fn forward(&mut self, x: &Tensor, training: bool, rng: &mut ChaCha8Rng) -> Tensor {
        let mut h = x.clone();
        let n_layers = self.layers.len();

        for l in 0..n_layers {
            h = self.layers[l].forward(&h);

            if l < n_layers - 1 {
                if let Some(bn) = self.batch_norms.get_mut(l).and_then(|o| o.as_mut()) {
                    if training { bn.train(); } else { bn.eval(); }
                    h = bn.forward(&h);
                }
                h = self.activations[l].forward(&h);
                if let Some(drop) = self.dropouts.get_mut(l).and_then(|o| o.as_mut()) {
                    if training { drop.train(); } else { drop.eval(); }
                    h = drop.forward(&h, rng);
                }
            }
        }
        h
    }

    /// Forward + backward, returning per-layer (weight_grad, bias_grad) and loss.
    fn forward_backward(
        &mut self,
        x: &Tensor,
        targets: &Tensor,
        rng: &mut ChaCha8Rng,
    ) -> (f64, Vec<(Vec<f64>, Vec<f64>)>) {
        let output = self.forward(x, true, rng);

        let (loss, mut grad_out) = match self.config.task_type {
            TaskType::Classification => {
                let t: Vec<f64> = (0..targets.nrows()).map(|i| targets.at(i, 0)).collect();
                binary_cross_entropy(&output, &t)
            }
            TaskType::Regression => mse(&output, targets),
        };

        let n_layers = self.layers.len();
        let mut layer_grads: Vec<(Vec<f64>, Vec<f64>)> = vec![(Vec::new(), Vec::new()); n_layers];

        for l in (0..n_layers).rev() {
            if l < n_layers - 1 {
                if let Some(drop) = self.dropouts.get(l).and_then(|o| o.as_ref()) {
                    grad_out = drop.backward(&grad_out);
                }
                grad_out = self.activations[l].backward(&grad_out);
                if let Some(bn) = self.batch_norms.get(l).and_then(|o| o.as_ref()) {
                    let (gi, _gg, _gb) = bn.backward(&grad_out);
                    grad_out = gi;
                }
            }
            let (gi, gw, gb) = self.layers[l].backward(&grad_out);
            grad_out = gi;
            layer_grads[l] = (gw, gb);
        }

        (loss, layer_grads)
    }

    /// Count total parameters.
    pub fn n_params(&self) -> usize {
        self.layers.iter().map(|l| l.n_params()).sum()
    }
}

/// Train an MLP model on the given data.
pub fn train_mlp(
    x: &Tensor,
    y: &Tensor,
    val: Option<(&Tensor, &Tensor)>,
    config: &MlpConfig,
) -> Result<MlpTrainOutput, String> {
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
    let x_val_scaled = val.and_then(|(xv, _)| scaler.as_ref().map(|s| s.transform(xv)));

    let mut model = MlpModel::new(ncols, config);
    let sizes = param_sizes(&model.layers);
    let mut opt = Optimizer::new(config.train.optimizer.clone(), &sizes);
    let mut sched = Scheduler::new(config.train.scheduler.clone(), config.train.optimizer.lr);
    let mut rng = ChaCha8Rng::seed_from_u64(config.train.seed);
    let batch_size = config.train.batch_size.min(nrows);

    let mut training_log = Vec::new();
    let mut best_state: Option<(Vec<Vec<f64>>, usize)> = None;
    let is_max_mode = config.train.early_stopping.as_ref().map(|es| es.mode == "max").unwrap_or(false);
    let mut best_metric = if is_max_mode { f64::NEG_INFINITY } else { f64::INFINITY };
    let mut bad_epochs = 0usize;

    for epoch in 0..config.train.n_epochs {
        let mut indices: Vec<usize> = (0..nrows).collect();
        indices.shuffle(&mut rng);

        let mut epoch_loss = 0.0;
        let mut n_batches = 0;

        for chunk in indices.chunks(batch_size) {
            let x_batch = extract_rows(&x_train, chunk);
            let y_batch = extract_rows(y, chunk);

            opt.read_params(&collect_flat_params(&model.layers));

            let (loss, layer_grads) = model.forward_backward(&x_batch, &y_batch, &mut rng);
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

        let (val_loss, val_metric) = if let Some((xv, yv)) = val {
            let xv_s = x_val_scaled.as_ref().unwrap_or(xv);
            let preds = model.forward(xv_s, false, &mut rng);
            let (vl, vm) = compute_val_metrics(&preds, yv, config.task_type);
            (Some(vl), Some(vm))
        } else {
            (None, None)
        };

        let lr = sched.lr();
        training_log.push(EpochLog { epoch, train_loss, val_loss, val_metric, lr });
        sched.step();

        if let Some(ref es) = config.train.early_stopping {
            let metric_val = match es.metric.as_str() {
                "val_loss" => val_loss,
                "val_metric" => val_metric,
                "train_loss" => Some(train_loss),
                _ => val_loss,
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

    let predictions = model.forward(&x_train, false, &mut rng);

    Ok(MlpTrainOutput { model, predictions, training_log })
}

/// Predict from a fitted model.
pub fn predict_mlp(model: &mut MlpModel, x: &Tensor) -> Tensor {
    let x_scaled = model.scaler.as_ref().map(|s| s.transform(x)).unwrap_or_else(|| x.clone());
    let mut rng = ChaCha8Rng::seed_from_u64(model.config.train.seed);
    let logits = model.forward(&x_scaled, false, &mut rng);

    match model.config.task_type {
        TaskType::Classification => {
            let (batch, _) = logits.shape();
            let mut probs = Tensor::zeros(batch, 1);
            for i in 0..batch {
                probs.set(i, 0, losses::sigmoid_stable(logits.at(i, 0)));
            }
            probs
        }
        TaskType::Regression => logits,
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════

pub(crate) fn extract_rows(tensor: &Tensor, indices: &[usize]) -> Tensor {
    let ncols = tensor.ncols();
    let mut flat = Vec::with_capacity(indices.len() * ncols);
    for &i in indices {
        flat.extend_from_slice(&tensor.row(i));
    }
    Tensor::from_rows(indices.len(), ncols, &flat)
}

pub(crate) fn compute_val_metrics(preds: &Tensor, targets: &Tensor, task_type: TaskType) -> (f64, f64) {
    match task_type {
        TaskType::Classification => {
            let t: Vec<f64> = (0..targets.nrows()).map(|i| targets.at(i, 0)).collect();
            let (loss, _) = binary_cross_entropy(preds, &t);
            let correct = (0..preds.nrows())
                .filter(|&i| {
                    let p = losses::sigmoid_stable(preds.at(i, 0));
                    (if p > 0.5 { 1.0 } else { 0.0 } - t[i]).abs() < 0.5
                })
                .count();
            (loss, correct as f64 / preds.nrows() as f64)
        }
        TaskType::Regression => {
            let (loss, _) = mse(preds, targets);
            (loss, -loss)
        }
    }
}

pub(crate) fn clip_gradients(grads: &mut [Vec<f64>], max_norm: f64) {
    let norm: f64 = grads.iter().flat_map(|g| g.iter()).map(|&v| v * v).sum::<f64>().sqrt();
    if norm > max_norm && norm > 0.0 {
        let scale = max_norm / norm;
        for g in grads.iter_mut() {
            for v in g.iter_mut() {
                *v *= scale;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimizer::OptimizerKind;

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

        let config = make_config(TaskType::Classification, 50, 0.01);
        let result = train_mlp(&x, &y, None, &config).unwrap();
        let probs = result.predictions;
        let correct = (0..40).filter(|&i| {
            let p = losses::sigmoid_stable(probs.at(i, 0));
            ((if p > 0.5 { 1.0 } else { 0.0 }) - y_data[i]).abs() < 0.5
        }).count();
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

        let config = make_config(TaskType::Regression, 100, 0.01);
        let result = train_mlp(&x, &y, None, &config).unwrap();
        let preds = result.predictions;
        let mse_val: f64 = (0..50).map(|i| (preds.at(i, 0) - y_data[i]).powi(2)).sum::<f64>() / 50.0;
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

        let config = make_config(TaskType::Classification, 50, 0.01);
        let result = train_mlp(&x, &y, None, &config).unwrap();
        let mut model = result.model;

        let x_new = Tensor::from_rows(2, 2, &[0.1, 0.0, 5.1, 5.0]);
        let probs = predict_mlp(&mut model, &x_new);
        assert!(probs.at(0, 0) < 0.5, "class 0 should have low prob, got {}", probs.at(0, 0));
        assert!(probs.at(1, 0) > 0.5, "class 1 should have high prob, got {}", probs.at(1, 0));
    }

    #[test]
    fn test_mlp_model_serialization() {
        let x = Tensor::from_rows(10, 2, &(0..20).map(|x| x as f64).collect::<Vec<_>>());
        let y = Tensor::from_rows(10, 1, &(0..10).map(|i| (i % 2) as f64).collect::<Vec<_>>());

        let config = make_config(TaskType::Classification, 5, 0.01);
        let result = train_mlp(&x, &y, None, &config).unwrap();

        // Serialize and deserialize.
        let json = serde_json::to_string(&result.model).unwrap();
        let restored: MlpModel = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.n_features, 2);
        assert_eq!(restored.layers.len(), result.model.layers.len());
    }
}
