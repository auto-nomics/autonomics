//! Neural network layers — Linear, activations, dropout, batch norm.
//!
//! Each layer stores its own parameters and gradients.  The training loop
//! calls `forward` to compute activations, then `backward` to accumulate
//! parameter gradients, then hands both to the optimizer.
//!
//! All layers operate on **batched** tensors: input shape `(batch, n_in)`,
//! output shape `(batch, n_out)`.

use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

use crate::tensor::Tensor;

// ═══════════════════════════════════════════════════════════════════════
// Activation functions
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Activation {
    Relu,
    Gelu,
    Selu,
    Tanh,
    Sigmoid,
    LeakyRelu,
    Elu,
}

impl Activation {
    pub fn activate(&self, x: f64) -> f64 {
        match self {
            Activation::Relu => x.max(0.0),
            Activation::Gelu => {
                // Approximate GELU
                0.5 * x * (1.0 + (std::f64::consts::FRAC_2_SQRT_PI * (x / std::f64::consts::SQRT_2)).tanh())
            }
            Activation::Selu => {
                let alpha = 1.6732632423543772;
                let scale = 1.0507009873554805;
                if x > 0.0 {
                    scale * x
                } else {
                    scale * alpha * x.exp() - scale * alpha
                }
            }
            Activation::Tanh => x.tanh(),
            Activation::Sigmoid => 1.0 / (1.0 + (-x).exp()),
            Activation::LeakyRelu => {
                if x > 0.0 { x } else { 0.01 * x }
            }
            Activation::Elu => {
                if x > 0.0 { x } else { x.exp() - 1.0 }
            }
        }
    }

    /// Derivative w.r.t. the pre-activation `x`.
    pub fn derivative(&self, x: f64) -> f64 {
        match self {
            Activation::Relu => {
                if x > 0.0 { 1.0 } else { 0.0 }
            }
            Activation::Gelu => {
                let c = std::f64::consts::FRAC_2_SQRT_PI / std::f64::consts::SQRT_2;
                let inner = c * x;
                let tanh_inner = inner.tanh();
                let sech2 = 1.0 - tanh_inner * tanh_inner;
                0.5 * (1.0 + tanh_inner) + 0.5 * x * sech2 * c
            }
            Activation::Selu => {
                let alpha = 1.6732632423543772;
                let scale = 1.0507009873554805;
                if x > 0.0 {
                    scale
                } else {
                    scale * alpha * x.exp()
                }
            }
            Activation::Tanh => {
                let t = x.tanh();
                1.0 - t * t
            }
            Activation::Sigmoid => {
                let s = 1.0 / (1.0 + (-x).exp());
                s * (1.0 - s)
            }
            Activation::LeakyRelu => {
                if x > 0.0 { 1.0 } else { 0.01 }
            }
            Activation::Elu => {
                if x > 0.0 { 1.0 } else { x.exp() }
            }
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "relu" => Some(Activation::Relu),
            "gelu" => Some(Activation::Gelu),
            "selu" => Some(Activation::Selu),
            "tanh" => Some(Activation::Tanh),
            "sigmoid" => Some(Activation::Sigmoid),
            "leaky_relu" => Some(Activation::LeakyRelu),
            "elu" => Some(Activation::Elu),
            _ => None,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Linear layer: y = x @ W + b
// ═══════════════════════════════════════════════════════════════════════

/// Fully-connected layer with weight matrix `W` `(n_in, n_out)` and bias `b` `(n_out,)`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Linear {
    /// Weight matrix, shape `(n_in, n_out)`.
    pub weight: Vec<Vec<f64>>, // row-major [n_in][n_out]
    /// Bias vector, length `n_out`.
    pub bias: Vec<f64>,
    /// Cached input from the last forward pass (for backward).
    #[serde(skip)]
    pub cached_input: Vec<Vec<f64>>, // [batch][n_in]
}

impl Linear {
    /// Create with He or Xavier initialisation.
    pub fn new(n_in: usize, n_out: usize, rng: &mut ChaCha8Rng, init: InitScheme) -> Self {
        let scale = match init {
            InitScheme::He => (2.0 / n_in as f64).sqrt(),
            InitScheme::Xavier => (1.0 / n_in as f64).sqrt(),
            InitScheme::Glorot => (6.0 / (n_in + n_out) as f64).sqrt(),
        };

        let weight: Vec<Vec<f64>> = (0..n_in)
            .map(|_| {
                (0..n_out)
                    .map(|_| rng.random::<f64>() * 2.0 * scale - scale)
                    .collect()
            })
            .collect();

        let bias = vec![0.0; n_out];

        Self {
            weight,
            bias,
            cached_input: Vec::new(),
        }
    }

    pub fn n_in(&self) -> usize {
        self.weight.len()
    }

    pub fn n_out(&self) -> usize {
        self.bias.len()
    }

    /// Forward: `y = x @ W + b`.
    /// Input: `(batch, n_in)`, Output: `(batch, n_out)`.
    pub fn forward(&mut self, x: &Tensor) -> Tensor {
        let (batch, n_in) = x.shape();
        debug_assert_eq!(n_in, self.n_in());
        let n_out = self.n_out();

        // Cache input for backward.
        self.cached_input = (0..batch).map(|i| x.row(i)).collect();

        let mut out = Tensor::zeros(batch, n_out);
        for i in 0..batch {
            for j in 0..n_out {
                let mut sum = self.bias[j];
                for k in 0..n_in {
                    sum += self.cached_input[i][k] * self.weight[k][j];
                }
                out.set(i, j, sum);
            }
        }
        out
    }

    /// Backward: given `grad_output` `(batch, n_out)`, compute
    /// `grad_input` `(batch, n_in)` and accumulate `grad_weight`, `grad_bias`.
    ///
    /// Returns `(grad_input, grad_weight_flat, grad_bias)` where
    /// `grad_weight_flat` is row-major `[n_in * n_out]` matching the order
    /// of `self.weight` flattened.
    pub fn backward(
        &self,
        grad_output: &Tensor,
    ) -> (Tensor, Vec<f64>, Vec<f64>) {
        let (batch, n_out) = grad_output.shape();
        let n_in = self.n_in();

        // grad_input[b][k] = sum_j grad_output[b][j] * W[k][j]
        let mut grad_input = Tensor::zeros(batch, n_in);
        for i in 0..batch {
            for k in 0..n_in {
                let mut sum = 0.0;
                for j in 0..n_out {
                    sum += grad_output.at(i, j) * self.weight[k][j];
                }
                grad_input.set(i, k, sum);
            }
        }

        // grad_weight[k][j] = sum_b x[b][k] * grad_output[b][j]
        let mut grad_weight = vec![0.0; n_in * n_out];
        for i in 0..batch {
            for k in 0..n_in {
                let x_ik = self.cached_input[i][k];
                for j in 0..n_out {
                    grad_weight[k * n_out + j] += x_ik * grad_output.at(i, j);
                }
            }
        }

        // grad_bias[j] = sum_b grad_output[b][j]
        let mut grad_bias = vec![0.0; n_out];
        for i in 0..batch {
            for j in 0..n_out {
                grad_bias[j] += grad_output.at(i, j);
            }
        }

        (grad_input, grad_weight, grad_bias)
    }

    /// Apply a parameter update given flat weight grads and bias grads.
    /// `update_fn` receives `(current_value, grad)` and returns the new value.
    pub fn apply_update<F: Fn(f64, f64) -> f64>(&mut self, grad_weight: &[f64], grad_bias: &[f64], update_fn: &F) {
        let n_in = self.n_in();
        let n_out = self.n_out();
        for k in 0..n_in {
            for j in 0..n_out {
                let idx = k * n_out + j;
                self.weight[k][j] = update_fn(self.weight[k][j], grad_weight[idx]);
            }
        }
        for j in 0..n_out {
            self.bias[j] = update_fn(self.bias[j], grad_bias[j]);
        }
    }

    /// Count total parameters.
    pub fn n_params(&self) -> usize {
        self.n_in() * self.n_out() + self.n_out()
    }
}

#[derive(Debug, Clone, Copy)]
pub enum InitScheme {
    He,
    Xavier,
    Glorot,
}

// ═══════════════════════════════════════════════════════════════════════
// Activation layer (stateless, just applies activation element-wise)
// ═══════════════════════════════════════════════════════════════════════

/// Applies an activation function element-wise.  Stores the pre-activations
/// from the last forward pass for use in backward.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ActivationLayer {
    pub activation: Activation,
    /// Cached pre-activations from forward.
    #[serde(skip)]
    pub cached_pre: Vec<Vec<f64>>,
}

impl ActivationLayer {
    pub fn new(activation: Activation) -> Self {
        Self {
            activation,
            cached_pre: Vec::new(),
        }
    }

    pub fn forward(&mut self, x: &Tensor) -> Tensor {
        let (batch, n) = x.shape();
        self.cached_pre = (0..batch).map(|i| x.row(i)).collect();
        let mut out = Tensor::zeros(batch, n);
        for i in 0..batch {
            for j in 0..n {
                out.set(i, j, self.activation.activate(x.at(i, j)));
            }
        }
        out
    }

    pub fn backward(&self, grad_output: &Tensor) -> Tensor {
        let (batch, n) = grad_output.shape();
        let mut out = Tensor::zeros(batch, n);
        for i in 0..batch {
            for j in 0..n {
                let deriv = self.activation.derivative(self.cached_pre[i][j]);
                out.set(i, j, grad_output.at(i, j) * deriv);
            }
        }
        out
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Dropout layer
// ═══════════════════════════════════════════════════════════════════════

/// Inverted dropout: during training, randomly zero elements with probability
/// `p` and scale survivors by `1/(1-p)`.  At eval time, identity.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Dropout {
    pub p: f64,
    pub training: bool,
    /// Cached dropout mask from forward.
    #[serde(skip)]
    pub mask: Vec<Vec<f64>>,
}

impl Dropout {
    pub fn new(p: f64) -> Self {
        assert!((0.0..=1.0).contains(&p), "dropout p must be in [0, 1]");
        Self {
            p,
            training: true,
            mask: Vec::new(),
        }
    }

    pub fn forward(&mut self, x: &Tensor, rng: &mut ChaCha8Rng) -> Tensor {
        let (batch, n) = x.shape();
        if !self.training || self.p == 0.0 {
            return x.clone();
        }
        let scale = 1.0 / (1.0 - self.p);
        let mut out = Tensor::zeros(batch, n);
        self.mask = vec![vec![0.0; n]; batch];
        for i in 0..batch {
            for j in 0..n {
                if rng.random::<f64>() >= self.p {
                    let m = scale;
                    self.mask[i][j] = m;
                    out.set(i, j, x.at(i, j) * m);
                }
            }
        }
        out
    }

    pub fn backward(&self, grad_output: &Tensor) -> Tensor {
        let (batch, n) = grad_output.shape();
        let mut out = Tensor::zeros(batch, n);
        for i in 0..batch {
            for j in 0..n {
                out.set(i, j, grad_output.at(i, j) * self.mask[i][j]);
            }
        }
        out
    }

    pub fn eval(&mut self) {
        self.training = false;
    }

    pub fn train(&mut self) {
        self.training = true;
    }
}

// ═══════════════════════════════════════════════════════════════════════
// BatchNorm1d layer
// ═══════════════════════════════════════════════════════════════════════

/// Batch normalisation for 1-D inputs (batch, n_features).
///
/// At training time, normalises using batch statistics and updates running
/// estimates.  At eval time, uses running estimates.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BatchNorm1d {
    pub n_features: usize,
    pub gamma: Vec<f64>,  // scale
    pub beta: Vec<f64>,   // shift
    pub momentum: f64,
    pub eps: f64,
    pub running_mean: Vec<f64>,
    pub running_var: Vec<f64>,
    pub training: bool,
    /// Cached values from forward for backward.
    #[serde(skip)]
    pub cached: BatchNormCache,
}

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct BatchNormCache {
    pub batch_mean: Vec<f64>,
    pub batch_var: Vec<f64>,
    pub x_centered: Vec<Vec<f64>>,  // x - mean
    pub x_norm: Vec<Vec<f64>>,      // (x - mean) / sqrt(var + eps)
    pub std: Vec<f64>,              // sqrt(var + eps)
    pub batch_size: usize,
}

impl BatchNorm1d {
    pub fn new(n_features: usize, rng: &mut ChaCha8Rng) -> Self {
        // Initialise gamma ~ N(1, 0.02), beta ~ N(0, 0.02) — slight noise
        // avoids exact identity start which can stall gradients.
        let gamma = (0..n_features)
            .map(|_| 1.0 + rng.random::<f64>() * 0.04 - 0.02)
            .collect();
        let beta = (0..n_features)
            .map(|_| rng.random::<f64>() * 0.04 - 0.02)
            .collect();
        Self {
            n_features,
            gamma,
            beta,
            momentum: 0.1,
            eps: 1e-5,
            running_mean: vec![0.0; n_features],
            running_var: vec![1.0; n_features],
            training: true,
            cached: BatchNormCache::default(),
        }
    }

    pub fn forward(&mut self, x: &Tensor) -> Tensor {
        let (batch, n) = x.shape();
        debug_assert_eq!(n, self.n_features);

        let mut out = Tensor::zeros(batch, n);

        if self.training {
            // Compute batch statistics.
            let mut batch_mean = vec![0.0; n];
            for j in 0..n {
                for i in 0..batch {
                    batch_mean[j] += x.at(i, j);
                }
                batch_mean[j] /= batch as f64;
            }

            let mut batch_var = vec![0.0; n];
            for j in 0..n {
                for i in 0..batch {
                    let d = x.at(i, j) - batch_mean[j];
                    batch_var[j] += d * d;
                }
                batch_var[j] /= batch as f64;
            }

            let std: Vec<f64> = batch_var.iter().map(|&v| (v + self.eps).sqrt()).collect();

            let mut x_centered = vec![vec![0.0; n]; batch];
            let mut x_norm = vec![vec![0.0; n]; batch];
            for i in 0..batch {
                for j in 0..n {
                    x_centered[i][j] = x.at(i, j) - batch_mean[j];
                    x_norm[i][j] = x_centered[i][j] / std[j];
                    out.set(i, j, self.gamma[j] * x_norm[i][j] + self.beta[j]);
                }
            }

            // Update running estimates.
            for j in 0..n {
                self.running_mean[j] = (1.0 - self.momentum) * self.running_mean[j]
                    + self.momentum * batch_mean[j];
                self.running_var[j] = (1.0 - self.momentum) * self.running_var[j]
                    + self.momentum * batch_var[j];
            }

            self.cached = BatchNormCache {
                batch_mean,
                batch_var,
                x_centered,
                x_norm,
                std,
                batch_size: batch,
            };
        } else {
            // Use running estimates.
            for i in 0..batch {
                for j in 0..n {
                    let xn = (x.at(i, j) - self.running_mean[j])
                        / (self.running_var[j] + self.eps).sqrt();
                    out.set(i, j, self.gamma[j] * xn + self.beta[j]);
                }
            }
        }

        out
    }

    /// Backward: returns `(grad_input, grad_gamma, grad_beta)`.
    pub fn backward(&self, grad_output: &Tensor) -> (Tensor, Vec<f64>, Vec<f64>) {
        let (batch, n) = grad_output.shape();
        let m = batch as f64;

        let mut grad_input = Tensor::zeros(batch, n);
        let mut grad_gamma = vec![0.0; n];
        let mut grad_beta = vec![0.0; n];

        for j in 0..n {
            for i in 0..batch {
                grad_gamma[j] += grad_output.at(i, j) * self.cached.x_norm[i][j];
                grad_beta[j] += grad_output.at(i, j);
            }
        }

        let std = &self.cached.std;
        for j in 0..n {
            let inv_std = 1.0 / std[j];
            let dvar = (0..batch)
                .map(|i| {
                    grad_output.at(i, j) * self.gamma[j] * self.cached.x_centered[i][j]
                        * -0.5
                        * (self.cached.batch_var[j] + self.eps).powf(-1.5)
                })
                .sum::<f64>();

            let dmean = (0..batch)
                .map(|i| {
                    grad_output.at(i, j) * self.gamma[j] * -inv_std
                        + dvar * -2.0 * self.cached.x_centered[i][j] / m
                })
                .sum::<f64>();

            for i in 0..batch {
                let dx = grad_output.at(i, j) * self.gamma[j] * inv_std
                    + dvar * 2.0 * self.cached.x_centered[i][j] / m
                    + dmean / m;
                grad_input.set(i, j, dx);
            }
        }

        (grad_input, grad_gamma, grad_beta)
    }

    pub fn eval(&mut self) {
        self.training = false;
    }

    pub fn train(&mut self) {
        self.training = true;
    }

    pub fn n_params(&self) -> usize {
        2 * self.n_features
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rng() -> ChaCha8Rng {
        ChaCha8Rng::seed_from_u64(42)
    }

    #[test]
    fn test_linear_forward_shape() {
        let mut layer = Linear::new(3, 2, &mut rng(), InitScheme::He);
        let x = Tensor::from_rows(4, 3, &[1.0; 12]);
        let y = layer.forward(&x);
        assert_eq!(y.shape(), (4, 2));
    }

    #[test]
    fn test_linear_backward_grad_shape() {
        let mut layer = Linear::new(3, 2, &mut rng(), InitScheme::He);
        let x = Tensor::from_rows(4, 3, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0]);
        let _y = layer.forward(&x);
        let grad_out = Tensor::from_rows(4, 2, &[1.0; 8]);
        let (grad_in, gw, gb) = layer.backward(&grad_out);
        assert_eq!(grad_in.shape(), (4, 3));
        assert_eq!(gw.len(), 6);
        assert_eq!(gb.len(), 2);
    }

    #[test]
    fn test_linear_gradient_check() {
        // Numerical gradient check on a small linear layer.
        let mut rng_local = rng();
        let mut layer = Linear::new(2, 3, &mut rng_local, InitScheme::Xavier);
        let x = Tensor::from_rows(2, 2, &[1.0, 2.0, 3.0, 4.0]);

        let y = layer.forward(&x);
        // Loss = sum(y^2), so dL/dy = 2*y
        let grad_out = Tensor::from_rows(
            2,
            3,
            &[y.at(0,0)*2.0, y.at(0,1)*2.0, y.at(0,2)*2.0,
              y.at(1,0)*2.0, y.at(1,1)*2.0, y.at(1,2)*2.0],
        );
        let (_, grad_w_analytic, _) = layer.backward(&grad_out);

        // Numerical gradient for weight[0][0].
        let eps = 1e-6;
        layer.weight[0][0] += eps;
        let y_plus = layer.forward(&x);
        let mut loss_plus = 0.0;
        for i in 0..2 {
            for j in 0..3 {
                loss_plus += y_plus.at(i, j).powi(2);
            }
        }
        layer.weight[0][0] -= 2.0 * eps;
        let y_minus = layer.forward(&x);
        let mut loss_minus = 0.0;
        for i in 0..2 {
            for j in 0..3 {
                loss_minus += y_minus.at(i, j).powi(2);
            }
        }
        layer.weight[0][0] += eps; // restore

        let numeric_grad = (loss_plus - loss_minus) / (2.0 * eps);
        let analytic_grad = grad_w_analytic[0];
        assert!(
            (numeric_grad - analytic_grad).abs() / (numeric_grad.abs() + 1e-10) < 1e-4,
            "grad mismatch: numeric={numeric_grad:.8}, analytic={analytic_grad:.8}"
        );
    }

    #[test]
    fn test_activation_relu() {
        let act = Activation::Relu;
        assert_eq!(act.activate(2.0), 2.0);
        assert_eq!(act.activate(-1.0), 0.0);
        assert_eq!(act.derivative(2.0), 1.0);
        assert_eq!(act.derivative(-1.0), 0.0);
    }

    #[test]
    fn test_activation_sigmoid() {
        let act = Activation::Sigmoid;
        let s = act.activate(0.0);
        assert!((s - 0.5).abs() < 1e-10);
        let d = act.derivative(0.0);
        assert!((d - 0.25).abs() < 1e-10);
    }

    #[test]
    fn test_activation_tanh() {
        let act = Activation::Tanh;
        assert!((act.activate(0.0)).abs() < 1e-10);
        assert!((act.derivative(0.0) - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_dropout_eval_is_identity() {
        let mut rng_local = rng();
        let mut drop = Dropout::new(0.5);
        drop.eval();
        let x = Tensor::from_rows(4, 3, &[1.0; 12]);
        let y = drop.forward(&x, &mut rng_local);
        for i in 0..4 {
            for j in 0..3 {
                assert_eq!(y.at(i, j), 1.0);
            }
        }
    }

    #[test]
    fn test_dropout_train_scales() {
        let mut rng_local = rng();
        let mut drop = Dropout::new(0.0); // p=0 → no dropout
        drop.train();
        let x = Tensor::from_rows(2, 2, &[1.0, 2.0, 3.0, 4.0]);
        let y = drop.forward(&x, &mut rng_local);
        assert_eq!(y.at(0, 0), 1.0);
        assert_eq!(y.at(1, 1), 4.0);
    }

    #[test]
    fn test_batchnorm_eval_uses_running_stats() {
        let mut rng_local = rng();
        let mut bn = BatchNorm1d::new(2, &mut rng_local);
        bn.eval();
        let x = Tensor::from_rows(2, 2, &[0.0, 0.0, 0.0, 0.0]);
        let y = bn.forward(&x);
        // With running_mean=0, running_var=1, gamma≈1, beta≈0: y ≈ 0.
        for i in 0..2 {
            for j in 0..2 {
                assert!(y.at(i, j).abs() < 0.1, "y[{i}][{j}] = {}", y.at(i, j));
            }
        }
    }

    #[test]
    fn test_batchnorm_train_normalises() {
        let mut rng_local = rng();
        let mut bn = BatchNorm1d::new(2, &mut rng_local);
        bn.gamma = vec![1.0, 1.0];
        bn.beta = vec![0.0, 0.0];
        let x = Tensor::from_rows(4, 2, &[1.0, 10.0, 2.0, 20.0, 3.0, 30.0, 4.0, 40.0]);
        let y = bn.forward(&x);
        // Each output column should have ≈ 0 mean.
        for j in 0..2 {
            let mean: f64 = (0..4).map(|i| y.at(i, j)).sum::<f64>() / 4.0;
            assert!(mean.abs() < 1e-10, "bn output col {j} mean = {mean}");
        }
    }
}
