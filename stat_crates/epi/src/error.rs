//! Error type for the epi crate.

use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum EpiError {
    #[error("empty input")]
    EmptyInput,
    #[error("dimension mismatch: {a} vs {b}")]
    DimensionMismatch { a: usize, b: usize },
    #[error("contingency table must have ≥ 2 rows and ≥ 2 columns (got {rows}×{cols})")]
    InvalidTable { rows: usize, cols: usize },
    #[error("expected counts < 5 in {n} cells ({pct:.1}%); consider Fisher exact test")]
    SmallExpectedCounts { n: usize, pct: f64 },
    #[error("invalid probability or quantile: {0}")]
    InvalidProbability(f64),
    #[error("numerical error: {0}")]
    Numerical(String),
}

pub type Result<T> = std::result::Result<T, EpiError>;
