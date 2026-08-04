//! `mr` — a pure-Rust port of the **algorithm API** of the R package
//! [`TwoSampleMR`](https://github.com/MRCIEU/TwoSampleMR), covering the core
//! two-sample Mendelian-randomisation estimators, allele harmonisation, the
//! Steiger directionality test, heterogeneity / pleiotropy testing, and the
//! supporting summary-statistic utilities.
//!
//! This is a **library only** — there is no IO, plotting, or remote-query code
//! (those live outside the algorithm surface). It is structured to be plugged
//! into a DAG engine: every public estimator is a pure function over
//! `&[f64]` effect / standard-error vectors plus a [`Parameters`] config.
//!
//! Linear algebra runs on [`faer`] (no LAPACK/MKL); distribution functions and
//! normal sampling on `statrs`/`rand`. Numeric results are validated against
//! the R reference — point estimates are bit-aligned to the R test suite's
//! golden values (e.g. `R/testthat/test_mr.R`), and bootstrap standard errors
//! (which depend on an RNG stream R and Rust cannot share) are validated for
//! convergence only.
//!
//! # Faithfulness conventions
//!
//! - **NA** is represented as `f64::NAN`. A value is "valid" iff `!is_nan`
//!   (matching R's `!is.na()`, which is `TRUE` for both `NA` and `NaN`).
//! - R's `pnorm` / `pt` / `pchisq` / `pf` are reproduced via [`dist`].
//! - The weighted linear regression used by IVW / Egger reproduces R's
//!   `summary(lm(...))` exactly (see [`linalg`]); it does **not** normalise the
//!   weights, unlike `bio_crates::ldsc::linalg::wls`.
//!
//! | Module            | R source                                           | Contents                                                            |
//! |-------------------|----------------------------------------------------|---------------------------------------------------------------------|
//! | [`dist`]          | `stats::pnorm/pt/pchisq/pf` etc.                   | distribution functions + normal sampling                            |
//! | [`na`]            | `is.na`                                            | NA / validity helpers                                               |
//! | [`linalg`]        | `stats::lm` + `summary.lm`                         | faithful weighted linear regression                                 |
//! | [`kde`]           | `stats::density` (Gaussian kernel)                 | KDE grid for the mode estimator                                     |
//! | [`utils`]         | `R/add_rsq.r`, `R/query.R`, `R/rucker.R`           | `get_r_from_*`, `effective_n`, `Isq`, `get_se`, …                   |
//! | [`methods`]       | `R/mr.R`, `R/mr_mode.R`                            | Wald, IVW family, Egger, medians, modes                             |
//! | [`harmonise`]     | `R/harmonise.R`                                    | allele / effect harmonisation ([`harmonise::HarmoniseAction`])      |
//! | [`steiger`]       | `R/steiger.R` + `psych::r.test`                    | Steiger directionality test + sensitivity                           |
//! | [`heterogeneity`] | `R/heterogeneity.R`                                | Cochran's Q heterogeneity + Egger pleiotropy test                   |
//! | [`dispatch`]      | `R/mr.R: mr()`                                     | the `mr()` driver, method list, default parameters                  |
#![allow(clippy::needless_range_loop)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::doc_lazy_continuation)]

pub mod clump;
pub mod dispatch;
pub mod dist;
pub mod harmonise;
pub mod heterogeneity;
pub mod kde;
pub mod linalg;
pub mod methods;
pub mod na;
pub mod result;
pub mod steiger;
pub mod utils;

pub use dispatch::{MrMethod, MrResultRow, default_parameters, mr, mr_method_list};
pub use result::MrEstimate;

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha12Rng;

/// Error type for the `mr` crate.
#[derive(Debug, thiserror::Error)]
pub enum MrError {
    /// Length mismatch between input vectors.
    #[error("length mismatch: {0}")]
    LengthMismatch(String),
    /// Too few valid (non-NA) SNPs for the requested method.
    #[error("insufficient valid SNPs: {0}")]
    InsufficientSnps(String),
    /// A numerical step failed (singular design, non-convergence, …).
    #[error("numerical failure: {0}")]
    Numerical(String),
    /// The requested method is not implemented in this build (external-package
    /// methods such as `mr_raps` / `mr_ivw_radial` are stubbed).
    #[error("method not implemented in this build: {0}")]
    NotImplemented(String),
}

pub type Result<T> = std::result::Result<T, MrError>;

/// Parameters mirroring `default_parameters()` in `R/mr.R:305`.
///
/// ```text
/// list(test_dist = "z", nboot = 1000, Cov = 0, penk = 20, phi = 1,
///      alpha = 0.05, Qthresh = 0.05, over.dispersion = TRUE,
///      shrinkage = FALSE)
/// ```
///
/// `loss.function` from the R original is intentionally omitted: it is only
/// consumed by RAPS (`mr_raps`), which is not ported, so the field was dead
/// weight. If/when RAPS is ported, re-introduce it alongside the implementation.
#[derive(Debug, Clone)]
pub struct Parameters {
    /// `"z"` or `"t"` — test distribution for some methods.
    pub test_dist: String,
    /// Number of bootstrap replications for SE estimation.
    pub nboot: usize,
    /// Outcome–exposure beta covariance used in the delta-method SE
    /// (`parameters$Cov` in `mr_meta_fixed` / `mr_meta_random`).
    pub cov: f64,
    /// Penalisation constant (`penk`) for penalised weighted median / mode.
    pub penk: f64,
    /// Bandwidth multiplier (`phi`) for the mode estimator.
    pub phi: f64,
    /// Two-sided significance level for confidence intervals.
    pub alpha: f64,
    /// Q-statistic threshold for the Rucker framework (`Qthresh`).
    pub qthresh: f64,
    /// Whether the model accounts for overdispersion (mode variants).
    pub over_dispersion: bool,
    /// Whether empirical partially-Bayes shrinkage is applied.
    pub shrinkage: bool,
}

impl Parameters {
    /// Returns the default parameters, exactly as `default_parameters()` does.
    pub fn default_for() -> Self {
        Self {
            test_dist: "z".to_string(),
            nboot: 1000,
            cov: 0.0,
            penk: 20.0,
            phi: 1.0,
            alpha: 0.05,
            qthresh: 0.05,
            over_dispersion: true,
            shrinkage: false,
        }
    }
}

impl Default for Parameters {
    fn default() -> Self {
        Self::default_for()
    }
}

/// Default fixed seed for the bootstrap RNG. R and Rust cannot share an RNG
/// stream (R uses Mersenne–Twister `rnorm`), so bootstrap *standard errors*
/// are not bit-identical to R; only the (deterministic) point estimates are.
/// Callers needing reproducibility should pass their own `&mut Rng`.
pub const DEFAULT_BOOT_SEED: u64 = 0x5EED_1234_5678_CAFE;

/// Build the default deterministic RNG used for bootstrap SEs.
pub fn default_rng() -> ChaCha12Rng {
    ChaCha12Rng::seed_from_u64(DEFAULT_BOOT_SEED)
}

/// Draw a single standard-normal value — convenience wrapper.
pub fn rnorm_one(rng: &mut impl Rng) -> f64 {
    rng.sample(rand_distr::StandardNormal)
}
