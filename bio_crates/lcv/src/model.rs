//! The LCV driver, ported from `LCV/R/RunLCV.R`.
//!
//! [`run_lcv`] performs a block **jackknife** over the genome, calling
//! [`crate::moments::estimate_k4`] on each leave-one-block-out subset, then
//! scans a **gcp grid** [-1, 0.01, 1] computing a t-distribution likelihood at
//! each point and deriving the posterior mean / SE of gcp, the partial-causality
//! z-score and p-value, and the fully-causal p-values.

use crate::error::{LcvError, Result};
use crate::moments::{estimate_k4, weighted_mean, K4Config, MomentEstimates};
use crate::stats::{dt, pt, pt_two_tailed};

/// Full LCV analysis output — mirrors the named list returned by R `RunLCV`.
#[derive(Debug, Clone)]
pub struct LcvOutput {
    /// Z score for partial genetic causality (zscore >> 0 ⟹ gcp > 0).
    pub zscore: f64,
    /// 2-tailed p-value for the null hypothesis gcp = 0.
    pub pval_gcpzero_2tailed: f64,
    /// Posterior mean gcp (gcp=+1: trait1→trait2; gcp=-1: trait2→trait1).
    pub gcp_pm: f64,
    /// Posterior standard error of gcp.
    pub gcp_pse: f64,
    /// Estimated genetic correlation.
    pub rho_est: f64,
    /// Standard error of the genetic-correlation estimate.
    pub rho_err: f64,
    /// P-values for the null that gcp = +1 (fully causal 1→2) and gcp = -1
    /// (fully causal 2→1), respectively.
    pub pval_fullycausal: [f64; 2],
    /// Z scores for trait 1 and trait 2 being heritable (recommend reporting
    /// only if h²_zscore > 7).
    pub h2_zscore: [f64; 2],
    /// Quality-control warnings (mirrors R `warning()` calls).
    pub warnings: Vec<String>,
}

/// Run the full LCV analysis — R `RunLCV`.
///
/// # Arguments
/// - `ell` — LD scores, sorted by genomic position.
/// - `z1`, `z2` — signed summary statistics (Z-scores or per-normalised-genotype
///   effects; LCV is scale-invariant).
/// - `weights` — regression weights (R default `1 / pmax(1, ell)`).
/// - `no_blocks` — number of jackknife blocks (R default 100).
/// - `cfg` — intercept / threshold / sample-size switches (see [`K4Config`]).
pub fn run_lcv(
    ell: &[f64],
    z1: &[f64],
    z2: &[f64],
    weights: &[f64],
    no_blocks: usize,
    cfg: &K4Config,
) -> Result<LcvOutput> {
    let m = ell.len();
    if z1.len() != m || z2.len() != m || weights.len() != m {
        return Err(LcvError::Input(
            "LD scores and summary statistics should have the same length".into(),
        ));
    }
    if m < no_blocks {
        return Err(LcvError::Input(format!(
            "need at least {no_blocks} SNPs for {no_blocks} jackknife blocks, got {m}"
        )));
    }

    // gcp grid: -1.00, -0.99, …, 0.00, …, 0.99, 1.00  (201 points)
    let grid: Vec<f64> = (-100i32..=100).map(|i| i as f64 / 100.0).collect();

    // ── Jackknife: estimate moments on each leave-one-block-out subset ──
    let block_size = m / no_blocks;
    let mut jk_est: Vec<MomentEstimates> = Vec::with_capacity(no_blocks);

    for jk in 0..no_blocks {
        let start = jk * block_size;
        let end = (jk + 1) * block_size; // exclusive
        // Indices to keep: [0, start) ∪ [end, m)
        let ind: Vec<usize> = (0..start).chain(end..m).collect();

        let ell_jk: Vec<f64> = ind.iter().map(|&i| ell[i]).collect();
        let z1_jk: Vec<f64> = ind.iter().map(|&i| z1[i]).collect();
        let z2_jk: Vec<f64> = ind.iter().map(|&i| z2[i]).collect();
        let w_jk: Vec<f64> = ind.iter().map(|&i| weights[i]).collect();

        let est = estimate_k4(&ell_jk, &z1_jk, &z2_jk, &w_jk, cfg)?;
        if est.rho.is_nan() || est.k41.is_nan() || est.k42.is_nan() {
            return Err(LcvError::Numeric(
                "NaNs produced, probably due to negative heritability estimates. \
                 Check that summary statistics and LD scores are ordered correctly."
                    .into(),
            ));
        }
        jk_est.push(est);
    }

    // ρ estimate and SE (jackknife)
    let rho_vals: Vec<f64> = jk_est.iter().map(|e| e.rho).collect();
    let rho_est = mean(&rho_vals);
    let rho_err = sd(&rho_vals) * (no_blocks as f64 + 1.0).sqrt();
    let flip = rho_est.signum();

    // Asymmetry: k41 -= 3ρ, k42 -= 3ρ  (R: jackknife[,2:3] <- jackknife[,2:3] - 3*jackknife[,1])
    let asym1: Vec<f64> = jk_est.iter().map(|e| e.k41 - 3.0 * e.rho).collect();
    let asym2: Vec<f64> = jk_est.iter().map(|e| e.k42 - 3.0 * e.rho).collect();

    // ── Likelihood grid over gcp ∈ [-1, 1] ──
    let mut likelihood = vec![0.0f64; grid.len()];
    let mut zscore = 0.0f64;
    let mut pval_fullycausal_1 = 0.0f64; // gcp = +1
    let mut pval_fullycausal_2 = 0.0f64; // gcp = -1

    for (kk, &xx) in grid.iter().enumerate() {
        // fx_jk = |rho_jk|^(-xx)
        let fx: Vec<f64> = jk_est.iter().map(|e| e.rho.abs().powf(-xx)).collect();

        // numer_jk = asym1/fx - fx*asym2
        // denom_jk = max(1/|rho_jk|, sqrt(asym1²/fx² + asym2²·fx²))
        // pct_diff_jk = numer / denom
        let mut pct_diff = vec![0.0f64; no_blocks];
        for j in 0..no_blocks {
            let fxj = fx[j];
            let numer = asym1[j] / fxj - fxj * asym2[j];
            let denom = (1.0 / jk_est[j].rho.abs())
                .max((asym1[j] * asym1[j] / (fxj * fxj) + asym2[j] * asym2[j] * fxj * fxj).sqrt());
            pct_diff[j] = numer / denom;
        }

        let statistic = mean(&pct_diff) / sd(&pct_diff) / (no_blocks as f64 + 1.0).sqrt();
        likelihood[kk] = dt(statistic, (no_blocks - 2) as f64);

        // Check endpoints / midpoint (exact float comparisons: grid values are
        // -1.0, 0.0, 1.0 exactly since they are integer / 100).
        if xx == -1.0 {
            pval_fullycausal_2 = pt(-flip * statistic, (no_blocks - 2) as f64);
        } else if xx == 1.0 {
            pval_fullycausal_1 = pt(flip * statistic, (no_blocks - 2) as f64);
        } else if xx == 0.0 {
            zscore = flip * statistic;
        }
    }

    let pval_gcpzero_2tailed = pt_two_tailed(zscore, (no_blocks - 1) as f64);

    // Posterior mean / SE of gcp (likelihood-weighted average over the grid)
    let grid_sq: Vec<f64> = grid.iter().map(|&g| g * g).collect();
    let gcp_pm = weighted_mean(&grid, &likelihood);
    let gcp_pse = (weighted_mean(&grid_sq, &likelihood) - gcp_pm * gcp_pm).sqrt();

    // h² z-scores (s1, s2 ∝ √h²)
    let s1_vals: Vec<f64> = jk_est.iter().map(|e| e.s1).collect();
    let s2_vals: Vec<f64> = jk_est.iter().map(|e| e.s2).collect();
    let h2_zscore_1 = mean(&s1_vals) / sd(&s1_vals) / (no_blocks as f64 + 1.0).sqrt();
    let h2_zscore_2 = mean(&s2_vals) / sd(&s2_vals) / (no_blocks as f64 + 1.0).sqrt();

    // ── Quality-control warnings ──
    let mut warnings = Vec::new();
    if h2_zscore_1 < 4.0 || h2_zscore_2 < 4.0 {
        warnings.push(
            "Very noisy heritability estimates potentially leading to false positives".into(),
        );
    } else if h2_zscore_1 < 7.0 || h2_zscore_2 < 7.0 {
        warnings.push(
            "Borderline noisy heritability estimates potentially leading to false positives".into(),
        );
    }
    if (rho_est / rho_err).abs() < 2.0 {
        warnings.push(
            "No significantly nonzero genetic correlation, potentially leading to conservative p-values".into(),
        );
    }

    Ok(LcvOutput {
        zscore,
        pval_gcpzero_2tailed,
        gcp_pm,
        gcp_pse,
        rho_est,
        rho_err,
        pval_fullycausal: [pval_fullycausal_1, pval_fullycausal_2],
        h2_zscore: [h2_zscore_1, h2_zscore_2],
        warnings,
    })
}

// ─────────────────────────── sample statistics ───────────────────────────

/// Arithmetic mean (R `mean`).
#[inline]
fn mean(x: &[f64]) -> f64 {
    x.iter().sum::<f64>() / x.len() as f64
}

/// Sample standard deviation with n-1 divisor (R `sd`).
#[inline]
fn sd(x: &[f64]) -> f64 {
    let n = x.len();
    if n < 2 {
        return 0.0;
    }
    let mu = mean(x);
    let ss = x.iter().map(|&v| (v - mu) * (v - mu)).sum::<f64>();
    (ss / (n as f64 - 1.0)).sqrt()
}
