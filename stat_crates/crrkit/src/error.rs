//! Error type for the `crrkit` competing-risk risk-estimation metrics.

use thiserror::Error;

/// Errors raised by the competing-risk scoring routines.
#[derive(Debug, Error)]
pub enum CrrkitError {
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

    /// An event-status code outside `{0, 1, 2}` was supplied.
    #[error("invalid event status {got} at position {index}: expected 0 (censored), 1 (event of interest) or 2 (competing event)")]
    BadStatus {
        /// Position of the offending observation.
        index: usize,
        /// The offending code.
        got: u8,
    },

    /// A time was not finite, or was negative.
    #[error("invalid time {got} at position {index}: times must be finite and non-negative")]
    BadTime {
        /// Position of the offending observation.
        index: usize,
        /// The offending time.
        got: f64,
    },

    /// A predicted probability fell outside `[0, 1]`.
    #[error("invalid predicted probability {value} at position {got}")]
    BadProbability {
        /// Position of the offending observation.
        got: usize,
        /// The offending probability.
        value: f64,
    },

    /// The evaluation horizon was not strictly positive.
    #[error("horizon must be finite and strictly positive, got {got}")]
    BadHorizon {
        /// The offending horizon.
        got: f64,
    },

    /// No outcome was knowable at the horizon (everybody censored before it).
    #[error("no outcome information at or beyond the horizon: every observation is censored before {horizon}")]
    NoOutcomeInformation {
        /// The evaluation horizon.
        horizon: f64,
    },

    /// The reverse-KM censoring survival collapsed to zero at or before a
    /// point where a weight is needed.
    #[error("censoring survival G collapsed to {g} at t = {t}: inverse-probability weights are undefined")]
    CollapsedCensoring {
        /// The value the censoring survival reached.
        g: f64,
        /// Where it collapsed.
        t: f64,
    },

    /// A calibration request produced empty groups.
    #[error("calibration grouping produced no usable groups for {n} subjects")]
    EmptyCalibration {
        /// Number of subjects offered.
        n: usize,
    },

    /// A bootstrap resample could not produce a usable estimate.
    #[error("all {n_boot} bootstrap resamples failed (censoring collapse or degenerate resample)")]
    BootstrapExhausted {
        /// Number of requested resamples.
        n_boot: usize,
    },

    /// Generic invalid-argument case.
    #[error("{0}")]
    Invalid(String),
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, CrrkitError>;
