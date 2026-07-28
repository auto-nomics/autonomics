//! `lava` — a pure-Rust port of the **algorithm API** of the R package
//! [`LAVA`](https://github.com/josefinwerme/LAVA) (Werme et al., *Nat Genet* 2022),
//! for **local genetic correlation** analysis: per-region local heritability
//! (univariate), bivariate local genetic correlation, partial genetic
//! correlation, and multiple regression of local genetic components.
//!
//! This is a **library only**. It ports the four core GWAS analyses
//! (`run.univ`, `run.bivar`, `run.pcor`, `run.multireg`), the combined
//! `run.univ.bivar` driver, locus meta-analysis (`meta.analyse.locus`), the
//! full input-processing / allele-alignment pipeline, PLINK reference loading,
//! LD decomposition, and the simulation-based inference (non-central Wishart
//! p-values and confidence intervals, faithful to `matrixsampling::rwishart`).
//!
//! The eQTL/sQTL subsystem (`process.eqtl.*`) and the custom `.bcor` binary LD
//! format are **out of scope** for this port (no test fixtures); they are
//! candidates for future work.
//!
//! # Faithfulness conventions
//!
//! - **NA** is `f64::NAN`; "valid" means `!is_nan()` (matching R's `!is.na()`).
//! - Linear algebra runs on [`faer`] (no LAPACK/MKL).
//! - Normal distribution functions (`pnorm`/`qnorm`/`pnorm_sf`/`dnorm`) are
//!   hand-implemented (reproducing R's `pnorm`/`qnorm`); the χ² and F survival
//!   functions use the maintained [`statrs`] crate.
//! - The non-central Wishart sampler reproduces `matrixsampling::rwishart`
//!   distributionally (`E[W] = nu·Sigma + Theta`); the RNG is a seeded
//!   `ChaCha8Rng` so Monte-Carlo p-values are reproducible. R and Rust cannot
//!   share an RNG stream, so simulation p-values / CIs are validated for
//!   convergence, while point estimates are matched to ~6 sig figs.
//!
//! | Module        | R source                        | Contents                                                       |
//! |---------------|---------------------------------|----------------------------------------------------------------|
//! | [`stats`]     | `stats::pnorm/qnorm/pchisq/pf`  | distribution wrappers + `cov2cor`                              |
//! | [`input`]     | `R/input_processing.R`          | sumstats / info / overlap / loci read, harmonize               |
//! | [`align`]     | `R/alignment.R`                 | allele-pair coding, sign-flip alignment                        |
//! | [`plink`]     | `src/load_plink.cpp`            | PLINK `.bed/.bim/.fam` reader, geno filter, LD cor & freq      |
//! | [`decompose`] | `decompose.ld`                  | SVD / eigen LD decomposition, PC pruning → R matrix            |
//! | [`binary`]    | `R/binary_processing.R`         | marginal Pearson r for binary phenotypes (logistic search)     |
//! | [`locus`]     | `process.locus`                 | delta / sigma / omega / h², sample overlap, failed drop        |
//! | [`wishart`]   | `R/wishart.R` + matrixsampling  | non-central Wishart sampler, adaptive simulation p-values      |
//! | [`ci`]        | `R/confidence_intervals.R`      | Wishart-quantile confidence intervals                          |
//! | [`pcor`]      | `R/partial_corrs.R`             | partial correlation / covariance / variance                    |
//! | [`analysis`]  | `R/analysis_functions.R`        | `run_univ/bivar/multireg/pcor`, `estimate_params`              |

#![allow(clippy::needless_range_loop)]

pub mod align;
pub mod analysis;
pub mod binary;
pub mod ci;
pub mod decompose;
pub mod error;
pub mod input;
pub mod locus;
pub mod pcor;
pub mod plink;
pub mod stats;
pub mod wishart;

pub use error::{LavaError, Result};

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

/// Default seed for the Monte-Carlo inference RNG (reproducible p-values/CIs).
pub const DEFAULT_RNG_SEED: u64 = 0x001A_7A4E_7AC0_FFEE;

/// Build a seeded RNG (ChaCha8) for reproducible Wishart sampling.
pub fn rng(seed: u64) -> ChaCha8Rng {
    ChaCha8Rng::seed_from_u64(seed)
}
