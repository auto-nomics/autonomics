//! DeepHit — discrete-time neural network for survival with competing risks.
//!
//! Simplified implementation: single shared MLP with a softmax output over
//! discrete time bins. The NLL loss pushes probability mass toward the
//! correct bin for each event sample.

use burn::module::{AutodiffModule, Module};
use burn::nn::Linear;
use burn::optim::{GradientsParams, Optimizer};
use burn::tensor::Tensor as BurnTensor;
use burn::tensor::TensorData;
use burn::tensor::backend::Backend;
use rand::SeedableRng;
use rand::seq::SliceRandom;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::backend::{self, Backend as BurnBackend, B};
use crate::configs::{
    Activation, EpochLog, LayerWeights, SchedulerConfig, TrainConfig,
};
use crate::configs::EarlyStoppingConfig;
use crate::data;
use crate::models::burn_net::{self, BurnMlp};
use crate::scheduler::Scheduler;
use crate::scaler::StandardScaler;
use crate::survival::TimeBins;
use crate::tensor::Tensor;

/// Combined network so Burn autodiff sees one graph.
#[derive(Module, Debug)]
pub struct DeepHitNet<B: Backend> {
    pub shared: BurnMlp<B>,
    pub head: BurnMlp<B>,
}

/// Configuration for DeepHit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeepHitConfig {
    pub hidden_sizes: Vec<usize>,
    pub activation: Activation,
    pub dropout: f64,
    pub n_time_bins: usize,
    pub time_bins_method: String,
    pub n_causes: usize,
    pub loss_alpha: f64,
    pub loss_beta: f64,
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
    pub shared_layers: Vec<LayerWeights>,
    pub head_layers: Vec<LayerWeights>,
    pub time_bins: TimeBins,
    pub config: DeepHitConfig,
    pub n_features: usize,
    pub scaler: Option<StandardScaler>,
    pub training_log: Vec<EpochLog>,
}

impl DeepHitModel {
    pub fn n_params(&self) -> usize {
        self.shared_layers.iter().map(|l| l.n_params()).sum::<usize>()
            + self.head_layers.iter().map(|l| l.n_params()).sum::<usize>()
    }
}

pub struct DeepHitTrainOutput {
    pub model: DeepHitModel,
    pub risk_scores: Tensor,
    pub training_log: Vec<EpochLog>,
}

pub fn train_deephit(
    x: &Tensor,
    times: &[f64],
    events: &[usize],
    _val: Option<(&Tensor, &[f64], &[usize])>,
    config: &DeepHitConfig,
) -> Result<DeepHitTrainOutput, String> {
    let device = backend::device();
    let (nrows, ncols) = x.shape();
    if nrows == 0 {
        return Err("empty training data".into());
    }

    let time_bins = TimeBins::fit(times, events, config.n_time_bins, &config.time_bins_method);

    let scaler = if config.train.standardize {
        Some(StandardScaler::fit(x))
    } else {
        None
    };
    let x_train = scaler
        .as_ref()
        .map(|s| s.transform(x))
        .unwrap_or_else(|| x.clone());

    let last_hidden = *config.hidden_sizes.last().unwrap_or(&32);

    let mut net = DeepHitNet::<BurnBackend> {
        shared: BurnMlp::new(&device, &{
            let mut s = vec![ncols];
            s.extend(config.hidden_sizes.iter().copied());
            s
        }),
        head: BurnMlp::new(&device, &[last_hidden, config.n_time_bins]),
    };

    let adam_config = burn_net::create_adam(&config.train.optimizer);
    let mut optim = adam_config.init::<BurnBackend, DeepHitNet<BurnBackend>>();
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
            let hidden = net.shared.forward(x_batch, config.activation);
            let logits = net.head.forward(hidden, Activation::Relu);
            let probs = burn::tensor::activation::softmax(logits, 1);

            // NLL loss for event samples.
            let chunk_len = chunk.len();
            let mut nll = BurnTensor::<BurnBackend, 1>::from_data(
                TensorData::new(vec![0.0_f32], [1]),
                &device,
            );

            for (local_i, &global_i) in chunk.iter().enumerate() {
                if events[global_i] == 0 {
                    continue;
                }
                if let Some(bin) = time_bins.bin_of(times[global_i]) {
                    let prob = probs
                        .clone()
                        .slice([local_i..local_i + 1, bin..bin + 1]);
                    let log_p = prob.clamp(1e-8_f32, 1.0).log();
                    nll = nll.add(log_p.neg().squeeze(1));
                }
            }

            let loss = nll.div_scalar(chunk_len as f32);
            epoch_loss += burn_net::read_scalar_1d(&loss);
            n_batches += 1;

            let grads = loss.backward();
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

    let shared_weights = net.shared.to_weights();
    let head_weights = net.head.to_weights();

    let model = DeepHitModel {
        shared_layers: shared_weights,
        head_layers: head_weights,
        time_bins: time_bins.clone(),
        config: config.clone(),
        n_features: ncols,
        scaler,
        training_log: training_log.clone(),
    };

    // Compute training risk scores (negative expected time → higher risk for earlier events).
    let infer_net = net.valid();
    let x_burn = data::f64_to_burn_infer(&x_train, &device);
    let hidden = infer_net.shared.forward(x_burn, config.activation);
    let logits = infer_net.head.forward(hidden, Activation::Relu);
    let probs = burn::tensor::activation::softmax(logits, 1);
    let p_data = probs.to_data();
    let p_slice = p_data.as_slice::<f32>().unwrap();
    let n_bins = config.n_time_bins;

    let mut risk_scores = Tensor::zeros(nrows, 1);
    for i in 0..nrows {
        let mut expected_time = 0.0f64;
        for b in 0..n_bins {
            let midpoint = time_bins.bin_midpoint(b);
            expected_time += p_slice[i * n_bins + b] as f64 * midpoint;
        }
        risk_scores.set(i, 0, -expected_time);
    }

    Ok(DeepHitTrainOutput {
        model,
        risk_scores,
        training_log,
    })
}

pub fn predict_deephit(model: &mut DeepHitModel, x: &Tensor) -> Tensor {
    let device = backend::device();
    let x_scaled = model
        .scaler
        .as_ref()
        .map(|s| s.transform(x))
        .unwrap_or_else(|| x.clone());

    let shared = BurnMlp::<B>::from_weights(&model.shared_layers, &device);
    let head = BurnMlp::<B>::from_weights(&model.head_layers, &device);
    let x_burn = data::f64_to_burn_infer(&x_scaled, &device);
    let hidden = shared.forward(x_burn, model.config.activation);
    let logits = head.forward(hidden, Activation::Relu);
    let probs = burn::tensor::activation::softmax(logits, 1);
    let p_data = probs.to_data();
    let p_slice = p_data.as_slice::<f32>().unwrap();
    let n = x_scaled.nrows();
    let n_bins = model.config.n_time_bins;

    let mut risk_scores = Tensor::zeros(n, 1);
    for i in 0..n {
        let mut expected_time = 0.0f64;
        for b in 0..n_bins {
            let midpoint = model.time_bins.bin_midpoint(b);
            expected_time += p_slice[i * n_bins + b] as f64 * midpoint;
        }
        risk_scores.set(i, 0, -expected_time);
    }

    risk_scores
}
