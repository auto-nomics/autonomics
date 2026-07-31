//! `cpassoc` — a pure-Rust port of **CPASSOC** (Cross-Phenotype Association)
//! by Zhu & Feng (*AJHG* 2015, doi:10.1016/j.ajhg.2014.11.014).
//!
//! CPASSOC combines GWAS summary statistics from multiple traits (and/or
//! cohorts) into two cross-phenotype association tests:
//!
//! - **SHom** — powerful when genetic effects are **homogeneous** across
//!   traits/cohorts. Follows χ²₁ under the null.
//! - **SHet** — powerful when **heterogeneous** effects exist (a variant
//!   affects only a subset of traits, or has opposite directions). Its null
//!   distribution is approximated by a 3-parameter shifted gamma `Gamma(k,θ)+a`
//!   estimated by Monte-Carlo simulation.
//!
//! This is a **library only**. It is a 1:1 faithful port of the R reference
//! (`reference/FunctionSet.R`):
//!   - `Non_Trucated_TestScore` → [`stats::shom`]
//!   - `Trucated_TestScore`     → [`stats::shet`]
//!   - `EstimateGamma`          → [`gamma::estimate_gamma`]
//!   - `EmpDist`                → [`gamma::emp_dist`]
//!
//! # Faithfulness conventions
//!
//! - Linear algebra runs on [`faer`]; the Moore-Penrose pseudoinverse (`ginv`)
//!   is reproduced via SVD (R uses `MASS::ginv`).
//! - The χ² and gamma survival functions use [`statrs`], matching R's
//!   `pchisq(..., lower.tail=F)` and `pgamma(..., lower.tail=F)`.
//! - MVN null simulation uses a seeded `ChaCha8Rng` with Cholesky-based
//!   sampling (matching `MASS::mvrnorm` distributionally). R and Rust cannot
//!   share an RNG stream, so gamma-fitting is validated for convergence while
//!   point estimates (SHom, SHet statistics, correlation matrix) are matched
//!   to R to ~6+ sig figs.
//! - R's `var()` uses the n-1 divisor; this crate follows the same convention.
//!
//! | Module      | R source                          | Contents                                              |
//! |-------------|-----------------------------------|-------------------------------------------------------|
//! | [`linalg`]  | `MASS::ginv`                      | Moore-Penrose pseudoinverse via SVD                   |
//! | [`stats`]   | `Non_Trucated_TestScore`, `Trucated_TestScore` | SHom, SHet statistics                       |
//! | [`mvn`]     | `MASS::mvrnorm`                   | multivariate normal sampling (Cholesky transform)     |
//! | [`gamma`]   | `EstimateGamma`, `EmpDist`        | shifted-gamma fitting, empirical null distribution    |
//! | [`distr`]   | `pchisq`, `pgamma`                | χ² and gamma survival functions (p-values)            |
//! | [`input`]   | `cor(X)`                          | correlation matrix estimation (Eq. 6), result types   |

#![allow(clippy::needless_range_loop)]

pub mod distr;
pub mod error;
pub mod gamma;
pub mod input;
pub mod linalg;
pub mod mvn;
pub mod stats;

pub use error::{CpassocError, Result};

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

/// Build a seeded ChaCha8 RNG for reproducible simulation.
pub fn rng(seed: u64) -> ChaCha8Rng {
    ChaCha8Rng::seed_from_u64(seed)
}

// ─── High-level analysis driver ─────────────────────────────────────────────

use faer::Mat;
use gamma::GammaParams;

/// Configuration for a full CPASSOC analysis.
#[derive(Clone, Debug)]
pub struct CpassocConfig {
    /// Per-trait sample sizes used as weights (length K).
    /// In practice these are `sqrt(n_j)` or raw `n_j` — only relative magnitude
    /// matters since weights are normalised internally.
    pub sample_size: Vec<f64>,
    /// Options for SHet (SHom has no options).
    pub shet_opts: stats::ShetOptions,
    /// Number of MVN null draws for gamma fitting (R default: 1e4–1e6).
    pub n_sim: usize,
    /// RNG seed for reproducible null simulation.
    pub seed: u64,
}

impl Default for CpassocConfig {
    fn default() -> Self {
        Self {
            sample_size: Vec::new(),
            shet_opts: stats::ShetOptions::default(),
            n_sim: 10_000,
            seed: 42,
        }
    }
}

/// Run a complete CPASSOC analysis: compute SHom and SHet statistics for all
/// SNPs, fit the null gamma distribution for SHet, and return per-SNP p-values.
///
/// - `x` — M×K matrix of summary statistics (Z-scores).
/// - `corr` — K×K correlation matrix (from [`input::corr_matrix`] or external).
/// - `config` — analysis configuration.
/// - `snp_ids` — optional SNP identifiers (length M); if empty, indices are used.
pub fn run_cpassoc(
    x: &Mat<f64>,
    corr: &Mat<f64>,
    config: &CpassocConfig,
    snp_ids: Option<&[String]>,
) -> Vec<input::CpassocResult> {
    // 1. Compute SHom statistics + p-values
    let shom_stats = stats::shom(x, &config.sample_size, corr);
    // 2. Compute SHet statistics
    let shet_stats = stats::shet(x, &config.sample_size, corr, config.shet_opts);
    // 3. Fit null gamma for SHet
    let gamma_params = gamma::estimate_gamma(
        config.n_sim,
        &config.sample_size,
        corr,
        config.shet_opts,
        config.seed,
    );

    let m = x.nrows();
    (0..m)
        .map(|i| {
            let id = match snp_ids {
                Some(ids) => ids[i].clone(),
                None => i.to_string(),
            };
            input::CpassocResult {
                id,
                shom: shom_stats[i],
                shet: shet_stats[i],
                shom_p: distr::shom_pvalue(shom_stats[i]),
                shet_p: distr::shet_pvalue(shet_stats[i], gamma_params),
            }
        })
        .collect()
}

/// Fit-only: compute the SHet null gamma parameters without running SNPs.
pub fn fit_null(
    sample_size: &[f64],
    corr: &Mat<f64>,
    opts: stats::ShetOptions,
    n_sim: usize,
    seed: u64,
) -> GammaParams {
    gamma::estimate_gamma(n_sim, sample_size, corr, opts, seed)
}
