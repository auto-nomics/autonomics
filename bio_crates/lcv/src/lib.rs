//! `lcv` — a pure-Rust port of the **Latent Causal Variable** model
//! (O'Connor & Price, *Nature Genetics* 2018, doi:10.1038/s41588-018-0099-7)
//! for distinguishing **genetic causation** from **genetic correlation**
//! using only GWAS summary statistics and LD scores.
//!
//! LCV posits a latent factor `L` with mixed causal effects on two traits,
//! parameterised by the **genetic causality proportion** (gcp):
//!   - `gcp = +1` ⟹ trait 1 → trait 2 (fully causal);
//!   - `gcp = -1` ⟹ trait 2 → trait 1;
//!   - `gcp = 0`  ⟹ no genetic causality (pleiotropy / shared confounding only).
//!
//! The method infers gcp from the **asymmetry of mixed 4th moments** of the
//! (LD-score-regressed) marginal effect-size estimates, using a block
//! jackknife for standard errors and a t-distribution likelihood on the
//! gcp grid [-1, 1].
//!
//! This is a **library only**. It is a 1:1 faithful port of the R reference:
//!   - `LCV/R/RunLCV.R` — the main `RunLCV` driver (jackknife → likelihood grid).
//!   - `LCV/R/MomentFunctions.R` — `WeightedRegression`, `WeightedMean`,
//!     `EstimateK4` (LDSC regression + mixed 4th moments).
//!   - `LCV/R/SimulateLCV.R` — simulated summary statistics under the LCV model.
//!
//! LCV needs only LD scores and Z-scores (no LD matrices / PLINK reference),
//! so this crate is self-contained — it does **not** depend on [`lava`] or
//! [`hdl`].
//!
//! # Faithfulness conventions
//!
//! - Linear algebra runs on [`faer`] (no LAPACK/MKL); the only "linear solve"
//!   is the tiny 1- or 2-column weighted least-squares of LDSC regression.
//! - Student's t density (`dt`) and CDF (`pt`) use [`statrs`], matching R.
//! - R's `sd()` / Matlab's `std()` use the n-1 divisor; this crate follows the
//!   same convention everywhere.
//! - The LCV estimator is fully deterministic (no RNG); the [`simulate`]
//!   module uses a seeded `ChaCha8Rng` for reproducible test data.
//!
//! | Module       | R source                       | Contents                                                  |
//! |--------------|--------------------------------|-----------------------------------------------------------|
//! | [`stats`]    | `dt` / `pt`                    | Student's t density and CDF (matching R)                  |
//! | [`moments`]  | `MomentFunctions.R`            | weighted regression / mean, `EstimateK4` (LDsc + k4)      |
//! | [`model`]    | `RunLCV.R`                     | jackknife, gcp likelihood grid, posterior, p-values       |
//! | [`simulate`] | `SimulateLCV.R`                | simulate summary statistics under the LCV model           |

#![allow(clippy::needless_range_loop)]

pub mod error;
pub mod model;
pub mod moments;
pub mod simulate;
pub mod stats;

pub use error::{LcvError, Result};

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

/// Default number of jackknife blocks (matches the R / Matlab reference).
pub const DEFAULT_NO_BLOCKS: usize = 100;

/// Default significance-chisq threshold for excluding GWS SNPs when computing
/// the LDSC intercept (R uses `.Machine$integer.max`; Matlab uses `inf`).
pub const DEFAULT_SIG_THRESHOLD: f64 = f64::INFINITY;

/// Build a seeded ChaCha8 RNG for reproducible simulation.
pub fn rng(seed: u64) -> ChaCha8Rng {
    ChaCha8Rng::seed_from_u64(seed)
}
