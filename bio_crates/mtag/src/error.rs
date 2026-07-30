//! Error types for the `mtag` crate.

/// Error type for MTAG operations.
#[derive(Debug, thiserror::Error)]
pub enum MtagError {
    /// Dimension / shape mismatch between matrices or vectors.
    #[error("dimension mismatch: {0}")]
    DimensionMismatch(String),
    /// An input value is invalid (empty data, non-positive-definite, etc.).
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// A linear-algebra operation failed (Cholesky, eigendecomposition, solve).
    #[error("linear algebra error: {0}")]
    Linalg(String),
    /// A numerical operation produced a non-finite value (NaN/Inf).
    #[error("numerical error: {0}")]
    Numerical(String),
    /// File / stream I/O failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// A matrix failed the positive-definiteness check.
    #[error("matrix is not positive semi-definite: {0}")]
    NotPositiveDefinite(String),
}

/// Crate-local `Result` alias.
pub type Result<T> = std::result::Result<T, MtagError>;
