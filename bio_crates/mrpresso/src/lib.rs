//! `mrpresso` — a pure-Rust port of the **MR-PRESSO** R package
//! (Verbanck et al. 2018, Nature Genetics), the Mendelian-Randomization
//! Pleiotropy RESidual Sum and Outlier framework.
//!
//! MR-PRESSO evaluates horizontal pleiotropy in multi-instrument Mendelian
//! randomisation from summary statistics, via three components:
//!
//! 1. **Global test** — detection of pleiotropy. Compares the observed
//!    leave-one-out residual sum of squares (RSS) to an empirical null
//!    distribution of RSS computed from simulated exposure/outcome data.
//! 2. **Outlier test** — correction of pleiotropy via outlier removal.
//!    Flags individual instrumental variables whose squared residual exceeds
//!    the empirical null, Bonferroni-corrected.
//! 3. **Distortion test** — tests the distortion of the causal estimate
//!    before vs. after outlier removal.
//!
//! The algorithm is a faithful port of `R/MR-PRESSO-internal.R` /
//! `R/mr_presso.R`. The random draws reproduce **R's** Mersenne-Twister +
//! inversion-`rnorm` bit-exactly ([`rng`]), so the empirical p-values
//! cross-validate against R for a shared `set.seed`. Weighted linear
//! regressions (through the origin) reproduce R's `lm` / `summary.lm`.
//!
//! # References
//!
//! - Verbanck M, Chen C-Y, Neale B, Do R (2018). *Detection of widespread
//!   horizontal pleiotropy in causal relationships inferred from Mendelian
//!   randomization between complex traits and diseases.* Nature Genetics
//!   50:693–698.

#![allow(clippy::needless_range_loop)]

pub mod error;
pub mod linalg;
pub mod mr_presso;
pub mod rng;

pub use error::{MrpressoError, Result};
pub use mr_presso::{MrpressoInput, MrpressoOutput, mr_presso};
