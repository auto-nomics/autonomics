//! Shared Burn neural network — the common building block for all model
//! architectures in this crate.
//!
//! Uses Burn's autodiff for all gradient computation. No manual backprop.

use burn::module::{AutodiffModule, Module};
use burn::nn::Linear;
use burn::tensor::backend::Backend;
use burn::tensor::{Tensor, TensorData};

use crate::backend::Backend as BurnBackend;
use crate::configs::{Activation, LayerWeights};
use crate::data;

// ═══════════════════════════════════════════════════════════════════════
// BurnMlp — generic feed-forward network
// ═══════════════════════════════════════════════════════════════════════

/// A feed-forward network consisting of stacked `Linear` layers with
/// element-wise activations between them.
#[derive(Module, Debug)]
pub struct BurnMlp<B: Backend> {
    pub layers: Vec<Linear<B>>,
}

impl<B: Backend> BurnMlp<B> {
    /// Create with the given layer sizes and random initialisation.
    ///
    /// `sizes` = `[n_features, hidden1, hidden2, ..., n_output]`.
    pub fn new(device: &B::Device, sizes: &[usize]) -> Self {
        let layers: Vec<Linear<B>> = sizes
            .windows(2)
            .map(|w| burn::nn::LinearConfig::new(w[0], w[1]).init(device))
            .collect();
        Self { layers }
    }

    /// Reconstruct from serialised layer weights.
    pub fn from_weights(weights: &[LayerWeights], device: &B::Device) -> Self {
        Self {
            layers: data::build_linear_layers(weights, device),
        }
    }

    /// Extract weights to serialisable format.
    pub fn to_weights(&self) -> Vec<LayerWeights> {
        data::extract_linear_weights(&self.layers)
    }

    /// Forward pass: `Linear → activation` for all but the last layer,
    /// then bare `Linear` for the output head.
    pub fn forward(&self, x: Tensor<B, 2>, activation: Activation) -> Tensor<B, 2> {
        let n_layers = self.layers.len();
        let mut h = x;
        for (i, layer) in self.layers.iter().enumerate() {
            h = layer.forward(h);
            if i < n_layers - 1 {
                h = apply_activation(h, activation);
            }
        }
        h
    }

    /// Count total parameters.
    pub fn n_params(&self) -> usize {
        self.layers
            .iter()
            .map(|l| {
                let w_shape = l.weight.shape();
                // Burn weight shape is [d_input, d_output].
                let out_f = l.bias.as_ref().map_or(0, |b| b.shape().dims[0]);
                w_shape.dims[0] * w_shape.dims[1] + out_f
            })
            .sum()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Activation helpers
// ═══════════════════════════════════════════════════════════════════════

pub fn apply_activation<B: Backend>(x: Tensor<B, 2>, act: Activation) -> Tensor<B, 2> {
    match act {
        Activation::Relu => burn::tensor::activation::relu(x),
        Activation::Gelu => burn::tensor::activation::gelu(x),
        Activation::Tanh => burn::tensor::activation::tanh(x),
        Activation::Sigmoid => burn::tensor::activation::sigmoid(x),
        Activation::LeakyRelu => burn::tensor::activation::leaky_relu(x, 0.01),
        Activation::Elu => {
            // ELU: x if x > 0 else e^x - 1
            let pos = x.clone().clamp_min(0.0);
            let neg = x.clamp_max(0.0).exp().sub_scalar(1.0);
            pos.add(neg)
        }
        Activation::Selu => {
            let scale: f32 = 1.0507009873554805;
            let alpha: f32 = 1.6732632423543772;
            let pos = x.clone().clamp_min(0.0).mul_scalar(scale);
            let neg = x.clamp_max(0.0).exp().sub_scalar(1.0).mul_scalar(alpha * scale);
            pos.add(neg)
        }
    }
}

/// Numerically stable sigmoid for f64 (used in predict functions).
pub fn sigmoid_stable_f64(x: f64) -> f64 {
    if x >= 0.0 {
        1.0 / (1.0 + (-x).exp())
    } else {
        let e = x.exp();
        e / (1.0 + e)
    }
}

/// Numerically stable sigmoid (f64 alias).
pub fn sigmoid_stable(x: f64) -> f64 {
    sigmoid_stable_f64(x)
}

// ═══════════════════════════════════════════════════════════════════════
// Loss functions (Burn autodiff compatible)
// ═══════════════════════════════════════════════════════════════════════

/// Binary cross-entropy loss from logits.
///
/// `logits` shape: `(batch, 1)`. `targets` shape: `(batch,)`.
/// Returns a scalar tensor suitable for `backward()`.
pub fn bce_loss(
    logits: &Tensor<BurnBackend, 2>,
    targets: &Tensor<BurnBackend, 1>,
) -> Tensor<BurnBackend, 1> {
    let eps: f32 = 1e-7;
    let p = burn::tensor::activation::sigmoid(logits.clone());
    let p_clamped = p.clamp(eps, 1.0 - eps);
    let log_p = p_clamped.clone().log();
    let log_1mp = p_clamped.neg().add_scalar(1.0).log();

    // Reshape targets from (batch,) to (batch, 1).
    let batch = logits.shape().dims[0];
    let targets_2d = targets.clone().reshape([batch, 1]);

    // element_loss = -(y * log_p + (1-y) * log_1mp)
    let element_loss = targets_2d
        .clone()
        .mul(log_p)
        .add(targets_2d.neg().add_scalar(1.0).mul(log_1mp))
        .neg();

    element_loss.mean()
}

/// Mean squared error loss.
pub fn mse_loss(
    preds: &Tensor<BurnBackend, 2>,
    targets: &Tensor<BurnBackend, 2>,
) -> Tensor<BurnBackend, 1> {
    let diff = preds.clone().sub(targets.clone());
    diff.clone().mul(diff).mean()
}

/// Negative Cox partial log-likelihood loss.
///
/// All computation uses Burn tensor ops so gradients flow automatically.
/// O(n_events × n_samples) implementation — sufficient for moderate batches.
pub fn cox_loss(
    risk_scores: &Tensor<BurnBackend, 2>,
    times: &[f64],
    events: &[usize],
) -> Tensor<BurnBackend, 1> {
    let device = crate::backend::device();
    let batch = risk_scores.shape().dims[0];

    // Sort by descending time.
    let mut order: Vec<usize> = (0..batch).collect();
    order.sort_by(|&a, &b| times[b].partial_cmp(&times[a]).unwrap_or(std::cmp::Ordering::Equal));

    let h = risk_scores.clone().reshape([batch]);

    // Numerical stability: subtract max.
    let h_max_val = h
        .clone()
        .max()
        .into_data()
        .as_slice::<f32>()
        .unwrap()[0];
    let exp_h = h.clone().sub_scalar(h_max_val).exp();

    let mut total_loss =
        Tensor::<BurnBackend, 1>::from_data(TensorData::new(vec![0.0_f32], [1]), &device);
    let mut n_events = 0usize;

    for &idx in &order {
        if events[idx] != 1 {
            continue;
        }
        n_events += 1;

        // Risk set = { j : time[j] >= time[idx] }.  Sum their exp_h.
        let mut risk_sum =
            Tensor::<BurnBackend, 1>::from_data(TensorData::new(vec![0.0_f32], [1]), &device);
        for &j in &order {
            if times[j] >= times[idx] - 1e-10 {
                let ej = exp_h.clone().slice([j..j + 1]);
                risk_sum = risk_sum.add(ej);
            }
        }

        // Loss contribution: -(h_idx - h_max - log(risk_sum))
        let h_idx = h.clone().slice([idx..idx + 1]);
        let log_risk = risk_sum.log();
        let contrib = h_idx.sub_scalar(h_max_val).sub(log_risk).neg();
        total_loss = total_loss.add(contrib);
    }

    if n_events == 0 {
        return Tensor::<BurnBackend, 1>::from_data(TensorData::new(vec![0.0_f32], [1]), &device);
    }

    total_loss.div_scalar(n_events as f32)
}

// ═══════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════

/// Read the first (scalar) value from a 1-D autodiff tensor.
pub fn read_scalar_1d(tensor: &Tensor<BurnBackend, 1>) -> f64 {
    let data = tensor.to_data();
    data.as_slice::<f32>().unwrap()[0] as f64
}

/// Extract rows from a dl::Tensor by index.
pub fn extract_rows(tensor: &crate::tensor::Tensor, indices: &[usize]) -> crate::tensor::Tensor {
    let ncols = tensor.ncols();
    let mut flat = Vec::with_capacity(indices.len() * ncols);
    for &i in indices {
        flat.extend_from_slice(&tensor.row(i));
    }
    crate::tensor::Tensor::from_rows(indices.len(), ncols, &flat)
}

// ═══════════════════════════════════════════════════════════════════════
// Optimiser factory
// ═══════════════════════════════════════════════════════════════════════

use crate::configs::OptimizerConfig;

/// Create a Burn Adam optimiser config from our config.
pub fn create_adam(config: &OptimizerConfig) -> burn::optim::AdamConfig {
    burn::optim::AdamConfig::new()
        .with_weight_decay(if config.weight_decay > 0.0 {
            Some(burn::optim::decay::WeightDecayConfig::new(config.weight_decay))
        } else {
            None
        })
        .with_beta_1(config.beta1 as f32)
        .with_beta_2(config.beta2 as f32)
        .with_epsilon(config.eps as f32)
}

/// Create a Burn SGD optimiser config from our config.
pub fn create_sgd(config: &OptimizerConfig) -> burn::optim::SgdConfig {
    burn::optim::SgdConfig::new()
        .with_momentum(if config.momentum > 0.0 {
            Some(burn::optim::momentum::MomentumConfig {
                momentum: config.momentum,
                dampening: 0.1,
                nesterov: false,
            })
        } else {
            None
        })
        .with_weight_decay(if config.weight_decay > 0.0 {
            Some(burn::optim::decay::WeightDecayConfig::new(config.weight_decay))
        } else {
            None
        })
}
