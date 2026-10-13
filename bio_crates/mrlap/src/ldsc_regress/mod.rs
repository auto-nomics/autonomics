//! LD Score regression engine backing the cross-trait stage
//! ([`crate::ldsc_runner`]) — a faithful Rust port of the Python LDSC suite's
//! `ldscore/regressions.py` (`LD_Score_Regression` / `Hsq` / `Gencov` / `RG`).
//!
//! Vendored from the removed `bio_crates/ldsc` crate (recoverable from git
//! history). Only the regression core MRlap needs is kept; the numeric code
//! is unchanged and stays covered by the unit tests shipped alongside plus
//! the R cross-validation fixtures in `tests/`.

pub mod irwls;
pub mod jackknife;
pub mod linalg;
pub mod regress;
pub mod stats;

pub use regress::{Gencov, Hsq, RG};

/// Error type for the LD Score regression engine.
///
/// Trimmed from the original `LdscError`: the vendored regression core only
/// constructs the four variants below (the I/O / parse / PLINK / DataFusion
/// variants belonged to the removed ingest layer).
#[derive(Debug, thiserror::Error)]
pub enum LdscError {
    /// Shape / size mismatch between vectors or matrices.
    #[error("dimension mismatch: {0}")]
    DimensionMismatch(String),
    /// An input value is invalid (empty data, non-numeric column, etc.).
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// A numerical linear-algebra operation failed (e.g. LLᵀ factorisation).
    #[error("linear algebra error: {0}")]
    Linalg(String),
    /// An arithmetic operation produced a non-finite value (NaN/Inf) where
    /// numpy would have raised (`np.seterr(divide='raise', invalid='raise')`).
    #[error("numerical error: {0}")]
    Numerical(String),
}

/// Module-local `Result` alias.
pub type Result<T> = std::result::Result<T, LdscError>;
