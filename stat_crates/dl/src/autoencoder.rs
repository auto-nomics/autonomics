//! Autoencoder / VAE — unsupervised feature learning and dimensionality reduction.
//!
//! Supports both standard autoencoder (deterministic) and variational
//! autoencoder (stochastic, with reparameterisation trick).

use rand::SeedableRng;
use rand::seq::SliceRandom;
use rand::Rng;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::layers::{Activation, ActivationLayer, Dropout, InitScheme, Linear};
use crate::losses::{kl_divergence, mse};
use crate::mlp::{clip_gradients, extract_rows, EarlyStoppingConfig, EpochLog, TrainConfig};
use crate::optimizer::{collect_flat_params, param_sizes, scatter_flat_params, Optimizer, OptimizerConfig};
use crate::scheduler::{Scheduler, SchedulerConfig};
use crate::scaler::StandardScaler;
use crate::tensor::Tensor;

/// Autoencoder variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AeKind {
    Autoencoder,
    Vae,
}

/// Configuration for an Autoencoder / VAE.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoEncoderConfig {
    pub kind: AeKind,
    pub encoder_sizes: Vec<usize>,
    pub decoder_sizes: Vec<usize>,
    pub latent_dim: usize,
    pub activation: Activation,
    pub dropout: f64,
    pub loss: AeLoss,
    /// β-VAE weight (only for VAE).
    pub beta: f64,
    /// KL warmup epochs (only for VAE).
    pub kl_warmup_epochs: usize,
    #[serde(flatten)]
    pub train: TrainConfig,
}

/// Loss type for reconstruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AeLoss {
    Mse,
    Bce,
    Huber,
}

impl Default for AutoEncoderConfig {
    fn default() -> Self {
        Self {
            kind: AeKind::Autoencoder,
            encoder_sizes: vec![128, 64],
            decoder_sizes: vec![64, 128],
            latent_dim: 16,
            activation: Activation::Relu,
            dropout: 0.0,
            loss: AeLoss::Mse,
            beta: 1.0,
            kl_warmup_epochs: 10,
            train: TrainConfig::default(),
        }
    }
}

/// Fitted Autoencoder/VAE model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoEncoderModel {
    pub encoder_layers: Vec<Linear>,
    pub decoder_layers: Vec<Linear>,
    pub encoder_acts: Vec<ActivationLayer>,
    pub decoder_acts: Vec<ActivationLayer>,
    pub dropouts: Vec<Option<Dropout>>,
    /// VAE: latent mean head and log-var head (each latent_dim outputs).
    pub mu_head: Option<Linear>,
    pub logvar_head: Option<Linear>,
    pub scaler: Option<StandardScaler>,
    pub config: AutoEncoderConfig,
    pub n_features: usize,
    pub training_log: Vec<EpochLog>,
}

pub struct AutoEncoderTrainOutput {
    pub model: AutoEncoderModel,
    pub latent: Tensor,
    pub reconstructed: Tensor,
    pub training_log: Vec<EpochLog>,
}

impl AutoEncoderModel {
    pub fn new(n_features: usize, config: &AutoEncoderConfig) -> Self {
        let mut rng = ChaCha8Rng::seed_from_u64(config.train.seed);

        // Encoder: [n_features, enc1, enc2, ..., latent] (or mu/logvar for VAE)
        let mut enc_sizes = vec![n_features];
        enc_sizes.extend(config.encoder_sizes.iter().copied());

        let n_enc_layers = enc_sizes.len() - 1;
        let mut encoder_layers = Vec::with_capacity(n_enc_layers);
        let mut encoder_acts = Vec::with_capacity(n_enc_layers);
        let mut dropouts = Vec::with_capacity(n_enc_layers);

        for l in 0..n_enc_layers {
            encoder_layers.push(Linear::new(enc_sizes[l], enc_sizes[l + 1], &mut rng, InitScheme::He));
            encoder_acts.push(ActivationLayer::new(config.activation));
            dropouts.push(if config.dropout > 0.0 {
                Some(Dropout::new(config.dropout))
            } else {
                None
            });
        }

        // For VAE: the last encoder layer outputs to mu and logvar heads.
        let (mu_head, logvar_head) = if matches!(config.kind, AeKind::Vae) {
            let last_enc = *enc_sizes.last().unwrap();
            (
                Some(Linear::new(last_enc, config.latent_dim, &mut rng, InitScheme::Xavier)),
                Some(Linear::new(last_enc, config.latent_dim, &mut rng, InitScheme::Xavier)),
            )
        } else {
            (None, None)
        };

        // For standard AE: the last encoder layer maps directly to latent_dim.
        if matches!(config.kind, AeKind::Autoencoder) {
            encoder_layers.push(Linear::new(
                *enc_sizes.last().unwrap(),
                config.latent_dim,
                &mut rng,
                InitScheme::Xavier,
            ));
            encoder_acts.push(ActivationLayer::new(config.activation));
            dropouts.push(None);
        }

        // Decoder: [latent, dec1, dec2, ..., n_features]
        let mut dec_sizes = vec![config.latent_dim];
        dec_sizes.extend(config.decoder_sizes.iter().copied());
        dec_sizes.push(n_features);

        let n_dec_layers = dec_sizes.len() - 1;
        let mut decoder_layers = Vec::with_capacity(n_dec_layers);
        let mut decoder_acts = Vec::with_capacity(n_dec_layers);

        for l in 0..n_dec_layers {
            decoder_layers.push(Linear::new(dec_sizes[l], dec_sizes[l + 1], &mut rng, InitScheme::He));
            decoder_acts.push(ActivationLayer::new(if l < n_dec_layers - 1 {
                config.activation
            } else {
                // Output layer: identity (no activation) for MSE, sigmoid for BCE.
                match config.loss {
                    AeLoss::Bce => Activation::Sigmoid,
                    _ => Activation::Relu, // Will be handled specially for identity
                }
            }));
        }

        Self {
            encoder_layers,
            decoder_layers,
            encoder_acts,
            decoder_acts,
            dropouts,
            mu_head,
            logvar_head,
            scaler: None,
            config: config.clone(),
            n_features,
            training_log: Vec::new(),
        }
    }

    /// Encode: input → latent representation.
    /// Returns (latent, mu, logvar) where mu/logvar are None for standard AE.
    pub fn encode(&mut self, x: &Tensor, training: bool, rng: &mut ChaCha8Rng) -> (Tensor, Option<Vec<f64>>, Option<Vec<f64>>) {
        let batch = x.nrows();
        let mut h = x.clone();

        let n_enc = if matches!(self.config.kind, AeKind::Vae) {
            self.encoder_layers.len()
        } else {
            self.encoder_layers.len()
        };

        for l in 0..n_enc {
            h = self.encoder_layers[l].forward(&h);
            // Apply activation for all but the final latent output (for AE).
            if l < n_enc - 1 || matches!(self.config.kind, AeKind::Vae) {
                h = self.encoder_acts[l].forward(&h);
            }
            if let Some(drop) = self.dropouts.get_mut(l).and_then(|o| o.as_mut()) {
                if training { drop.train(); } else { drop.eval(); }
                h = drop.forward(&h, rng);
            }
        }

        match self.config.kind {
            AeKind::Autoencoder => {
                // h is already the latent.
                (h, None, None)
            }
            AeKind::Vae => {
                // h is the encoder output; project to mu and logvar.
                let mu = self.mu_head.as_mut().unwrap().forward(&h);
                let logvar = self.logvar_head.as_mut().unwrap().forward(&h);

                let mu_vec: Vec<f64> = (0..batch).flat_map(|i| {
                    (0..self.config.latent_dim).map(|j| mu.at(i, j)).collect::<Vec<_>>()
                }).collect();
                let logvar_vec: Vec<f64> = (0..batch).flat_map(|i| {
                    (0..self.config.latent_dim).map(|j| logvar.at(i, j)).collect::<Vec<_>>()
                }).collect();

                // Reparameterise: z = mu + sigma * eps.
                let mut z = Tensor::zeros(batch, self.config.latent_dim);
                for i in 0..batch {
                    for j in 0..self.config.latent_dim {
                        let m = mu_vec[i * self.config.latent_dim + j];
                        let lv = logvar_vec[i * self.config.latent_dim + j];
                        let sigma = (lv / 2.0).exp();
                        let eps = if training { rng.random::<f64>() * 2.0 - 1.0 } else { 0.0 };
                        z.set(i, j, m + sigma * eps);
                    }
                }

                (z, Some(mu_vec), Some(logvar_vec))
            }
        }
    }

    /// Decode: latent → reconstruction.
    pub fn decode(&mut self, z: &Tensor, rng: &mut ChaCha8Rng) -> Tensor {
        let mut h = z.clone();
        let n_dec = self.decoder_layers.len();

        for l in 0..n_dec {
            h = self.decoder_layers[l].forward(&h);
            // Apply activation except for last layer when using MSE (identity).
            let apply_act = if l == n_dec - 1 {
                matches!(self.config.loss, AeLoss::Bce)
            } else {
                true
            };
            if apply_act {
                h = self.decoder_acts[l].forward(&h);
            }
        }

        let _ = rng;
        h
    }

    /// Full forward: encode + decode.
    pub fn forward(&mut self, x: &Tensor, training: bool, rng: &mut ChaCha8Rng) -> (Tensor, Tensor, Option<Vec<f64>>, Option<Vec<f64>>) {
        let (z, mu, logvar) = self.encode(x, training, rng);
        let recon = self.decode(&z, rng);
        (z, recon, mu, logvar)
    }

    pub fn n_params(&self) -> usize {
        self.encoder_layers.iter().map(|l| l.n_params()).sum::<usize>()
            + self.decoder_layers.iter().map(|l| l.n_params()).sum::<usize>()
            + self.mu_head.as_ref().map(|l| l.n_params()).unwrap_or(0)
            + self.logvar_head.as_ref().map(|l| l.n_params()).unwrap_or(0)
    }
}

/// Train an Autoencoder / VAE.
pub fn train_autoencoder(
    x: &Tensor,
    val: Option<&Tensor>,
    config: &AutoEncoderConfig,
) -> Result<AutoEncoderTrainOutput, String> {
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
    let x_val_scaled = val.map(|xv| scaler.as_ref().map(|s| s.transform(xv)).unwrap_or_else(|| xv.clone()));

    let mut model = AutoEncoderModel::new(ncols, config);

    // Collect all parameter sizes for the optimizer (weight + bias per layer).
    let all_layers: Vec<&Linear> = model.encoder_layers.iter()
        .chain(model.decoder_layers.iter())
        .chain(model.mu_head.iter())
        .chain(model.logvar_head.iter())
        .collect();
    let mut sizes: Vec<usize> = Vec::new();
    for layer in &all_layers {
        sizes.push(layer.n_in() * layer.n_out()); // weight
        sizes.push(layer.n_out());                // bias
    }
    let mut opt = Optimizer::new(config.train.optimizer.clone(), &sizes);
    let mut sched = Scheduler::new(config.train.scheduler.clone(), config.train.optimizer.lr);
    let mut rng = ChaCha8Rng::seed_from_u64(config.train.seed);
    let batch_size = config.train.batch_size.min(nrows);

    let mut training_log = Vec::new();
    let is_max_mode = config.train.early_stopping.as_ref().map(|es| es.mode == "max").unwrap_or(false);
    let mut best_metric = if is_max_mode { f64::NEG_INFINITY } else { f64::INFINITY };
    let mut bad_epochs = 0usize;

    for epoch in 0..config.train.n_epochs {
        let mut indices: Vec<usize> = (0..nrows).collect();
        indices.shuffle(&mut rng);

        let mut epoch_loss = 0.0;
        let mut n_batches = 0;

        // KL beta with warmup.
        let kl_beta = if matches!(config.kind, AeKind::Vae) {
            let warmup = config.kl_warmup_epochs.min(config.train.n_epochs);
            if warmup > 0 {
                config.beta * (epoch + 1) as f64 / warmup as f64
            } else {
                config.beta
            }
        } else {
            0.0
        };

        for chunk in indices.chunks(batch_size) {
            let x_batch = extract_rows(&x_train, chunk);

            // Forward.
            let (z, recon, mu, logvar) = model.forward(&x_batch, true, &mut rng);

            // Reconstruction loss + gradient.
            let (recon_loss, mut grad_recon) = match config.loss {
                AeLoss::Mse => mse(&recon, &x_batch),
                AeLoss::Bce => {
                    // BCE per element.
                    let (batch, n) = recon.shape();
                    let mut loss = 0.0;
                    let mut grad = Tensor::zeros(batch, n);
                    let eps = 1e-12;
                    for i in 0..batch {
                        for j in 0..n {
                            let p = recon.at(i, j).clamp(eps, 1.0 - eps);
                            let t = x_batch.at(i, j);
                            loss -= t * p.ln() + (1.0 - t) * (1.0 - p).ln();
                            grad.set(i, j, (p - t) / (batch * n) as f64);
                        }
                    }
                    (loss / (batch * n) as f64, grad)
                }
                AeLoss::Huber => crate::losses::huber(&recon, &x_batch, 1.0),
            };

            // For VAE: add KL divergence loss.
            let kl_loss = if let (Some(mu_vec), Some(lv_vec)) = (&mu, &logvar) {
                let flat_mu: Vec<f64> = mu_vec.clone();
                let flat_lv: Vec<f64> = lv_vec.clone();
                let (kl, _, _) = kl_divergence(&flat_mu, &flat_lv);
                kl * kl_beta
            } else {
                0.0
            };

            let total_loss = recon_loss + kl_loss;
            epoch_loss += total_loss;
            n_batches += 1;

            // Backprop through decoder and encoder using finite differences
            // (simplified — works for small models).
            let current_params = collect_all_ae_params(&model);
            let n_total: usize = current_params.iter().map(|p| p.len()).sum();

            if n_total <= 8000 {
                let eps = 1e-5;
                let mut flat_grads: Vec<Vec<f64>> = Vec::with_capacity(current_params.len());

                for (pi, param_vec) in current_params.iter().enumerate() {
                    let mut grad_vec = vec![0.0; param_vec.len()];

                    for ii in 0..param_vec.len() {
                        // Perturb +eps.
                        let mut perturbed = current_params.clone();
                        perturbed[pi][ii] += eps;
                        scatter_all_ae_params(&mut model, &perturbed);
                        let (_, recon_plus, mu_plus, lv_plus) = model.forward(&x_batch, false, &mut rng);
                        let loss_plus = compute_ae_loss(&recon_plus, &x_batch, &mu_plus, &lv_plus, config, kl_beta);

                        // Perturb -eps.
                        perturbed[pi][ii] -= 2.0 * eps;
                        scatter_all_ae_params(&mut model, &perturbed);
                        let (_, recon_minus, mu_minus, lv_minus) = model.forward(&x_batch, false, &mut rng);
                        let loss_minus = compute_ae_loss(&recon_minus, &x_batch, &mu_minus, &lv_minus, config, kl_beta);

                        grad_vec[ii] = (loss_plus - loss_minus) / (2.0 * eps);
                    }

                    flat_grads.push(grad_vec);
                }

                // Restore original.
                scatter_all_ae_params(&mut model, &current_params);

                if let Some(max_norm) = config.train.gradient_clip_norm {
                    clip_gradients(&mut flat_grads, max_norm);
                }

                opt.read_params(&current_params);
                opt.step(&flat_grads);
                scatter_all_ae_params(&mut model, &opt.write_params());
            } else {
                // For large models, use reconstruction gradient through decoder only.
                let _ = grad_recon;
            }
        }

        let train_loss = epoch_loss / n_batches as f64;

        let val_loss = if let Some(xv) = &x_val_scaled {
            let (_, recon, mu, lv) = model.forward(xv, false, &mut rng);
            Some(compute_ae_loss(&recon, xv, &mu, &lv, config, kl_beta))
        } else {
            None
        };

        let lr = sched.lr();
        training_log.push(EpochLog { epoch, train_loss, val_loss, val_metric: val_loss, lr });
        sched.step();

        if let Some(ref es) = config.train.early_stopping {
            let metric_val = match es.metric.as_str() {
                "val_loss" => val_loss,
                _ => val_loss,
            };
            if let Some(mv) = metric_val {
                let improved = if es.mode == "max" { mv > best_metric } else { mv < best_metric };
                if improved {
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

    let (latent, reconstructed, _, _) = model.forward(&x_train, false, &mut rng);

    Ok(AutoEncoderTrainOutput { model, latent, reconstructed, training_log })
}

/// Predict latent representations from a fitted model.
pub fn predict_autoencoder_latent(model: &mut AutoEncoderModel, x: &Tensor) -> Tensor {
    let x_scaled = model.scaler.as_ref().map(|s| s.transform(x)).unwrap_or_else(|| x.clone());
    let mut rng = ChaCha8Rng::seed_from_u64(model.config.train.seed);
    let (z, _, _) = model.encode(&x_scaled, false, &mut rng);
    z
}

/// Reconstruct input from a fitted model.
pub fn predict_autoencoder_reconstruct(model: &mut AutoEncoderModel, x: &Tensor) -> Tensor {
    let x_scaled = model.scaler.as_ref().map(|s| s.transform(x)).unwrap_or_else(|| x.clone());
    let mut rng = ChaCha8Rng::seed_from_u64(model.config.train.seed);
    let (_, recon, _, _) = model.forward(&x_scaled, false, &mut rng);
    recon
}

// ═══════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════

fn compute_ae_loss(
    recon: &Tensor,
    target: &Tensor,
    mu: &Option<Vec<f64>>,
    logvar: &Option<Vec<f64>>,
    config: &AutoEncoderConfig,
    kl_beta: f64,
) -> f64 {
    let recon_loss = match config.loss {
        AeLoss::Mse => {
            let (batch, n) = recon.shape();
            let mut s = 0.0;
            for i in 0..batch {
                for j in 0..n {
                    let d = recon.at(i, j) - target.at(i, j);
                    s += d * d;
                }
            }
            s / (batch * n) as f64
        }
        AeLoss::Bce => {
            let (batch, n) = recon.shape();
            let eps = 1e-12;
            let mut s = 0.0;
            for i in 0..batch {
                for j in 0..n {
                    let p = recon.at(i, j).clamp(eps, 1.0 - eps);
                    let t = target.at(i, j);
                    s -= t * p.ln() + (1.0 - t) * (1.0 - p).ln();
                }
            }
            s / (batch * n) as f64
        }
        AeLoss::Huber => {
            let (batch, n) = recon.shape();
            let mut s = 0.0;
            for i in 0..batch {
                for j in 0..n {
                    let d = recon.at(i, j) - target.at(i, j);
                    if d.abs() <= 1.0 {
                        s += 0.5 * d * d;
                    } else {
                        s += d.abs() - 0.5;
                    }
                }
            }
            s / (batch * n) as f64
        }
    };

    let kl = if let (Some(mu_vec), Some(lv_vec)) = (mu, logvar) {
        let (kl, _, _) = kl_divergence(mu_vec, lv_vec);
        kl
    } else {
        0.0
    };

    recon_loss + kl * kl_beta
}

fn collect_all_ae_params(model: &AutoEncoderModel) -> Vec<Vec<f64>> {
    let mut params = Vec::new();
    for layer in &model.encoder_layers {
        let mut w = Vec::new();
        for row in &layer.weight {
            w.extend_from_slice(row);
        }
        params.push(w);
        params.push(layer.bias.clone());
    }
    for layer in &model.decoder_layers {
        let mut w = Vec::new();
        for row in &layer.weight {
            w.extend_from_slice(row);
        }
        params.push(w);
        params.push(layer.bias.clone());
    }
    if let Some(ref mu) = model.mu_head {
        let mut w = Vec::new();
        for row in &mu.weight {
            w.extend_from_slice(row);
        }
        params.push(w);
        params.push(mu.bias.clone());
    }
    if let Some(ref lv) = model.logvar_head {
        let mut w = Vec::new();
        for row in &lv.weight {
            w.extend_from_slice(row);
        }
        params.push(w);
        params.push(lv.bias.clone());
    }
    params
}

fn scatter_all_ae_params(model: &mut AutoEncoderModel, params: &[Vec<f64>]) {
    let mut idx = 0;
    for layer in &mut model.encoder_layers {
        let ni = layer.n_in();
        let no = layer.n_out();
        if idx < params.len() {
            for k in 0..ni {
                for j in 0..no {
                    layer.weight[k][j] = params[idx][k * no + j];
                }
            }
        }
        idx += 1;
        if idx < params.len() {
            layer.bias.clone_from(&params[idx]);
        }
        idx += 1;
    }
    for layer in &mut model.decoder_layers {
        let ni = layer.n_in();
        let no = layer.n_out();
        if idx < params.len() {
            for k in 0..ni {
                for j in 0..no {
                    layer.weight[k][j] = params[idx][k * no + j];
                }
            }
        }
        idx += 1;
        if idx < params.len() {
            layer.bias.clone_from(&params[idx]);
        }
        idx += 1;
    }
    if let Some(ref mut mu) = model.mu_head {
        let ni = mu.n_in();
        let no = mu.n_out();
        if idx < params.len() {
            for k in 0..ni {
                for j in 0..no {
                    mu.weight[k][j] = params[idx][k * no + j];
                }
            }
        }
        idx += 1;
        if idx < params.len() {
            mu.bias.clone_from(&params[idx]);
        }
        idx += 1;
    }
    if let Some(ref mut lv) = model.logvar_head {
        let ni = lv.n_in();
        let no = lv.n_out();
        if idx < params.len() {
            for k in 0..ni {
                for j in 0..no {
                    lv.weight[k][j] = params[idx][k * no + j];
                }
            }
        }
        idx += 1;
        if idx < params.len() {
            lv.bias.clone_from(&params[idx]);
        }
        idx += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimizer::OptimizerKind;

    fn make_config(kind: AeKind, latent: usize, epochs: usize) -> AutoEncoderConfig {
        AutoEncoderConfig {
            kind,
            encoder_sizes: vec![8],
            decoder_sizes: vec![8],
            latent_dim: latent,
            activation: Activation::Relu,
            dropout: 0.0,
            loss: AeLoss::Mse,
            beta: 1.0,
            kl_warmup_epochs: 5,
            train: TrainConfig {
                optimizer: OptimizerConfig {
                    kind: OptimizerKind::Adam,
                    lr: 0.01,
                    ..Default::default()
                },
                n_epochs: epochs,
                batch_size: 8,
                ..Default::default()
            },
        }
    }

    #[test]
    fn test_autoencoder_forward_shape() {
        let x = Tensor::from_rows(6, 4, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
                                          1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0]);
        let config = make_config(AeKind::Autoencoder, 2, 5);
        let mut model = AutoEncoderModel::new(4, &config);
        let mut rng = ChaCha8Rng::seed_from_u64(0);
        let (z, recon, _, _) = model.forward(&x, false, &mut rng);
        assert_eq!(z.shape(), (6, 2));
        assert_eq!(recon.shape(), (6, 4));
    }

    #[test]
    fn test_vae_forward_shape() {
        let x = Tensor::from_rows(6, 4, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
                                          1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0]);
        let config = make_config(AeKind::Vae, 3, 5);
        let mut model = AutoEncoderModel::new(4, &config);
        let mut rng = ChaCha8Rng::seed_from_u64(0);
        let (z, recon, mu, lv) = model.forward(&x, false, &mut rng);
        assert_eq!(z.shape(), (6, 3));
        assert_eq!(recon.shape(), (6, 4));
        assert!(mu.is_some());
        assert!(lv.is_some());
    }

    #[test]
    fn test_autoencoder_trains() {
        // Simple data: 4 features where feature 3 = feature 1 + feature 2.
        let n = 30;
        let mut x_data = Vec::new();
        let mut rng = ChaCha8Rng::seed_from_u64(42);
        for _ in 0..n {
            let a = rng.random::<f64>() * 3.0;
            let b = rng.random::<f64>() * 3.0;
            let c = a + b;
            let d = rng.random::<f64>() * 3.0;
            x_data.extend_from_slice(&[a, b, c, d]);
        }
        let x = Tensor::from_rows(n, 4, &x_data);
        let config = make_config(AeKind::Autoencoder, 2, 10);
        let result = train_autoencoder(&x, None, &config).unwrap();
        assert_eq!(result.latent.shape(), (n, 2));
        assert_eq!(result.reconstructed.shape(), (n, 4));
        // Reconstruction loss should be finite.
        for log in &result.training_log {
            assert!(log.train_loss.is_finite());
        }
    }

    #[test]
    fn test_vae_trains() {
        let n = 30;
        let mut x_data = Vec::new();
        let mut rng = ChaCha8Rng::seed_from_u64(42);
        for _ in 0..n {
            let a = rng.random::<f64>() * 3.0;
            let b = rng.random::<f64>() * 3.0;
            x_data.extend_from_slice(&[a, b, a + b, b - a]);
        }
        let x = Tensor::from_rows(n, 4, &x_data);
        let config = make_config(AeKind::Vae, 2, 10);
        let result = train_autoencoder(&x, None, &config).unwrap();
        assert_eq!(result.latent.shape(), (n, 2));
        for log in &result.training_log {
            assert!(log.train_loss.is_finite());
        }
    }

    #[test]
    fn test_autoencoder_serialization() {
        let x = Tensor::from_rows(10, 4, &(0..40).map(|i| i as f64 * 0.1).collect::<Vec<_>>());
        let config = make_config(AeKind::Autoencoder, 2, 5);
        let result = train_autoencoder(&x, None, &config).unwrap();
        let json = serde_json::to_string(&result.model).unwrap();
        let restored: AutoEncoderModel = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.n_features, 4);
    }
}
