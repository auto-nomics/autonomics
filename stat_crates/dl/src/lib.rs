//! Deep learning crate — Burn-based neural networks for the DAG engine.
//!
//! Uses Burn (with `burn-ndarray` CPU backend + `burn-autodiff`) for all
//! gradient computation. The public API is a set of serde-serialisable model
//! structs and train/predict functions that operate on `dl::Tensor` (f64).

// ─── Modules ───────────────────────────────────────────────────────────

pub mod artifact;
pub mod backend;
pub mod configs;
pub mod data;
pub mod models;
pub mod scaler;
pub mod scheduler;
pub mod survival;
pub mod tensor;

// ─── Re-exports for nodes-dl ───────────────────────────────────────────

// Core types.
pub use artifact::{Architecture, ArtifactTaskType, DLModelArtifact, TrainingMeta};
pub use configs::{
    Activation, AeKind, AeLoss, CellType, EarlyStoppingConfig, EpochLog, LayerWeights,
    OptimizerConfig, OptimizerKind, Pooling, PositionalEncoding, SchedulerConfig, SeqPooling,
    TaskType, TrainConfig,
};
pub use scaler::StandardScaler;
pub use tensor::Tensor;

// Survival metrics.
pub use survival::{TimeBins, brier_score, c_index, td_auc};

// ─── Model modules (path-accessible: dl::mlp::*, dl::deepsurv::*, etc.) ─

pub use models::autoencoder;
pub use models::burn_net;
pub use models::deephit;
pub use models::deepsurv;
pub use models::mlp;
pub use models::rnn;
pub use models::transformer;

// ─── Convenience re-exports ────────────────────────────────────────────

pub use autoencoder::{
    AutoEncoderConfig, AutoEncoderModel, AutoEncoderTrainOutput, encode_autoencoder,
    predict_autoencoder, train_autoencoder,
};
pub use deepsurv::{
    DeepSurvConfig, DeepSurvModel, DeepSurvTrainOutput, predict_deepsurv, train_deepsurv,
};
pub use mlp::{MlpConfig, MlpModel, MlpTrainOutput, predict_mlp, train_mlp};
pub use transformer::{
    TransformerConfig, TransformerModel, TransformerTrainOutput, predict_transformer,
    train_transformer,
};
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
    embed_from_burn_mlp(&x_scaled, &infer_model, model.config.activation)
}

/// Extract penultimate-layer embeddings from a trained Transformer.
pub fn embed_transformer(model: &mut transformer::TransformerModel, x: &Tensor) -> Tensor {
    let device = backend::device();
    let x_scaled = model
        .scaler
        .as_ref()
        .map(|s| s.transform(x))
        .unwrap_or_else(|| x.clone());

    let infer_model = models::burn_net::BurnMlp::<backend::B>::from_weights(&model.layers, &device);
    // Transformer uses GELU activation internally.
    embed_from_burn_mlp(&x_scaled, &infer_model, configs::Activation::Gelu)
}

/// Extract penultimate-layer embeddings from a trained RNN.
pub fn embed_rnn(model: &mut rnn::RnnModel, x: &Tensor) -> Tensor {
    let device = backend::device();
    let x_scaled = model
        .scaler
        .as_ref()
        .map(|s| s.transform(x))
        .unwrap_or_else(|| x.clone());

    let infer_model = models::burn_net::BurnMlp::<backend::B>::from_weights(&model.layers, &device);
    // RNN uses ReLU activation internally.
    embed_from_burn_mlp(&x_scaled, &infer_model, configs::Activation::Relu)
}

/// Forward through all layers except the last (output head) and return the
/// penultimate layer's output as a feature matrix.
fn embed_from_burn_mlp(
    x_scaled: &Tensor,
    infer_model: &models::burn_net::BurnMlp<backend::B>,
    activation: configs::Activation,
) -> Tensor {
    let device = backend::device();
    let n_layers = infer_model.layers.len();
    let mut h = data::f64_to_burn_infer(x_scaled, &device);
    for (i, layer) in infer_model.layers.iter().enumerate() {
        if i == n_layers - 1 {
            break;
        }
        h = layer.forward(h);
        if i < n_layers - 2 {
            h = models::burn_net::apply_activation(h, activation);
        }
    }
    data::burn2d_to_tensor(h)
}
pub use deephit::{
    DeepHitConfig, DeepHitModel, DeepHitTrainOutput, predict_deephit, train_deephit,
};
pub use rnn::{RnnConfig, RnnModel, RnnTrainOutput, predict_rnn, train_rnn};

// ─── Test ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_burn_tensor_addition() {
        let device = backend::device();
        let t1 = burn::tensor::Tensor::<backend::Backend, 1>::from_floats([1.0, 2.0, 3.0], &device);
        let t2 = burn::tensor::Tensor::<backend::Backend, 1>::from_floats([4.0, 5.0, 6.0], &device);
        let result = t1 + t2;
        let val = result.into_data().as_slice::<f32>().unwrap()[0];
        assert!((val - 5.0).abs() < 1e-6, "expected 5.0, got {val}");
    }

    #[test]
    fn test_burn_autodiff() {
        let device = backend::device();
        let x = burn::tensor::Tensor::<backend::Backend, 1>::from_floats([1.0, 2.0, 3.0], &device)
            .require_grad();
        let y = x.clone().powi_scalar(2).sum();
        let grads = y.backward();
        let x_grad = x.grad(&grads).unwrap();
        let grad_vals = x_grad.to_data().as_slice::<f32>().unwrap().to_vec();
        // d/dx sum(x^2) = 2x = [2, 4, 6].
        assert!(
            (grad_vals[0] - 2.0).abs() < 1e-3,
            "grad[0] = {}",
            grad_vals[0]
        );
        assert!(
            (grad_vals[1] - 4.0).abs() < 1e-3,
            "grad[1] = {}",
            grad_vals[1]
        );
        assert!(
            (grad_vals[2] - 6.0).abs() < 1e-3,
            "grad[2] = {}",
            grad_vals[2]
        );
    }
}
