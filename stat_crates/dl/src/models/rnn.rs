//! RNN / LSTM / GRU — recurrent neural networks for longitudinal/sequence data.
//!
//! Simplified Burn-based implementation: treats each sequence as a flat feature
//! vector and processes through an MLP. Full RNN cell implementations can be
//! added later; the Burn autodiff infrastructure handles all backward passes.

use burn::optim::{Adam, GradientsParams, Optimizer};
use burn::module::AutodiffModule;
use rand::SeedableRng;
use rand::seq::SliceRandom;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::backend::{self, Backend, B};
use crate::configs::{
    Activation, CellType, EpochLog, LayerWeights, OptimizerConfig, SchedulerConfig, SeqPooling,
    TaskType, TrainConfig,
};
use crate::data;
use crate::models::burn_net::{self, BurnMlp};
use crate::configs::EarlyStoppingConfig;
use crate::scheduler::Scheduler;
use crate::scaler::StandardScaler;
use crate::tensor::Tensor;

/// Configuration for RNN/LSTM/GRU.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RnnConfig {
    pub cell_type: CellType,
    pub hidden_size: usize,
    pub n_layers: usize,
    pub bidirectional: bool,
    pub dropout: f64,
    pub pooling: SeqPooling,
    pub task_type: TaskType,
    #[serde(flatten)]
    pub train: TrainConfig,
}

impl Default for RnnConfig {
    fn default() -> Self {
        Self {
            cell_type: CellType::Lstm,
            hidden_size: 64,
            n_layers: 1,
            bidirectional: false,
            dropout: 0.0,
            pooling: SeqPooling::Last,
            task_type: TaskType::Classification,
            train: TrainConfig::default(),
        }
    }
}

/// Fitted RNN model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RnnModel {
    pub layers: Vec<LayerWeights>,
    pub config: RnnConfig,
    pub n_features: usize,
    pub n_seq_features: usize,
    pub n_static_features: usize,
    pub seq_len: usize,
    pub scaler: Option<StandardScaler>,
    pub training_log: Vec<EpochLog>,
}

impl RnnModel {
    pub fn n_params(&self) -> usize {
        self.layers.iter().map(|l| l.n_params()).sum()
    }
}

pub struct RnnTrainOutput {
    pub model: RnnModel,
    pub predictions: Tensor,
    pub training_log: Vec<EpochLog>,
}

pub fn train_rnn(
    x: &Tensor,
    y: &Tensor,
    _val: Option<(&Tensor, &Tensor)>,
    config: &RnnConfig,
    n_seq_features: usize,
    n_static_features: usize,
    seq_len: usize,
) -> Result<RnnTrainOutput, String> {
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

    // Simplified architecture: treat all features as flat input → hidden → output.
    let hidden_mult = if config.bidirectional { 2 } else { 1 };
    let mut sizes = vec![ncols];
    for _ in 0..config.n_layers {
        sizes.push(config.hidden_size * hidden_mult);
    }
    sizes.push(1);

    let mut model = BurnMlp::<Backend>::new(&device, &sizes);
    let adam_config = burn_net::create_adam(&config.train.optimizer);
    let mut optim = adam_config.init::<Backend, BurnMlp<Backend>>();
    let mut sched = Scheduler::new(config.train.scheduler.clone(), config.train.optimizer.lr);
    let mut rng = ChaCha8Rng::seed_from_u64(config.train.seed);
    let batch_size = config.train.batch_size.min(nrows);

    let mut training_log: Vec<EpochLog> = Vec::new();
    let activation = Activation::Relu;

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

    let weights = model.to_weights();
    let rnn_model = RnnModel {
        layers: weights,
        config: config.clone(),
        n_features: ncols,
        n_seq_features,
        n_static_features,
        seq_len,
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

    Ok(RnnTrainOutput {
        model: rnn_model,
        predictions,
        training_log,
    })
}

pub fn predict_rnn(model: &mut RnnModel, x: &Tensor) -> Tensor {
    let device = backend::device();
    let x_scaled = model
        .scaler
        .as_ref()
        .map(|s| s.transform(x))
        .unwrap_or_else(|| x.clone());

    let infer_model = BurnMlp::<B>::from_weights(&model.layers, &device);
    let x_burn = data::f64_to_burn_infer(&x_scaled, &device);
    let raw = infer_model.forward(x_burn, Activation::Relu);

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
