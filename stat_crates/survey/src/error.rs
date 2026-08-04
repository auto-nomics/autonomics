//! Error type for the survey crate.

use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum SurveyError {
    #[error("empty input: {0}")]
    EmptyInput(String),
    #[error("length mismatch: {context} — {a} vs {b}")]
    LengthMismatch { context: String, a: usize, b: usize },
    #[error("invalid design: {0}")]
    InvalidDesign(String),
    #[error("lonely PSU in stratum {stratum}: {hint}")]
    LonelyPsu { stratum: String, hint: String },
    #[error("all weights are zero")]
    ZeroWeights,
    #[error("invalid input: {0}")]
    InvalidInput(String),
}

pub type Result<T> = std::result::Result<T, SurveyError>;
