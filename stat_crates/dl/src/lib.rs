//! Deep learning crate — Burn-based neural networks for the DAG engine.
//!
//! Uses Burn (with `burn-ndarray` CPU backend + `burn-autodiff`) for all
//! gradient computation. The public API is a set of serde-serialisable model
//! structs and train/predict functions that operate on `dl::Tensor` (f64).

// ─── Modules ───────────────────────────────────────────────────────────

pub mod backend;
pub mod tensor;
pub mod data;
pub mod configs;
pub mod scaler;
pub mod survival;
pub mod scheduler;
pub mod artifact;
pub mod models;

// ─── Re-exports for nodes-dl ───────────────────────────────────────────

// Core types.
pub use tensor::Tensor;
pub use configs::{
    Activation, AeKind, AeLoss, CellType, LayerWeights, OptimizerConfig, OptimizerKind,
    Pooling, PositionalEncoding, SchedulerConfig, SeqPooling, TaskType, TrainConfig,
    EarlyStoppingConfig, EpochLog,
};
pub use scaler::StandardScaler;
pub use artifact::{Architecture, ArtifactTaskType, DLModelArtifact, TrainingMeta};

// Survival metrics.
pub use survival::{c_index, td_auc, brier_score, TimeBins};

// ─── Model modules (path-accessible: dl::mlp::*, dl::deepsurv::*, etc.) ─

pub use models::mlp;
pub use models::deepsurv;
pub use models::transformer;
pub use models::autoencoder;
pub use models::deephit;
pub use models::rnn;
pub use models::burn_net;

// ─── Convenience re-exports ────────────────────────────────────────────

pub use mlp::{MlpConfig, MlpModel, MlpTrainOutput, train_mlp, predict_mlp};
pub use deepsurv::{DeepSurvConfig, DeepSurvModel, DeepSurvTrainOutput, train_deepsurv, predict_deepsurv};
pub use transformer::{TransformerConfig, TransformerModel, TransformerTrainOutput, train_transformer, predict_transformer};
pub use autoencoder::{AutoEncoderConfig, AutoEncoderModel, AutoEncoderTrainOutput, train_autoencoder, predict_autoencoder, encode_autoencoder};
/// Alias for [`encode_autoencoder`] (nodes-dl compatibility).
pub fn predict_autoencoder_latent(model: &mut autoencoder::AutoEncoderModel, x: &Tensor) -> Tensor {
    encode_autoencoder(model, x)
}

/// Extract hidden-layer embeddings from a trained MLP.
///
/// Forwards through all layers except the output head and returns the
/// resulting feature matrix.
pub fn embed_mlp(model: &mut mlp::MlpModel, x: &Tensor) -> Tensor {
    let device = backend::device();
    let x_scaled = model
        .scaler
        .as_ref()
        .map(|s| s.transform(x))
        .unwrap_or_else(|| x.clone());

    let infer_model = models::burn_net::BurnMlp::<backend::B>::from_weights(&model.layers, &device);
    let x_burn = data::f64_to_burn_infer(&x_scaled, &device);

    // Forward through all layers except the last (output head).
    let n_layers = infer_model.layers.len();
    let mut h = x_burn;
    for (i, layer) in infer_model.layers.iter().enumerate() {
        h = layer.forward(h);
        if i < n_layers - 1 {
            h = models::burn_net::apply_activation(h, model.config.activation);
        }
    }
    // h is now the output. We want the penultimate layer output.
    // Re-do forward but stop before the last layer.
    let mut h = data::f64_to_burn_infer(&x_scaled, &device);
    for (i, layer) in infer_model.layers.iter().enumerate() {
        if i == n_layers - 1 {
            break;
        }
        h = layer.forward(h);
        if i < n_layers - 2 {
            h = models::burn_net::apply_activation(h, model.config.activation);
        }
    }
    data::burn2d_to_tensor(h)
}
pub use deephit::{DeepHitConfig, DeepHitModel, DeepHitTrainOutput, train_deephit, predict_deephit};
pub use rnn::{RnnConfig, RnnModel, RnnTrainOutput, train_rnn, predict_rnn};

// ─── Test ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_burn_tensor_addition() {
        let device = backend::device();
        let t1 = burn::tensor::Tensor::<backend::Backend, 1>::from_floats(
            [1.0, 2.0, 3.0],
            &device,
        );
        let t2 = burn::tensor::Tensor::<backend::Backend, 1>::from_floats(
            [4.0, 5.0, 6.0],
            &device,
        );
        let result = t1 + t2;
        let val = result.into_data().as_slice::<f32>().unwrap()[0];
        assert!((val - 5.0).abs() < 1e-6, "expected 5.0, got {val}");
    }

    #[test]
    fn test_burn_autodiff() {
        let device = backend::device();
        let x =
            burn::tensor::Tensor::<backend::Backend, 1>::from_floats([1.0, 2.0, 3.0], &device)
                .require_grad();
        let y = x.clone().powi_scalar(2).sum();
        let grads = y.backward();
        let x_grad = x.grad(&grads).unwrap();
        let grad_vals = x_grad.to_data().as_slice::<f32>().unwrap().to_vec();
        // d/dx sum(x^2) = 2x = [2, 4, 6].
        assert!((grad_vals[0] - 2.0).abs() < 1e-3, "grad[0] = {}", grad_vals[0]);
        assert!((grad_vals[1] - 4.0).abs() < 1e-3, "grad[1] = {}", grad_vals[1]);
        assert!((grad_vals[2] - 6.0).abs() < 1e-3, "grad[2] = {}", grad_vals[2]);
    }
}
