//! Error types for the `mrpresso` crate.

use thiserror::Error;

/// Errors produced by MR-PRESSO.
#[derive(Debug, Error)]
pub enum MrpressoError {
    #[error("SignifThreshold must be <= 1 (got {0})")]
    ThresholdTooLarge(f64),
    #[error("BetaExposure and SdExposure must have the same number of elements")]
    ExposureSdMismatch,
    #[error("not enough instrumental variables: nrow={0}, nexp={1}")]
    NotEnoughInstruments(usize, usize),
    #[error("not enough elements to compute empirical P-values: nrow={0} >= NbDistribution={1}")]
    NotEnoughForEmpirical(usize, usize),
    #[error("data contains no rows after NA removal")]
    EmptyData,
    #[error("column length mismatch: {0}")]
    LengthMismatch(String),
    #[error("linear algebra error: {0}")]
    Linalg(String),
}

/// Crate result type.
pub type Result<T> = std::result::Result<T, MrpressoError>;
