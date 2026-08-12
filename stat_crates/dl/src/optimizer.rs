//! Optimizers — SGD, Adam, AdamW, RMSprop.
//!
//! Each optimizer owns per-parameter moment buffers and exposes a single
//! `step` method that receives `&mut [param]` and `&[grad]` slices.
//!
//! The training loop groups all parameters from all layers into flat vectors
//! and calls `step` once per mini-batch.

use rand_chacha::ChaCha8Rng;

use crate::layers::Linear;

// ═══════════════════════════════════════════════════════════════════════
// Optimizer trait
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OptimizerKind {
    Sgd,
    Adam,
    Adamw,
    Rmsprop,
}

impl OptimizerKind {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "sgd" => Some(OptimizerKind::Sgd),
            "adam" => Some(OptimizerKind::Adam),
            "adamw" => Some(OptimizerKind::Adamw),
            "rmsprop" => Some(OptimizerKind::Rmsprop),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OptimizerConfig {
    pub kind: OptimizerKind,
    pub lr: f64,
    pub weight_decay: f64,
    /// SGD momentum.
    pub momentum: f64,
    /// Adam/RMSprop beta1.
    pub beta1: f64,
    /// Adam/RMSprop beta2.
    pub beta2: f64,
    /// Adam/RMSprop epsilon.
    pub eps: f64,
}

impl Default for OptimizerConfig {
    fn default() -> Self {
        Self {
            kind: OptimizerKind::Adam,
            lr: 0.001,
            weight_decay: 0.0,
            momentum: 0.9,
            beta1: 0.9,
            beta2: 0.999,
            eps: 1e-8,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Parameter — a flat view of one trainable tensor
// ═══════════════════════════════════════════════════════════════════════

/// A named parameter vector with its gradient.
/// The training loop collects all parameters into a `Vec<Param>`
/// and the optimizer updates them in-place.
pub struct Param {
    pub value: Vec<f64>,
    pub grad: Vec<f64>,
    /// Momentum / first moment buffer.
    pub m: Vec<f64>,
    /// Second moment buffer.
    pub v: Vec<f64>,
}

impl Param {
    pub fn new(n: usize) -> Self {
        Self {
            value: vec![0.0; n],
            grad: vec![0.0; n],
            m: vec![0.0; n],
            v: vec![0.0; n],
        }
    }

    pub fn len(&self) -> usize {
        self.value.len()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Optimizer state
// ═══════════════════════════════════════════════════════════════════════

pub struct Optimizer {
    pub config: OptimizerConfig,
    pub params: Vec<Param>,
    pub timestep: usize,
}

impl Optimizer {
    /// Create an optimiser with the given config and parameter sizes.
    pub fn new(config: OptimizerConfig, param_sizes: &[usize]) -> Self {
        let params: Vec<Param> = param_sizes.iter().map(|&n| Param::new(n)).collect();
        Self {
            config,
            params,
            timestep: 0,
        }
    }

    /// Take the current parameter values from the layers.
    /// `flat_params` is `[weight_flat, bias, weight_flat, bias, ...]`.
    pub fn read_params(&mut self, flat_params: &[Vec<f64>]) {
        for (i, p) in self.params.iter_mut().enumerate() {
            if let Some(src) = flat_params.get(i) {
                p.value.clone_from(src);
            }
        }
    }

    /// Write updated parameter values back.
    pub fn write_params(&self) -> Vec<Vec<f64>> {
        self.params.iter().map(|p| p.value.clone()).collect()
    }

    /// One optimisation step given the gradients for each parameter group.
    /// `grads[i]` corresponds to parameter group `i`.
    pub fn step(&mut self, grads: &[Vec<f64>]) {
        self.timestep += 1;
        let t = self.timestep;
        let lr = self.config.lr;
        let wd = self.config.weight_decay;
        let beta1 = self.config.beta1;
        let beta2 = self.config.beta2;
        let eps = self.config.eps;
        let momentum = self.config.momentum;

        for (i, param) in self.params.iter_mut().enumerate() {
            let grad = match grads.get(i) {
                Some(g) => g,
                None => continue,
            };

            for j in 0..param.len() {
                let g = grad[j] + wd * param.value[j];

                match self.config.kind {
                    OptimizerKind::Sgd => {
                        param.m[j] = momentum * param.m[j] + g;
                        param.value[j] -= lr * param.m[j];
                    }
                    OptimizerKind::Adam | OptimizerKind::Adamw => {
                        // Adam: L2 weight decay folded into gradient.
                        // AdamW: decoupled weight decay applied separately.
                        let g_eff = if matches!(self.config.kind, OptimizerKind::Adamw) {
                            grad[j]
                        } else {
                            g // includes L2 penalty
                        };
                        param.m[j] = beta1 * param.m[j] + (1.0 - beta1) * g_eff;
                        param.v[j] = beta2 * param.v[j] + (1.0 - beta2) * g_eff * g_eff;
                        let m_hat = param.m[j] / (1.0 - beta1.powi(t as i32));
                        let v_hat = param.v[j] / (1.0 - beta2.powi(t as i32));
                        let update = lr * m_hat / (v_hat.sqrt() + eps);
                        param.value[j] -= update;
                        if matches!(self.config.kind, OptimizerKind::Adamw) {
                            param.value[j] -= lr * wd * param.value[j];
                        }
                    }
                    OptimizerKind::Rmsprop => {
                        param.v[j] = beta2 * param.v[j] + (1.0 - beta2) * g * g;
                        param.value[j] -= lr * g / (param.v[j].sqrt() + eps);
                    }
                }
            }
        }
    }
}

/// Collect all trainable parameters from a slice of `Linear` layers into
/// flat vectors (alternating weight, bias).
pub fn collect_flat_params(layers: &[Linear]) -> Vec<Vec<f64>> {
    let mut out = Vec::new();
    for layer in layers {
        let mut w = Vec::with_capacity(layer.n_in() * layer.n_out());
        for row in &layer.weight {
            w.extend_from_slice(row);
        }
        out.push(w);
        out.push(layer.bias.clone());
    }
    out
}

/// Write flat parameter values back into `Linear` layers.
pub fn scatter_flat_params(layers: &mut [Linear], params: &[Vec<f64>]) {
    for (i, layer) in layers.iter_mut().enumerate() {
        let w_idx = i * 2;
        let b_idx = i * 2 + 1;
        if let Some(w_flat) = params.get(w_idx) {
            for k in 0..layer.n_in() {
                for j in 0..layer.n_out() {
                    layer.weight[k][j] = w_flat[k * layer.n_out() + j];
                }
            }
        }
        if let Some(b) = params.get(b_idx) {
            layer.bias.clone_from(b);
        }
    }
}

/// Collect parameter sizes from `Linear` layers.
pub fn param_sizes(layers: &[Linear]) -> Vec<usize> {
    let mut out = Vec::new();
    for layer in layers {
        out.push(layer.n_in() * layer.n_out());
        out.push(layer.n_out());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layers::InitScheme;
    use rand::SeedableRng;

    #[test]
    fn test_sgd_step() {
        let mut rng = ChaCha8Rng::seed_from_u64(0);
        let layer = Linear::new(2, 1, &mut rng, InitScheme::Xavier);
        let sizes = param_sizes(&[layer.clone()]);
        let mut opt = Optimizer::new(
            OptimizerConfig {
                kind: OptimizerKind::Sgd,
                lr: 0.1,
                momentum: 0.0,
                ..Default::default()
            },
            &sizes,
        );
        opt.read_params(&collect_flat_params(&[layer.clone()]));

        // Gradient = [1, 1, 1] for weight, [1] for bias.
        let grads = vec![vec![1.0; 2], vec![1.0]];
        opt.step(&grads);

        let updated = opt.write_params();
        // All params started at same value, so after one step should decrease by lr*grad.
        let delta = updated[0][0] - layer.weight[0][0];
        assert!((delta - (-0.1)).abs() < 1e-10, "SGD delta = {delta}");
    }

    #[test]
    fn test_adam_step() {
        let mut rng = ChaCha8Rng::seed_from_u64(0);
        let layer = Linear::new(2, 1, &mut rng, InitScheme::Xavier);
        let sizes = param_sizes(&[layer.clone()]);
        let mut opt = Optimizer::new(
            OptimizerConfig {
                kind: OptimizerKind::Adam,
                lr: 0.01,
                ..Default::default()
            },
            &sizes,
        );
        opt.read_params(&collect_flat_params(&[layer.clone()]));

        let grads = vec![vec![1.0; 2], vec![1.0]];
        opt.step(&grads);

        let updated = opt.write_params();
        // After first Adam step, the bias-corrected m_hat = 1.0, v_hat = 1.0.
        // Update = lr * 1.0 / (1.0 + eps) ≈ lr.
        let delta = updated[0][0] - layer.weight[0][0];
        assert!(delta < 0.0, "Adam should decrease param");
    }

    #[test]
    fn test_roundtrip_flat_params() {
        let mut rng = ChaCha8Rng::seed_from_u64(1);
        let mut layer = Linear::new(3, 2, &mut rng, InitScheme::He);
        // Modify some weights.
        layer.weight[0][0] = 42.0;
        layer.bias[1] = 7.0;

        let flat = collect_flat_params(&[layer.clone()]);
        let mut layer2 = Linear::new(3, 2, &mut rng, InitScheme::Xavier);
        let mut layer_arr = [layer2];
        scatter_flat_params(&mut layer_arr, &flat);
        layer2 = layer_arr.into_iter().next().unwrap();
        assert_eq!(layer2.weight[0][0], 42.0);
        assert_eq!(layer2.bias[1], 7.0);
    }
}
