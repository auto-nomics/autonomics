//! Error types for the coloc crate.

use thiserror::Error;

/// Error returned by coloc functions.
#[derive(Debug, Error)]
pub enum ColocError {
    #[error("dataset {0}: duplicated snps found")]
    DuplicateSnps(String),

    #[error("dataset {0}: MAF should be strictly >0 & <1")]
    BadMaf(String),

    #[error("dataset {suffix}: lengths of inputs don't match: {element}")]
    LengthMismatch { suffix: String, element: String },

    #[error("dataset {suffix}: {element} contains missing values")]
    MissingValues { suffix: String, element: String },

    #[error("dataset {0}: Infinite values in beta and/or varbeta")]
    InfiniteValues(String),

    #[error("dataset {0}: Zero values in varbeta")]
    ZeroVarbeta(String),

    #[error("dataset {0}: s must be between 0 and 1")]
    BadS(String),

    #[error("require p values and MAF if beta, varbeta are unavailable")]
    InsufficientData,

    #[error("dataset {0}: pvalues should not be negative or exactly 0")]
    NonPositivePvalue(String),

    #[error("dataset {0}: require s for case-control if beta, varbeta unavailable")]
    MissingSForCC(String),

    #[error("dataset {0}: sample size N <= 0 or not set")]
    BadN(String),

    #[error("dataset {0}: must give sdY for type quant, or MAF and N so it can be estimated")]
    MissingSdYOrMafN(String),

    #[error("type must be quant or cc, got {0}")]
    BadTraitType(String),

    #[error("estimated sdY is negative — check data quality")]
    NegativeSdY,

    #[error("MAF required to estimate sdY")]
    MissingMafForSdY,

    #[error("N required to estimate sdY")]
    MissingNForSdY,

    #[error("dataset1 and dataset2 have no common SNPs")]
    NoCommonSnps,
}

/// Convenience `Result` alias.
pub type Result<T> = std::result::Result<T, ColocError>;
