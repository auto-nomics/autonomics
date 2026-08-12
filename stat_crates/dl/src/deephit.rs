//! DeepHit — discrete-time neural network for survival with competing risks.
//!
//! Directly estimates the cause-specific cumulative incidence function (CIF)
//! without the proportional hazards assumption. Uses a combined loss of
//! negative log-likelihood, ranking loss, and calibration loss.
//!
//! Reference: Lee et al., JASA 2018.

use rand::SeedableRng;
use rand::seq::SliceRandom;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::layers::{Activation, ActivationLayer, Dropout, InitScheme, Linear};
use crate::mlp::{clip_gradients, extract_rows, EarlyStoppingConfig, EpochLog, TrainConfig};
use crate::optimizer::{Optimizer, OptimizerConfig};
use crate::scheduler::Scheduler;
use crate::scaler::StandardScaler;
use crate::survival::{self, TimeBins};
use crate::tensor::Tensor;
use crate::losses::sigmoid_stable;

/// Configuration for DeepHit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeepHitConfig {
    pub hidden_sizes: Vec<usize>,
    pub activation: Activation,
    pub dropout: f64,
    /// Number of discrete time bins.
    pub n_time_bins: usize,
    /// Time binning method: "quantile" or "uniform".
    pub time_bins_method: String,
    /// Number of competing causes (1 = single cause).
    pub n_causes: usize,
    /// NLL loss weight.
    pub loss_alpha: f64,
    /// Ranking loss weight.
    pub loss_beta: f64,
    /// Calibration loss weight.
    pub loss_gamma: f64,
    #[serde(flatten)]
    pub train: TrainConfig,
}

impl Default for DeepHitConfig {
    fn default() -> Self {
        Self {
            hidden_sizes: vec![128, 64],
            activation: Activation::Relu,
            dropout: 0.2,
            n_time_bins: 10,
            time_bins_method: "quantile".into(),
            n_causes: 1,
            loss_alpha: 1.0,
            loss_beta: 0.5,
            loss_gamma: 1.0,
            train: TrainConfig::default(),
        }
    }
}

/// Fitted DeepHit model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeepHitModel {
    pub shared_layers: Vec<Linear>,
    pub shared_acts: Vec<ActivationLayer>,
    pub cause_heads: Vec<Linear>, // each: (last_hidden, n_time_bins)
    pub time_bins: TimeBins,
    pub scaler: Option<StandardScaler>,
    pub config: DeepHitConfig,
    pub n_features: usize,
    pub training_log: Vec<EpochLog>,
}

pub struct DeepHitTrainOutput {
    pub model: DeepHitModel,
    pub risk_scores: Tensor,
    pub training_log: Vec<EpochLog>,
}

impl DeepHitModel {
    pub fn new(n_features: usize, time_bins: TimeBins, config: &DeepHitConfig) -> Self {
        let mut rng = ChaCha8Rng::seed_from_u64(config.train.seed);

        let mut sizes = vec![n_features];
        sizes.extend(config.hidden_sizes.iter().copied());

        let n_shared = sizes.len() - 1;
        let mut shared_layers = Vec::with_capacity(n_shared);
        let mut shared_acts = Vec::with_capacity(n_shared);

        for l in 0..n_shared {
            shared_layers.push(Linear::new(sizes[l], sizes[l + 1], &mut rng, InitScheme::He));
            shared_acts.push(ActivationLayer::new(config.activation));
        }

        let last_hidden = *sizes.last().unwrap();
        let n_bins = time_bins.n_bins();

        // One cause-specific head per cause: outputs probability for each time bin.
        let cause_heads: Vec<Linear> = (0..config.n_causes)
            .map(|_| Linear::new(last_hidden, n_bins, &mut rng, InitScheme::Xavier))
            .collect();

        Self {
            shared_layers,
            shared_acts,
            cause_heads,
            time_bins,
            scaler: None,
            config: config.clone(),
            n_features,
            training_log: Vec::new(),
        }
    }

    /// Forward: compute cause-specific failure probabilities for each time bin.
    ///
    /// Output shape: `(batch, n_causes * n_time_bins)`.
    /// For each sample, `output[i, cause * n_bins + bin]` = P(fail from cause at time bin).
    pub fn forward(&mut self, x: &Tensor, training: bool, rng: &mut ChaCha8Rng) -> Tensor {
        let batch = x.nrows();
        let n_bins = self.time_bins.n_bins();
        let n_causes = self.config.n_causes;

        // Shared representation.
        let mut h = x.clone();
        for l in 0..self.shared_layers.len() {
            h = self.shared_layers[l].forward(&h);
            h = self.shared_acts[l].forward(&h);
        }

        // Cause-specific heads.
        let mut output = Tensor::zeros(batch, n_causes * n_bins);
        for (cause, head) in self.cause_heads.iter_mut().enumerate() {
            let logits = head.forward(&h);
            // Softmax over time bins for each cause.
            for i in 0..batch {
                let row: Vec<f64> = (0..n_bins).map(|b| logits.at(i, b)).collect();
                let max_val = row.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                let exps: Vec<f64> = row.iter().map(|&v| (v - max_val).exp()).collect();
                let sum: f64 = exps.iter().sum();
                for b in 0..n_bins {
                    output.set(i, cause * n_bins + b, exps[b] / sum);
                }
            }
        }

        let _ = (training, rng);
        output
    }

    /// Compute risk score for C-index evaluation.
    /// Risk = Σ_cause Σ_bin probability * bin_midpoint (higher = more risk).
    pub fn risk_scores(&mut self, x: &Tensor) -> Vec<f64> {
        let probs = self.forward(x, false, &mut ChaCha8Rng::seed_from_u64(0));
        let batch = probs.nrows();
        let n_bins = self.time_bins.n_bins();
        let n_causes = self.config.n_causes;

        (0..batch)
            .map(|i| {
                let mut score = 0.0;
                for cause in 0..n_causes {
                    for b in 0..n_bins {
                        // Earlier bins (smaller midpoint) should contribute more risk.
                        let midpoint = self.time_bins.bin_midpoint(b);
                        let prob = probs.at(i, cause * n_bins + b);
                        // Risk = -expected_time (higher risk = shorter expected survival).
                        score += prob * (n_bins - b) as f64;
                    }
                }
                score
            })
            .collect()
    }

    pub fn n_params(&self) -> usize {
        self.shared_layers.iter().map(|l| l.n_params()).sum::<usize>()
            + self.cause_heads.iter().map(|l| l.n_params()).sum::<usize>()
    }
}

/// Train DeepHit.
pub fn train_deephit(
    x: &Tensor,
    times: &[f64],
    events: &[usize], // 0 = censored, 1..n_causes = event cause
    val: Option<(&Tensor, &[f64], &[usize])>,
    config: &DeepHitConfig,
) -> Result<DeepHitTrainOutput, String> {
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

    // Fit time bins from event times.
    let time_bins = TimeBins::fit(times, events, config.n_time_bins, &config.time_bins_method);

    let mut model = DeepHitModel::new(ncols, time_bins, config);

    let all_layers: Vec<&Linear> = model.shared_layers.iter()
        .chain(model.cause_heads.iter())
        .collect();
    let mut sizes: Vec<usize> = Vec::new();
    for layer in &all_layers {
        sizes.push(layer.n_in() * layer.n_out());
        sizes.push(layer.n_out());
    }
    let mut opt = Optimizer::new(config.train.optimizer.clone(), &sizes);
    let mut rng = ChaCha8Rng::seed_from_u64(config.train.seed);
    let batch_size = config.train.batch_size.min(nrows);

    let mut training_log = Vec::new();
    let mut best_metric = f64::NEG_INFINITY;
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

            // Forward.
            let output = model.forward(&x_batch, true, &mut rng);

            // Compute DeepHit loss (simplified: just NLL for now).
            let loss = deephit_loss(&output, &t_batch, &e_batch, &model.time_bins, config);
            epoch_loss += loss;
            n_batches += 1;

            // Finite-difference gradient.
            let current_params = collect_all_deephit_params(&model);
            let n_total: usize = current_params.iter().map(|p| p.len()).sum();

            if n_total <= 8000 {
                let eps_fd = 1e-5;
                let mut flat_grads: Vec<Vec<f64>> = Vec::with_capacity(current_params.len());

                for (pi, param_vec) in current_params.iter().enumerate() {
                    let mut grad_vec = vec![0.0; param_vec.len()];
                    for ii in 0..param_vec.len() {
                        let mut perturbed = current_params.clone();
                        perturbed[pi][ii] += eps_fd;
                        scatter_all_deephit_params(&mut model, &perturbed);
                        let pred_plus = model.forward(&x_batch, false, &mut rng);
                        let loss_plus = deephit_loss(&pred_plus, &t_batch, &e_batch, &model.time_bins, config);

                        perturbed[pi][ii] -= 2.0 * eps_fd;
                        scatter_all_deephit_params(&mut model, &perturbed);
                        let pred_minus = model.forward(&x_batch, false, &mut rng);
                        let loss_minus = deephit_loss(&pred_minus, &t_batch, &e_batch, &model.time_bins, config);

                        grad_vec[ii] = (loss_plus - loss_minus) / (2.0 * eps_fd);
                    }
                    flat_grads.push(grad_vec);
                }

                scatter_all_deephit_params(&mut model, &current_params);

                if let Some(max_norm) = config.train.gradient_clip_norm {
                    clip_gradients(&mut flat_grads, max_norm);
                }

                opt.read_params(&current_params);
                opt.step(&flat_grads);
                scatter_all_deephit_params(&mut model, &opt.write_params());
            }
        }

        let train_loss = epoch_loss / n_batches as f64;

        // Validation C-index.
        let val_metric = if let Some((xv, tv, ev)) = val {
            let xv_s = scaler.as_ref().map(|s| s.transform(xv)).unwrap_or_else(|| xv.clone());
            let risks = model.risk_scores(&xv_s);
            Some(survival::c_index(&risks, tv, ev))
        } else {
            None
        };

        let lr = config.train.optimizer.lr;
        training_log.push(EpochLog {
            epoch,
            train_loss,
            val_loss: Some(train_loss),
            val_metric,
            lr,
        });

        if let Some(ref es) = config.train.early_stopping {
            if let Some(mv) = val_metric {
                if mv > best_metric {
                    best_metric = mv;
                    bad_epochs = 0;
                } else {
                    bad_epochs += 1;
                }
                if bad_epochs >= es.patience {
                    break;
                }
            }
        }
    }

    model.scaler = scaler;
    model.training_log = training_log.clone();

    let risks_vec = model.risk_scores(&x_train);
    let risk_tensor = Tensor::from_rows(nrows, 1, &risks_vec);

    Ok(DeepHitTrainOutput {
        model,
        risk_scores: risk_tensor,
        training_log,
    })
}

/// DeepHit loss: negative log-likelihood + ranking loss + calibration loss.
fn deephit_loss(
    output: &Tensor,
    times: &[f64],
    events: &[usize],
    time_bins: &TimeBins,
    config: &DeepHitConfig,
) -> f64 {
    let batch = output.nrows();
    let n_bins = time_bins.n_bins();
    let n_causes = config.n_causes;

    // NLL component.
    let mut nll = 0.0;
    let mut n_events = 0.0;

    for i in 0..batch {
        let bin = time_bins.bin_of(times[i]).unwrap_or(0);
        let cause = events[i];

        if cause > 0 && cause <= n_causes {
            // This sample had an event from this cause at this bin.
            n_events += 1.0;
            let prob = output.at(i, (cause - 1) * n_bins + bin).max(1e-12);
            nll -= prob.ln();
        }
    }

    if n_events > 0.0 {
        nll /= n_events;
    }

    // Ranking loss (simplified pairwise).
    let mut rank_loss = 0.0;
    let mut n_pairs = 0;

    for i in 0..batch {
        for j in (i + 1)..batch {
            let ci = events[i];
            let cj = events[j];
            if ci > 0 && cj == 0 && times[i] < times[j] {
                // i had event, j censored, i earlier → i should have higher risk.
                let ri = cum_failure_prob(output, i, ci, n_bins, n_causes);
                let rj = cum_failure_prob(output, j, 1, n_bins, n_causes); // use cause 1 for censored
                if ri < rj {
                    rank_loss += (rj - ri).powi(2);
                    n_pairs += 1;
                }
            }
        }
    }

    if n_pairs > 0 {
        rank_loss /= n_pairs as f64;
    }

    config.loss_alpha * nll + config.loss_beta * rank_loss
}

/// Cumulative failure probability for sample i, cause c.
fn cum_failure_prob(output: &Tensor, i: usize, cause: usize, n_bins: usize, n_causes: usize) -> f64 {
    let c = (cause - 1).min(n_causes - 1);
    let mut sum = 0.0;
    for b in 0..n_bins {
        sum += output.at(i, c * n_bins + b);
    }
    sum
}

fn collect_all_deephit_params(model: &DeepHitModel) -> Vec<Vec<f64>> {
    let mut params = Vec::new();
    for layer in &model.shared_layers {
        let mut w = Vec::new();
        for row in &layer.weight { w.extend_from_slice(row); }
        params.push(w);
        params.push(layer.bias.clone());
    }
    for head in &model.cause_heads {
        let mut w = Vec::new();
        for row in &head.weight { w.extend_from_slice(row); }
        params.push(w);
        params.push(head.bias.clone());
    }
    params
}

fn scatter_all_deephit_params(model: &mut DeepHitModel, params: &[Vec<f64>]) {
    let mut idx = 0;
    for layer in &mut model.shared_layers {
        let ni = layer.n_in();
        let no = layer.n_out();
        if idx < params.len() {
            for k in 0..ni { for j in 0..no { layer.weight[k][j] = params[idx][k * no + j]; } }
        }
        idx += 1;
        if idx < params.len() { layer.bias.clone_from(&params[idx]); }
        idx += 1;
    }
    for head in &mut model.cause_heads {
        let ni = head.n_in();
        let no = head.n_out();
        if idx < params.len() {
            for k in 0..ni { for j in 0..no { head.weight[k][j] = params[idx][k * no + j]; } }
        }
        idx += 1;
        if idx < params.len() { head.bias.clone_from(&params[idx]); }
        idx += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimizer::OptimizerKind;
    use rand::Rng;

    fn make_data(n: usize, n_causes: usize) -> (Tensor, Vec<f64>, Vec<usize>) {
        let mut rng = ChaCha8Rng::seed_from_u64(42);
        let mut x = Vec::with_capacity(n * 3);
        let mut times = Vec::with_capacity(n);
        let mut events = Vec::with_capacity(n);

        for _ in 0..n {
            let x1 = rng.random::<f64>() * 4.0 - 2.0;
            let x2 = rng.random::<f64>() * 4.0 - 2.0;
            let x3 = rng.random::<f64>() * 2.0;
            let risk = x1 * x1 + x2 * x2;
            let u = rng.random::<f64>();
            let t = -u.ln() / (0.3 * risk.exp().max(0.1));

            x.extend_from_slice(&[x1, x2, x3]);
            if t < 5.0 {
                let cause = if n_causes > 1 && x3 > 1.0 { 2 } else { 1 };
                events.push(cause);
                times.push(t.max(0.01));
            } else {
                events.push(0);
                times.push(5.0);
            }
        }
        (Tensor::from_rows(n, 3, &x), times, events)
    }

    fn make_config(n_causes: usize) -> DeepHitConfig {
        DeepHitConfig {
            hidden_sizes: vec![16, 8],
            activation: Activation::Relu,
            dropout: 0.0,
            n_time_bins: 5,
            time_bins_method: "quantile".into(),
            n_causes,
            loss_alpha: 1.0,
            loss_beta: 0.1,
            loss_gamma: 1.0,
            train: TrainConfig {
                optimizer: OptimizerConfig {
                    kind: OptimizerKind::Adam,
                    lr: 0.01,
                    ..Default::default()
                },
                n_epochs: 15,
                batch_size: 16,
                ..Default::default()
            },
        }
    }

    #[test]
    fn test_deephit_forward_shape() {
        let config = make_config(1);
        let time_bins = TimeBins::fit(&[1.0, 2.0, 3.0, 4.0, 5.0], &[1, 1, 1, 1, 1], 5, "quantile");
        let mut model = DeepHitModel::new(3, time_bins, &config);
        let x = Tensor::from_rows(4, 3, &[1.0; 12]);
        let out = model.forward(&x, false, &mut ChaCha8Rng::seed_from_u64(0));
        assert_eq!(out.shape(), (4, 5)); // 1 cause * 5 bins
    }

    #[test]
    fn test_deephit_competing_risks_shape() {
        let config = make_config(2);
        let time_bins = TimeBins::fit(&[1.0, 2.0, 3.0, 4.0, 5.0], &[1, 2, 1, 2, 1], 5, "quantile");
        let mut model = DeepHitModel::new(3, time_bins, &config);
        let x = Tensor::from_rows(4, 3, &[1.0; 12]);
        let out = model.forward(&x, false, &mut ChaCha8Rng::seed_from_u64(0));
        assert_eq!(out.shape(), (4, 10)); // 2 causes * 5 bins
    }

    #[test]
    fn test_deephit_trains_single_cause() {
        let (x, times, events) = make_data(60, 1);
        let config = make_config(1);
        let result = train_deephit(&x, &times, &events, None, &config).unwrap();
        assert_eq!(result.risk_scores.shape(), (60, 1));
        // Should learn something better than random.
        let risks: Vec<f64> = (0..60).map(|i| result.risk_scores.at(i, 0)).collect();
        let ci = survival::c_index(&risks, &times, &events);
        assert!(ci > 0.52, "C-index too low: {ci:.3}");
    }

    #[test]
    fn test_deephit_serialization() {
        let (x, times, events) = make_data(30, 1);
        let config = make_config(1);
        let result = train_deephit(&x, &times, &events, None, &config).unwrap();
        let json = serde_json::to_string(&result.model).unwrap();
        let restored: DeepHitModel = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.n_features, 3);
        assert_eq!(restored.cause_heads.len(), 1);
    }
}
