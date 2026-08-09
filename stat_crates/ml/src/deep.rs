//! Deep learning — MLP (multilayer perceptron) implemented natively in faer.
//!
//! Provides forward/backward passes, ReLU/Sigmoid activations, SGD training.
//! Avoids the massive burn dependency tree while covering the core use case.

use faer::Mat;
use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DeepError {
    #[error("empty input")]
    Empty,
    #[error("architecture mismatch: expected {expected} inputs, got {got}")]
    ArchMismatch { expected: usize, got: usize },
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, DeepError>;

#[derive(Debug, Clone, Copy)]
pub enum Activation {
    ReLU,
    Sigmoid,
    Tanh,
}

impl Activation {
    fn activate(&self, x: f64) -> f64 {
        match self {
            Activation::ReLU => x.max(0.0),
            Activation::Sigmoid => 1.0 / (1.0 + (-x).exp()),
            Activation::Tanh => x.tanh(),
        }
    }

    fn derivative(&self, x: f64) -> f64 {
        match self {
            Activation::ReLU => {
                if x > 0.0 {
                    1.0
                } else {
                    0.0
                }
            }
            Activation::Sigmoid => {
                let s = 1.0 / (1.0 + (-x).exp());
                s * (1.0 - s)
            }
            Activation::Tanh => {
                let t = x.tanh();
                1.0 - t * t
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Multilayer Perceptron (MLP)
// ═══════════════════════════════════════════════════════════════════════

pub struct MlpModel {
    /// Weight matrices per layer: [n_prev+1, n_curr] (including bias row)
    pub weights: Vec<Mat<f64>>,
    pub activations: Vec<Activation>,
    pub n_features: usize,
}

/// MLP training options.
pub struct MlpOptions {
    pub hidden_sizes: Vec<usize>,
    pub activation: Activation,
    pub learning_rate: f64,
    pub n_epochs: usize,
    pub seed: u64,
}

/// Fit an MLP for binary classification (output = sigmoid → probability).
pub fn mlp_fit(data: &Mat<f64>, labels: &[usize], opts: &MlpOptions) -> Result<MlpModel> {
    let (nrows, ncols) = data.shape();
    if nrows == 0 {
        return Err(DeepError::Empty);
    }

    let mut rng = ChaCha8Rng::seed_from_u64(opts.seed);

    // Build layer sizes: [n_features, h1, h2, ..., 1]
    let mut layer_sizes = vec![ncols];
    layer_sizes.extend(opts.hidden_sizes.iter());
    layer_sizes.push(1); // binary output

    // Initialize weights: each layer has [prev_size+1, curr_size] (bias row)
    let mut weights: Vec<Mat<f64>> = Vec::new();
    for l in 0..layer_sizes.len() - 1 {
        let n_in = layer_sizes[l] + 1; // +1 for bias
        let n_out = layer_sizes[l + 1];
        let w = Mat::from_fn(n_in, n_out, |_, _| {
            rng.random::<f64>() * 0.2 - 0.1 // small random init
        });
        weights.push(w);
    }

    let acts = vec![opts.activation; layer_sizes.len() - 2]; // hidden layers
    let mut activations = acts;
    activations.push(Activation::Sigmoid); // output layer

    // Training loop (full-batch SGD)
    for _epoch in 0..opts.n_epochs {
        // Forward pass for all samples
        let mut all_activations: Vec<Vec<Vec<f64>>> = Vec::new(); // [layer][sample][neuron]
        let mut all_zs: Vec<Vec<Vec<f64>>> = Vec::new(); // pre-activation

        for i in 0..nrows {
            let mut input: Vec<f64> = (0..ncols).map(|j| data[(i, j)]).collect();
            let mut layer_acts: Vec<Vec<f64>> = Vec::new();
            let mut layer_zs: Vec<Vec<f64>> = Vec::new();

            for (l, w) in weights.iter().enumerate() {
                // Add bias
                let mut biased = vec![1.0];
                biased.extend(input.iter());
                // z = W^T * input (including bias row)
                let n_out = w.ncols();
                let n_in = w.nrows();
                let z: Vec<f64> = (0..n_out)
                    .map(|j| (0..n_in).map(|k| w[(k, j)] * biased[k]).sum())
                    .collect();
                let a: Vec<f64> = z.iter().map(|&v| activations[l].activate(v)).collect();
                layer_zs.push(z);
                layer_acts.push(a.clone());
                input = a;
            }
            all_activations.push(layer_acts);
            all_zs.push(layer_zs);
        }

        // Backward pass + weight update
        let n_layers = weights.len();
        let mut weight_grads: Vec<Mat<f64>> = weights
            .iter()
            .map(|w| Mat::<f64>::zeros(w.nrows(), w.ncols()))
            .collect();

        for i in 0..nrows {
            let target = if labels[i] != 0 { 1.0 } else { 0.0 };
            let mut delta: Vec<f64> = Vec::new();

            for l in (0..n_layers).rev() {
                let prev_input: Vec<f64> = if l == 0 {
                    let mut v = vec![1.0]; // bias
                    v.extend((0..ncols).map(|j| data[(i, j)]));
                    v
                } else {
                    let mut v = vec![1.0]; // bias
                    v.extend(all_activations[i][l - 1].iter());
                    v
                };

                if l == n_layers - 1 {
                    // Output layer: delta = (pred - target) * sigmoid'
                    let pred = all_activations[i][l][0];
                    let z = all_zs[i][l][0];
                    delta = vec![(pred - target) * Activation::Sigmoid.derivative(z)];
                } else {
                    // Hidden layer: delta = (W_next * delta) * act'
                    let z = &all_zs[i][l];
                    let w_next = &weights[l + 1];
                    let new_delta: Vec<f64> = (0..w_next.nrows() - 1) // skip bias row
                        .map(|j| {
                            let z_j = z[j];

                            (0..delta.len())
                                .map(|k| w_next[(j + 1, k)] * delta[k]) // +1 for bias row
                                .sum::<f64>()
                                * activations[l].derivative(z_j)
                        })
                        .collect();
                    delta = new_delta;
                }

                // Accumulate gradient
                for k in 0..weight_grads[l].nrows() {
                    for j in 0..weight_grads[l].ncols() {
                        weight_grads[l][(k, j)] += prev_input[k] * delta[j];
                    }
                }
            }
        }

        // Update weights (gradient descent)
        let lr = opts.learning_rate / nrows as f64;
        for l in 0..n_layers {
            for k in 0..weights[l].nrows() {
                for j in 0..weights[l].ncols() {
                    weights[l][(k, j)] -= lr * weight_grads[l][(k, j)];
                }
            }
        }
    }

    Ok(MlpModel {
        weights,
        activations,
        n_features: ncols,
    })
}

/// Predict with fitted MLP.
pub fn mlp_predict(model: &MlpModel, data: &Mat<f64>) -> Vec<usize> {
    let (nrows, ncols) = data.shape();
    (0..nrows)
        .map(|i| {
            let mut input: Vec<f64> = (0..ncols).map(|j| data[(i, j)]).collect();
            for (l, w) in model.weights.iter().enumerate() {
                let mut biased = vec![1.0];
                biased.extend(input.iter());
                let n_out = w.ncols();
                let n_in = w.nrows();
                let z: Vec<f64> = (0..n_out)
                    .map(|j| (0..n_in).map(|k| w[(k, j)] * biased[k]).sum())
                    .collect();
                input = z
                    .iter()
                    .map(|&v| model.activations[l].activate(v))
                    .collect();
            }
            if input[0] > 0.5 { 1 } else { 0 }
        })
        .collect()
}

/// Predict probabilities.
pub fn mlp_predict_proba(model: &MlpModel, data: &Mat<f64>) -> Vec<f64> {
    let (nrows, ncols) = data.shape();
    (0..nrows)
        .map(|i| {
            let mut input: Vec<f64> = (0..ncols).map(|j| data[(i, j)]).collect();
            for (l, w) in model.weights.iter().enumerate() {
                let mut biased = vec![1.0];
                biased.extend(input.iter());
                let n_out = w.ncols();
                let n_in = w.nrows();
                let z: Vec<f64> = (0..n_out)
                    .map(|j| (0..n_in).map(|k| w[(k, j)] * biased[k]).sum())
                    .collect();
                input = z
                    .iter()
                    .map(|&v| model.activations[l].activate(v))
                    .collect();
            }
            input[0]
        })
        .collect()
}

// ═══════════════════════════════════════════════════════════════════════
// Autoencoder (encoder-decoder for dimensionality reduction)
// ═══════════════════════════════════════════════════════════════════════

pub struct AutoencoderResult {
    pub encoded: Vec<Vec<f64>>,       // n_samples × latent_dim
    pub reconstructed: Vec<Vec<f64>>, // n_samples × n_features
    pub reconstruction_error: f64,
}

pub fn autoencoder(
    data: &Mat<f64>,
    latent_dim: usize,
    n_epochs: usize,
    learning_rate: f64,
    seed: u64,
) -> Result<AutoencoderResult> {
    // Simple linear autoencoder: encode = W1*X, decode = W2*encode
    // Minimize ||X - W2*W1*X||^2
    let (nrows, ncols) = data.shape();
    if nrows == 0 {
        return Err(DeepError::Empty);
    }
    let mut rng = ChaCha8Rng::seed_from_u64(seed);

    // Initialize W1 (ncols × latent) and W2 (latent × ncols)
    let mut w1 = Mat::from_fn(ncols, latent_dim, |_, _| rng.random::<f64>() * 0.1);
    let mut w2 = Mat::from_fn(latent_dim, ncols, |_, _| rng.random::<f64>() * 0.1);

    for _ in 0..n_epochs {
        // Forward + backward for each sample
        let mut grad_w1 = Mat::<f64>::zeros(ncols, latent_dim);
        let mut grad_w2 = Mat::<f64>::zeros(latent_dim, ncols);

        for i in 0..nrows {
            let x: Vec<f64> = (0..ncols).map(|j| data[(i, j)]).collect();
            // Encode: z = x * W1 → [latent_dim]
            let z: Vec<f64> = (0..latent_dim)
                .map(|k| (0..ncols).map(|j| x[j] * w1[(j, k)]).sum())
                .collect();
            // Decode: x_hat = z * W2 → [ncols]
            let x_hat: Vec<f64> = (0..ncols)
                .map(|j| (0..latent_dim).map(|k| z[k] * w2[(k, j)]).sum())
                .collect();
            // Error
            let err: Vec<f64> = x_hat.iter().zip(&x).map(|(p, a)| p - a).collect();
            // Gradients
            // dL/dW2 = z^T * err
            for k in 0..latent_dim {
                for j in 0..ncols {
                    grad_w2[(k, j)] += z[k] * err[j];
                }
            }
            // dL/dW1 = x^T * (err * W2^T)
            for j in 0..ncols {
                for k in 0..latent_dim {
                    let grad_z = (0..ncols).map(|m| err[m] * w2[(k, m)]).sum::<f64>();
                    grad_w1[(j, k)] += x[j] * grad_z;
                }
            }
        }

        // Update
        let lr = learning_rate / nrows as f64;
        for j in 0..ncols {
            for k in 0..latent_dim {
                w1[(j, k)] -= lr * grad_w1[(j, k)];
            }
        }
        for k in 0..latent_dim {
            for j in 0..ncols {
                w2[(k, j)] -= lr * grad_w2[(k, j)];
            }
        }
    }

    // Compute final encoding + reconstruction
    let encoded: Vec<Vec<f64>> = (0..nrows)
        .map(|i| {
            let x: Vec<f64> = (0..ncols).map(|j| data[(i, j)]).collect();
            (0..latent_dim)
                .map(|k| (0..ncols).map(|j| x[j] * w1[(j, k)]).sum())
                .collect()
        })
        .collect();
    let reconstructed: Vec<Vec<f64>> = encoded
        .iter()
        .map(|z| {
            (0..ncols)
                .map(|j| (0..latent_dim).map(|k| z[k] * w2[(k, j)]).sum())
                .collect()
        })
        .collect();

    let mut recon_sq = 0.0f64;
    for i in 0..nrows {
        for j in 0..ncols {
            recon_sq += (reconstructed[i][j] - data[(i, j)]).powi(2);
        }
    }
    let recon_error = recon_sq.sqrt();

    Ok(AutoencoderResult {
        encoded,
        reconstructed,
        reconstruction_error: recon_error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::mat_from_row_major;

    #[test]
    fn test_mlp() {
        let data = mat_from_row_major(
            8,
            2,
            &[
                0.0, 0.0, 0.5, 0.5, 0.1, 0.2, 0.3, 0.1, 5.0, 5.0, 5.5, 5.5, 5.1, 5.2, 5.3, 5.1,
            ],
        );
        let labels = vec![0, 0, 0, 0, 1, 1, 1, 1];
        let opts = MlpOptions {
            hidden_sizes: vec![4],
            activation: Activation::ReLU,
            learning_rate: 0.5,
            n_epochs: 100,
            seed: 42,
        };
        let model = mlp_fit(&data, &labels, &opts).unwrap();
        let preds = mlp_predict(&model, &data);
        assert_eq!(preds.len(), 8);
        // Should classify well-separated data reasonably
        let correct = preds.iter().zip(&labels).filter(|(p, l)| p == l).count();
        assert!(correct >= 5);
    }

    #[test]
    fn test_autoencoder() {
        let data = mat_from_row_major(
            6,
            4,
            &[
                1.0, 2.0, 1.0, 2.0, 2.0, 1.0, 2.0, 1.0, 1.0, 2.0, 1.0, 2.0, 2.0, 1.0, 2.0, 1.0,
                10.0, 20.0, 10.0, 20.0, 20.0, 10.0, 20.0, 10.0,
            ],
        );
        let result = autoencoder(&data, 2, 200, 0.001, 42).unwrap();
        assert_eq!(result.encoded.len(), 6);
        assert_eq!(result.encoded[0].len(), 2);
        assert!(result.reconstruction_error.is_finite());
        assert!(result.reconstruction_error >= 0.0);
    }
}
