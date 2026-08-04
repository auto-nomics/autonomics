//! Error type for the `cmprsk` port.

use thiserror::Error;

/// Errors raised by the competing-risks routines.
#[derive(Debug, Error)]
pub enum CmprskError {
    /// Input vectors had mismatched lengths.
    #[error("length mismatch: {what} has {got} elements, expected {expected}")]
    LengthMismatch {
        /// Name of the offending input.
        what: &'static str,
        /// Observed length.
        got: usize,
        /// Expected length.
        expected: usize,
    },

    /// No observations remained after the complete-case filter.
    #[error("no observations remain after removing missing values")]
    NoObservations,

    /// No failures of the requested type were present.
    #[error("no events with failcode present in the data")]
    NoEvents,

    /// The model matrix was empty (neither `cov1` nor `cov2` supplied).
    #[error("at least one of cov1 / cov2 must be supplied")]
    NoCovariates,

    /// `cov2` and the time-function matrix disagreed on the number of columns.
    #[error("cov2 has {ncov2} columns but tf produced {ntf} columns")]
    TfShapeMismatch {
        /// Columns in `cov2`.
        ncov2: usize,
        /// Columns produced by the time function.
        ntf: usize,
    },

    /// A supplied `init` vector had the wrong length.
    #[error("init has {got} elements but the model has {expected} parameters")]
    BadInit {
        /// Observed length.
        got: usize,
        /// Number of model parameters.
        expected: usize,
    },

    /// A linear system could not be solved (singular information matrix).
    #[error("singular matrix in {context}")]
    Singular {
        /// Where the singularity was hit.
        context: &'static str,
    },

    /// Covariate dimensions supplied to `predict` did not match the fit.
    #[error("predict: {0}")]
    Predict(String),

    /// Generic invalid-argument case.
    #[error("{0}")]
    Invalid(String),
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, CmprskError>;
