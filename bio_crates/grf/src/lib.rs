//! Rust port of R grf (Generalized Random Forests), version 2.6.1.
//!
//! This crate exposes the same statistical API as R's `grf` package, but
//! dispatches every algorithm to the upstream C++ core via the
//! `grf-sys` C ABI. Algorithms are therefore bit-identical to the R package;
//! the Rust layer only handles:
//!
//! - **High-level parameter validation & defaults** (`grf::TrainOptions`)
//! - **Data shape marshaling** (Arrow/Vec<Vec<f64>> ↔ column-major buffer)
//! - **Serialization** of trained forests to a portable binary blob
//! - **Forest-flavored convenience** (R-learner two-stage for `causal_forest`,
//!   OOB prediction wrappers, etc.)
//!
//! Higher-level crates (DAG node bundles, agent tools) build on this crate.
//!
//! # Crate layout
//!
//! - [`forest`] — the `ForestBlob` newtype (serializable trained forest)
//!   plus the per-type trainer helpers.
//! - [`data`] — column-major ↔ row-major conversion utilities for grf input.
//! - [`nodes`] — DAG node implementations (currently: `grf_regression_forest`,
//!   `grf_predict_forest`).
//!
//! # Quick example
//!
//! ```ignore
//! use grf::{TrainOptions, RegressionForestSpec, ForestKind};
//!
//! let spec = RegressionForestSpec {
//!     x: df, y: y_vec,
//!     options: TrainOptions { num_trees: 100, ..Default::default() },
//! };
//! let trained = spec.fit()?;
//! let predictions = trained.predict(&test_df, /*estimate_variance=*/false)?;
//! ```

pub mod data;
pub mod forest;
pub mod nodes;

pub use data::{Matrix, column_major, from_column_major};
pub use forest::{
    CausalSurvivalTrainer, CausalTrainer, ForestBlob, ForestKind, ForestStats, InstrumentalTrainer,
    LlRegressionTrainer, LmTrainer, MultiCausalSpec, MultiCausalTrainer, MultiRegressionTrainer,
    OobPredictions, PredictRequest, Predictions, ProbabilityTrainer, QuantileTrainer,
    RegressionTrainer, SurvivalTrainer,
};

use grf_sys as sys;
use thiserror::Error;

/// Top-level crate error type.
#[derive(Debug, Error)]
pub enum GrfError {
    #[error("grf: {0}")]
    Sys(#[from] sys::GrfError),

    /// Convenience: a string-form C++ error, used by Rust-side helpers
    /// (statkit OLS, etc.) that don't have a richer grf-sys error to wrap.
    #[error("grf: {0}")]
    Cpp(String),

    #[error("grf: shape mismatch: {0}")]
    Shape(String),

    #[error("grf: forest kind mismatch: trained as {trained:?}, requested {requested:?}")]
    KindMismatch {
        trained: ForestKind,
        requested: ForestKind,
    },

    #[error("grf: missing required input: {0}")]
    Missing(String),
}

pub type Result<T> = std::result::Result<T, GrfError>;
