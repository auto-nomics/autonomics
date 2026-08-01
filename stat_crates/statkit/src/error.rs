//! Error type for the statkit crate.

use thiserror::Error;

/// Errors returned by statkit primitives.
#[derive(Debug, Clone, Error)]
pub enum StatError {
    #[error("empty input")]
    EmptyInput,
    #[error("length mismatch: {a} vs {b}")]
    LengthMismatch { a: usize, b: usize },
    #[error("invalid weights (negative or NaN)")]
    InvalidWeights,
    #[error("insufficient data: need ≥ {min} observations, got {actual}")]
    InsufficientData { min: usize, actual: usize },
    #[error("singular design matrix")]
    SingularMatrix,
    #[error("IRLS did not converge in {max_iter} iterations")]
    NotConverged { max_iter: usize },
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("invalid quantile: {0}")]
    InvalidQuantile(f64),
    #[error("numerical error: {0}")]
    Numerical(String),
}

pub type Result<T> = std::result::Result<T, StatError>;
