//! `mtag` — a pure-Rust port of **MTAG** (Multi-Trait Analysis of GWAS,
//! Turley et al. *Nature Genetics* 2018, doi:10.1038/s41588-017-0009-4).
//!
//! MTAG jointly analyses multiple GWAS summary-statistics sets to increase
//! the effective sample size for each trait by leveraging genetic
//! correlations among traits. The method estimates:
//!
//! - **Σ** (Sigma): the residual covariance of Z-scores, estimated via LD
//!   Score Regression bivariate intercepts.
//! - **Ω** (Omega): the genetic effect-size covariance, estimated via GMM
//!   (method of moments, default) or numerical MLE.
//!
//! Then applies a conditional-expectation formula to produce MTAG-adjusted
//! betas and SEs for each SNP–trait pair.
//!
//! This is a **library only** — a 1:1 faithful port of the Python reference
//! (`reference/mtag/mtag.py` v1.0.8):
//!
//! | Module      | Python source                        | Contents                                                |
//! |-------------|--------------------------------------|---------------------------------------------------------|
//! | [`linalg`]  | `_posDef_adjustment`, `cov2corr`     | posdef adjustment, MVN PDF, eigendecomposition, invert  |
//! | [`nelder`]  | `scipy.optimize.minimize(Nelder-Mead)` | generic Nelder-Mead simplex optimiser                 |
//! | [`omega`]   | `gmm_omega`, `estimate_omega`        | Ω estimation: GMM / MLE / special cases                 |
//! | [`mtag`]    | `mtag_analysis`                      | core MTAG conditional formula                           |
//! | [`fdr`]     | `fdr`, `compute_fdr`, `simplex_walk` | max FDR grid search                                     |
//! | [`data`]    | `load_and_merge_data`, `save_*`      | data loading, allele harmonisation, result I/O          |
//! | [`model`]   | `mtag()`                             | the top-level pipeline driver                           |
//!
//! Linear algebra runs on [`faer`] (no LAPACK/MKL backend). Distribution
//! functions (normal PDF/CDF/ISF) use [`statrs`], matching scipy.

#![allow(clippy::needless_range_loop)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::doc_lazy_continuation)]

pub mod data;
pub mod error;
pub mod fdr;
pub mod linalg;
pub mod model;
pub mod mtag;
pub mod nelder;
pub mod omega;

pub use error::{MtagError, Result};
