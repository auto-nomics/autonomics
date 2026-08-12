//! `dl` — deep learning algorithms for the DAG engine.
//!
//! Pure Rust/faer neural network implementations: MLP, DeepSurv, Transformer,
//! Autoencoder, etc.  No external DL framework dependency (Burn/Candle/tch).
//!
//! ## Architecture
//!
//! | Layer  | Location | Responsibility |
//! |--------|----------|----------------|
//! | Tensor | [`tensor`] | Dense matrix wrapper |
//! | Layers | [`layers`] | Linear, activations, dropout, batch norm |
//! | Optimizer | [`optimizer`] | SGD, Adam, AdamW, RMSprop |
//! | Scheduler | [`scheduler`] | Step, cosine, plateau, warmup-cosine |
//! | Losses | [`losses`] | BCE, MSE, Huber, Cox partial likelihood |
//! | Models | [`mlp`], [`deepsurv`] | Model architectures + training loops |
//! | Survival | [`survival`] | C-index, td-AUC, Brier score, time bins |
//! | Artifact | [`artifact`] | `DLModelArtifact` — serialised model |

pub mod artifact;
pub mod deepsurv;
pub mod layers;
pub mod losses;
pub mod mlp;
pub mod optimizer;
pub mod scaler;
pub mod scheduler;
pub mod survival;
pub mod tensor;
pub mod transformer;

// Re-exports for convenience.
pub use artifact::{Architecture, ArtifactTaskType, DLModelArtifact, TrainingMeta};
pub use deepsurv::{predict_deepsurv, train_deepsurv, DeepSurvConfig, DeepSurvModel};
pub use layers::{Activation, BatchNorm1d, Dropout, Linear};
pub use losses::{binary_cross_entropy, cox_partial_likelihood_loss, mse, sigmoid_stable};
pub use mlp::{predict_mlp, train_mlp, MlpConfig, MlpModel, TaskType, TrainConfig};
pub use optimizer::{Optimizer, OptimizerConfig, OptimizerKind};
pub use scaler::StandardScaler;
pub use scheduler::{Scheduler, SchedulerConfig};
pub use survival::{c_index, td_auc, brier_score, TimeBins};
pub use tensor::Tensor;
pub use transformer::{
    predict_transformer, train_transformer, Pooling, PositionalEncoding, TransformerConfig,
    TransformerModel,
};
