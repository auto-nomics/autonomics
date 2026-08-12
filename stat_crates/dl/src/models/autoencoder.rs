//! Autoencoder / VAE — unsupervised feature learning.
//!
//! Encoder maps input → latent, decoder maps latent → reconstruction.

use burn::module::{AutodiffModule, Module};
use burn::nn::Linear;
use burn::optim::{Adam, GradientsParams, Optimizer};
use burn::tensor::backend::Backend;
use rand::SeedableRng;
use rand::seq::SliceRandom;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::backend::{self, Backend as BurnBackend, B};
use crate::configs::{
    Activation, AeKind, AeLoss, EpochLog, LayerWeights, SchedulerConfig, TrainConfig,
};
use crate::data;
use crate::models::burn_net::{self, BurnMlp};
use crate::configs::EarlyStoppingConfig;
use crate::scheduler::Scheduler;
use crate::scaler::StandardScaler;
use crate::tensor::Tensor;

/// Combined encoder-decoder module so Burn autodiff sees one graph.
#[derive(Module, Debug)]
pub struct AeNet<B: Backend> {
    pub encoder: BurnMlp<B>,
    pub decoder: BurnMlp<B>,
}

/// Configuration for an AutoEncoder / VAE.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoEncoderConfig {
    pub kind: AeKind,
    pub encoder_sizes: Vec<usize>,
    pub decoder_sizes: Vec<usize>,
    pub latent_dim: usize,
    pub activation: Activation,
    pub dropout: f64,
    pub loss: AeLoss,
    pub beta: f64,
    pub kl_warmup_epochs: usize,
    #[serde(flatten)]
    pub train: TrainConfig,
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

/// Fitted AutoEncoder model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoEncoderModel {
    pub encoder_layers: Vec<LayerWeights>,
    pub decoder_layers: Vec<LayerWeights>,
    pub config: AutoEncoderConfig,
    pub n_features: usize,
    pub scaler: Option<StandardScaler>,
    pub training_log: Vec<EpochLog>,
}

impl AutoEncoderModel {
    pub fn n_params(&self) -> usize {
        self.encoder_layers.iter().map(|l| l.n_params()).sum::<usize>()
            + self.decoder_layers.iter().map(|l| l.n_params()).sum::<usize>()
    }
}

pub struct AutoEncoderTrainOutput {
    pub model: AutoEncoderModel,
    pub latent: Tensor,
    pub training_log: Vec<EpochLog>,
}

pub fn train_autoencoder(
    x: &Tensor,
    _val: Option<(&Tensor,)>,
    config: &AutoEncoderConfig,
) -> Result<AutoEncoderTrainOutput, String> {
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

    let mut enc_sizes = vec![ncols];
    enc_sizes.extend(config.encoder_sizes.iter().copied());
    enc_sizes.push(config.latent_dim);

    let mut dec_sizes = vec![config.latent_dim];
    dec_sizes.extend(config.decoder_sizes.iter().copied());
    dec_sizes.push(ncols);

    let mut net = AeNet::<BurnBackend> {
        encoder: BurnMlp::new(&device, &enc_sizes),
        decoder: BurnMlp::new(&device, &dec_sizes),
    };

    let adam_config = burn_net::create_adam(&config.train.optimizer);
    let mut optim = adam_config.init::<BurnBackend, AeNet<BurnBackend>>();
    let mut sched = Scheduler::new(config.train.scheduler.clone(), config.train.optimizer.lr);
    let mut rng = ChaCha8Rng::seed_from_u64(config.train.seed);
    let batch_size = config.train.batch_size.min(nrows);

    let mut training_log: Vec<EpochLog> = Vec::new();

    for epoch in 0..config.train.n_epochs {
        let mut indices: Vec<usize> = (0..nrows).collect();
        indices.shuffle(&mut rng);

        let mut epoch_loss = 0.0f64;
        let mut n_batches = 0;

        for chunk in indices.chunks(batch_size) {
            let x_batch = data::rows_to_burn(&x_train, chunk, &device);

            let latent = net.encoder.forward(x_batch.clone(), config.activation);
            let recon = net.decoder.forward(latent.clone(), config.activation);

            let recon_loss = burn_net::mse_loss(&recon, &x_batch);

            let total_loss = match config.kind {
                AeKind::Autoencoder => recon_loss,
                AeKind::Vae => {
                    let kl = latent.clone().powi_scalar(2).mean().mul_scalar(0.5);
                    let kl_weight = if epoch < config.kl_warmup_epochs {
                        (epoch as f64 / config.kl_warmup_epochs as f64) * config.beta
                    } else {
                        config.beta
                    };
                    recon_loss.clone().add(kl.mul_scalar(kl_weight))
                }
            };

            epoch_loss += burn_net::read_scalar_1d(&total_loss);
            n_batches += 1;

            let grads = total_loss.backward();
            let grads = GradientsParams::from_grads(grads, &net);
            net = optim.step(config.train.optimizer.lr, net, grads);
        }

        let train_loss = epoch_loss / n_batches as f64;
        let lr = sched.lr();
        training_log.push(EpochLog {
            epoch,
            train_loss,
            val_loss: None,
            val_metric: None,
            lr,
        });
        sched.step();
    }

    let enc_weights = net.encoder.to_weights();
    let dec_weights = net.decoder.to_weights();

    let ae_model = AutoEncoderModel {
        encoder_layers: enc_weights,
        decoder_layers: dec_weights,
        config: config.clone(),
        n_features: ncols,
        scaler,
        training_log: training_log.clone(),
    };

    // Extract latent representations.
    let infer_net = net.valid();
    let x_burn = data::f64_to_burn_infer(&x_train, &device);
    let latent_burn = infer_net.encoder.forward(x_burn, config.activation);
    let latent = data::burn2d_to_tensor(latent_burn);

    Ok(AutoEncoderTrainOutput {
        model: ae_model,
        latent,
        training_log,
    })
}

pub fn predict_autoencoder(model: &mut AutoEncoderModel, x: &Tensor) -> Tensor {
    let device = backend::device();
    let x_scaled = model
        .scaler
        .as_ref()
        .map(|s| s.transform(x))
        .unwrap_or_else(|| x.clone());

    let enc = BurnMlp::<B>::from_weights(&model.encoder_layers, &device);
    let dec = BurnMlp::<B>::from_weights(&model.decoder_layers, &device);
    let x_burn = data::f64_to_burn_infer(&x_scaled, &device);
    let latent = enc.forward(x_burn, model.config.activation);
    let recon = dec.forward(latent, model.config.activation);
    data::burn2d_to_tensor(recon)
}

pub fn encode_autoencoder(model: &mut AutoEncoderModel, x: &Tensor) -> Tensor {
    let device = backend::device();
    let x_scaled = model
        .scaler
        .as_ref()
        .map(|s| s.transform(x))
        .unwrap_or_else(|| x.clone());

    let enc = BurnMlp::<B>::from_weights(&model.encoder_layers, &device);
    let x_burn = data::f64_to_burn_infer(&x_scaled, &device);
    let latent = enc.forward(x_burn, model.config.activation);
    data::burn2d_to_tensor(latent)
}
