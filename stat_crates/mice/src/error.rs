//! Error type for the `mice` crate.

/// Errors returned by MICE imputation algorithms.
#[derive(Debug, thiserror::Error)]
pub enum MiceError {
    /// Input vector length mismatch.
    #[error("length mismatch: {0}")]
    LengthMismatch(String),
    /// Too few observations / predictors for the requested fit.
    #[error("insufficient data: {0}")]
    InsufficientData(String),
    /// A numerical step failed (singular design, non-convergence, ...).
    #[error("numerical failure: {0}")]
    Numerical(String),
    /// Spec validation failed.
    #[error("invalid specification: {0}")]
    InvalidSpec(String),
    /// Method name was unrecognised.
    #[error("unknown imputation method: {0}")]
    UnknownMethod(String),
}

pub type Result<T> = std::result::Result<T, MiceError>;
