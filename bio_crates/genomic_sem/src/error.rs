//! Error types for the `genomic_sem` crate.

use thiserror::Error;

/// Error type for all GenomicSEM operations.
#[derive(Debug, Error)]
pub enum GenomicSemError {
    #[error("matrix dimension mismatch: expected {expected}, got {actual}")]
    DimMismatch { expected: String, actual: String },

    #[error("matrix is not square ({rows}×{cols})")]
    NotSquare { rows: usize, cols: usize },

    #[error("SEM fitting failed to converge")]
    Convergence,

    #[error("SEM model syntax error: {0}")]
    Syntax(String),

    #[error("LDSC regression error: {0}")]
    Ldsc(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("trait names contain mathematical operators (+, -, *, /, ^) that lavaan misreads")]
    BadTraitNames,

    #[error("no trait names in the LDSC output match names in the model")]
    NoTraitMatch,

    #[error("genetic covariance matrix has negative heritability (negative diagonal)")]
    NegativeHeritability,

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("numerical error: {0}")]
    Numerical(String),
}

/// Result alias.
pub type Result<T> = std::result::Result<T, GenomicSemError>;
