//! `mvmr` — a pure-Rust port of the R package
//! [`MVMR`](https://github.com/WSpiller/MVMR) (Sanderson, Spiller, Bowden;
//! *Int. J. Epidemiology* 2019), which performs **multivariable Mendelian
//! randomisation** from two-sample summary statistics.
//!
//! The package implements the IVW multivariable MR estimator (weighted linear
//! regression through the origin with first-order inverse-variance weights),
//! plus the diagnostic statistics that the paper introduces:
//!
//! * the **conditional F-statistic** for instrument strength ([`strength`],
//!   [`strhet`]);
//! * the **Cochran's Q statistic** for instrument validity / pleiotropy
//!   ([`pleiotropy`]);
//! * the legacy combined IVW + strength + validity driver ([`mvmr`]);
//! * the weak-instrument-adjusted estimator via Q-minimisation ([`qhet`]).
//!
//! Linear algebra runs on [`faer`] (no LAPACK/MKL); distribution functions on
//! `statrs`. Numeric results are validated against the reference R package —
//! point estimates, standard errors, Q statistics and p-values match the R
//! `summary(lm(...))` golden values to ~1e-10 (see `tests/cross_validation.rs`).
//!
//! # Faithfulness notes
//!
//! * The weighted regression reproduces R's `lm(y ~ -1 + X, weights = w)`
//!   exactly: weights are precision weights (not normalised), the residual
//!   standard error is `sigma = sqrt(Σ wᵢ rᵢ² / (n − p))`, and the coefficient
//!   covariance is `sigma² (Xᵀ W X)⁻¹` (see [`linalg`]).
//! * `gencov = 0` (the default and recommended setting in the R docs) is the
//!   only well-identified path in two-sample summary data; the legacy `mvmr`
//!   and `pleiotropy` functions accept a non-zero scalar only for interface
//!   compatibility with the R package.
//! * [`qhet`] reproduces the `optim`/`optimize` profiled likelihood but does
//!   **not** reproduce R's bootstrap CIs (they depend on an RNG stream that
//!   Rust and R cannot share); it returns only point estimates unless callers
//!   supply their own bootstrap wrapper.
//!
//! # References
//!
//! - Sanderson E, Richardson TG, Hemani G, Smith GD, Bowden J (2019).
//!   *An examination of multivariable Mendelian randomization in the
//!   single-sample and two-sample summary data settings.* International
//!   Journal of Epidemiology 48(3):713–727. doi:10.1093/ije/dyy262

#![allow(clippy::needless_range_loop)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::doc_lazy_continuation)]

pub mod cov;
pub mod error;
pub mod format;
pub mod ivw;
pub mod legacy;
pub mod linalg;
pub mod pleiotropy;
pub mod qhet;
pub mod strength;
pub mod strhet;

pub use error::{MvmrError, Result};
pub use format::MvmrInput;
pub use ivw::{IvwResult, ivw_mvmr};
pub use legacy::{MvmrLegacyResult, mvmr};
pub use pleiotropy::{PleiotropyResult, pleiotropy_mvmr};
pub use qhet::{QhetResult, qhet_mvmr};
pub use strength::{StrengthResult, strength_mvmr};
pub use strhet::{StrhetResult, strhet_mvmr};
