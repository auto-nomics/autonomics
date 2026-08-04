//! Error type for the `mvmr` crate.

/// Errors returned by MVMR estimators.
#[derive(Debug, thiserror::Error)]
pub enum MvmrError {
    /// Input vector length mismatch.
    #[error("length mismatch: {0}")]
    LengthMismatch(String),
    /// Too few instruments for the requested number of exposures.
    #[error("insufficient instruments: {0}")]
    InsufficientSnps(String),
    /// A numerical step failed (singular design, non-convergence, …).
    #[error("numerical failure: {0}")]
    Numerical(String),
    /// Inconsistent exposure count across inputs.
    #[error("inconsistent exposure count: {0}")]
    BadExposureCount(String),
}

pub type Result<T> = std::result::Result<T, MvmrError>;
