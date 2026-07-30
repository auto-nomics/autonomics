//! Error type for the `mrlap` crate.

use thiserror::Error;

/// Errors produced by the MRlap pipeline.
#[derive(Debug, Error)]
pub enum MrlapError {
    /// A user-supplied parameter was out of range (mirrors R's `stop(...)` calls).
    #[error("{0}")]
    InvalidArg(String),
    /// Required column missing / ambiguous in the input GWAS.
    #[error("input GWAS: {0}")]
    Input(String),
    /// A numerical step failed (no IVs, non-convergence, ...).
    #[error("numerical: {0}")]
    Numerical(String),
    /// The underlying LD Score regression failed.
    #[error("LDSC: {0}")]
    Ldsc(#[from] ldsc::LdscError),
}

impl MrlapError {
    pub fn invalid_arg(msg: impl Into<String>) -> Self {
        Self::InvalidArg(msg.into())
    }
    pub fn input(msg: impl Into<String>) -> Self {
        Self::Input(msg.into())
    }
    pub fn numerical(msg: impl Into<String>) -> Self {
        Self::Numerical(msg.into())
    }
}

pub type Result<T> = std::result::Result<T, MrlapError>;
