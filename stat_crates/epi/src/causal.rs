//! Causal inference: IPTW and PSM.
//!
//! # IPTW (Inverse Probability of Treatment Weighting)
//!
//! Estimates the Average Treatment Effect (ATE) by reweighting observations
//! with the inverse propensity score:
//!
//! ```text
//! wᵢ = Tᵢ/p̂ᵢ + (1 − Tᵢ)/(1 − p̂ᵢ)           (unstabilized)
//! wᵢ = Tᵢ·p̄/p̂ᵢ + (1−Tᵢ)·(1−p̄)/(1−p̂ᵢ)   (stabilized)
//! ```
//!
//! The treatment effect is then the coefficient on `T` in a weighted
//! regression of `Y` on `T` (WLS).
//!
//! # PSM (Propensity Score Matching)
//!
//! Matches each treated subject to the nearest control by propensity score
//! (greedy nearest-neighbour, with optional caliper). The ATT is the mean
//! outcome difference within matched pairs.

use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

use statkit::regression;

use crate::error::{EpiError, Result};

// ── IPTW ───────────────────────────────────────────────────────────────────

/// Options for IPTW estimation.
#[derive(Debug, Clone)]
pub struct IptwOptions {
    /// Use stabilized weights (default true). When false, uses unstabilized
    /// weights `1/p̂` and `1/(1−p̂)`.
    pub stabilized: bool,
    /// Bootstrap iterations for the treatment effect SE (default 1000).
    pub n_bootstrap: usize,
    /// Random seed.
    pub seed: u64,
    /// Trim propensity scores to [trim, 1−trim] to avoid extreme weights
    /// (default 0.01).
    pub trim: f64,
}

impl Default for IptwOptions {
    fn default() -> Self {
        Self {
            stabilized: true,
            n_bootstrap: 1000,
            seed: 42,
            trim: 0.01,
        }
    }
}

/// Result of an IPTW analysis.
#[derive(Debug, Clone)]
pub struct IptwResult {
    /// Estimated Average Treatment Effect (coefficient on T in weighted Y ~ T).
    pub ate: f64,
    /// ATE standard error from bootstrap.
    pub ate_se: f64,
    /// ATE bootstrap 95% CI lower.
    pub ate_ci_lower: f64,
    /// ATE bootstrap 95% CI upper.
    pub ate_ci_upper: f64,
    /// Fitted propensity scores `p̂ᵢ`.
    pub propensity_scores: Vec<f64>,
    /// IPTW weights.
    pub weights: Vec<f64>,
    /// Mean propensity score among treated.
    pub ps_treated_mean: f64,
    /// Mean propensity score among controls.
    pub ps_control_mean: f64,
    /// Effective sample size (Kish's formula): (Σw)² / Σ(w²).
    pub ess: f64,
    /// Number of observations.
    pub n_obs: usize,
    /// Number of treated.
    pub n_treated: usize,
}

/// Estimate the Average Treatment Effect via IPTW.
///
/// `treatment` is binary (0/1), `outcome` is continuous, and `covariates` are
/// the confounders used to estimate propensity scores.
pub fn iptw(
    treatment: &[f64],
    outcome: &[f64],
    covariates: &[&[f64]],
    opts: &IptwOptions,
) -> Result<IptwResult> {
    let n = treatment.len();
    if n == 0 || outcome.len() != n {
        return Err(EpiError::DimensionMismatch {
            a: n,
            b: outcome.len(),
        });
    }
    for &t in treatment {
        if t != 0.0 && t != 1.0 {
            return Err(EpiError::Numerical(format!(
                "treatment must be 0/1, got {t}"
            )));
        }
    }
    for c in covariates.iter() {
        if c.len() != n {
            return Err(EpiError::DimensionMismatch { a: n, b: c.len() });
        }
    }

    let n_treated = treatment.iter().filter(|&&t| t == 1.0).count();
    let p_bar = n_treated as f64 / n as f64;

    // ── Step 1: Estimate propensity scores via logistic regression ────────
    let cov_slices: Vec<&[f64]> = covariates.iter().copied().collect();
    let ps_fit = regression::logistic(&cov_slices, treatment, true).map_err(epi_from_stat)?;

    // Trim propensity scores.
    let trim_lo = opts.trim;
    let trim_hi = 1.0 - opts.trim;
    let propensity_scores: Vec<f64> = ps_fit
        .fitted
        .iter()
        .map(|&p| p.clamp(trim_lo, trim_hi))
        .collect();

    // ── Step 2: Compute IPTW weights ─────────────────────────────────────
    let weights: Vec<f64> = (0..n)
        .map(|i| {
            let ps = propensity_scores[i];
            if opts.stabilized {
                treatment[i] * p_bar / ps + (1.0 - treatment[i]) * (1.0 - p_bar) / (1.0 - ps)
            } else {
                treatment[i] / ps + (1.0 - treatment[i]) / (1.0 - ps)
            }
        })
        .collect();

    // ── Step 3: WLS — Y ~ T with IPTW weights ────────────────────────────
    let (ate, _fit) = weighted_outcome_effect(treatment, outcome, &weights)?;

    // ── Step 4: Bootstrap SE ─────────────────────────────────────────────
    let mut rng = ChaCha8Rng::seed_from_u64(opts.seed);
    let indices: Vec<usize> = (0..n).collect();
    let mut boot_ate = Vec::with_capacity(opts.n_bootstrap);

    for _ in 0..opts.n_bootstrap {
        let boot_idx: Vec<usize> = (0..n)
            .map(|_| indices[(rng.random::<f64>() * n as f64) as usize])
            .collect();
        let t_b: Vec<f64> = boot_idx.iter().map(|&i| treatment[i]).collect();
        let y_b: Vec<f64> = boot_idx.iter().map(|&i| outcome[i]).collect();
        let cov_b: Vec<Vec<f64>> = covariates
            .iter()
            .map(|c| boot_idx.iter().map(|&i| c[i]).collect())
            .collect();
        let cov_b_slices: Vec<&[f64]> = cov_b.iter().map(|v| v.as_slice()).collect();

        // Refit PS model and weights on bootstrap sample.
        if let Ok(ps_b) = regression::logistic(&cov_b_slices, &t_b, true) {
            let n_t_b = t_b.iter().filter(|&&t| t == 1.0).count();
            let pb_b = n_t_b as f64 / n as f64;
            let w_b: Vec<f64> = (0..n)
                .map(|i| {
                    let ps = ps_b.fitted[i].clamp(trim_lo, trim_hi);
                    if opts.stabilized {
                        t_b[i] * pb_b / ps + (1.0 - t_b[i]) * (1.0 - pb_b) / (1.0 - ps)
                    } else {
                        t_b[i] / ps + (1.0 - t_b[i]) / (1.0 - ps)
                    }
                })
                .collect();
            if let Ok((ate_b, _)) = weighted_outcome_effect(&t_b, &y_b, &w_b) {
                boot_ate.push(ate_b);
            }
        }
    }

    let (ci_lo, ci_hi) = percentile_ci(&boot_ate);
    let se = if boot_ate.len() > 1 {
        let mean = boot_ate.iter().sum::<f64>() / boot_ate.len() as f64;
        (boot_ate.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (boot_ate.len() - 1) as f64)
            .sqrt()
    } else {
        f64::NAN
    };

    // Effective sample size (Kish).
    let sum_w: f64 = weights.iter().sum();
    let sum_w2: f64 = weights.iter().map(|w| w * w).sum();
    let ess = if sum_w2 > 0.0 {
        sum_w * sum_w / sum_w2
    } else {
        0.0
    };

    let ps_treated_mean: f64 = (0..n)
        .filter(|&i| treatment[i] == 1.0)
        .map(|i| propensity_scores[i])
        .sum::<f64>()
        / n_treated as f64;
    let n_control = n - n_treated;
    let ps_control_mean = if n_control > 0 {
        (0..n)
            .filter(|&i| treatment[i] == 0.0)
            .map(|i| propensity_scores[i])
            .sum::<f64>()
            / n_control as f64
    } else {
        f64::NAN
    };

    Ok(IptwResult {
        ate,
        ate_se: se,
        ate_ci_lower: ci_lo,
        ate_ci_upper: ci_hi,
        propensity_scores,
        weights,
        ps_treated_mean,
        ps_control_mean,
        ess,
        n_obs: n,
        n_treated,
    })
}

// ── PSM ────────────────────────────────────────────────────────────────────

/// Options for PSM estimation.
#[derive(Debug, Clone)]
pub struct PsmOptions {
    /// Caliper width (maximum PS distance for a match). `None` = no caliper
    /// (default: 0.2 × SD of logit(PS), per Austin 2011).
    pub caliper: Option<f64>,
    /// Matching ratio: number of controls per treated (default 1).
    pub ratio: usize,
    /// Match with replacement (default false).
    pub replacement: bool,
    /// Bootstrap iterations for ATT SE (default 1000).
    pub n_bootstrap: usize,
    /// Random seed.
    pub seed: u64,
}

impl Default for PsmOptions {
    fn default() -> Self {
        Self {
            caliper: None,
            ratio: 1,
            replacement: false,
            n_bootstrap: 1000,
            seed: 42,
        }
    }
}

/// Result of a PSM analysis.
#[derive(Debug, Clone)]
pub struct PsmResult {
    /// Estimated Average Treatment Effect on the Treated.
    pub att: f64,
    /// ATT bootstrap SE.
    pub att_se: f64,
    /// ATT bootstrap 95% CI lower.
    pub att_ci_lower: f64,
    /// ATT bootstrap 95% CI upper.
    pub att_ci_upper: f64,
    /// Number of matched pairs (or matched sets).
    pub n_matched: usize,
    /// Number of treated.
    pub n_treated: usize,
    /// Number of controls.
    pub n_control: usize,
    /// Fitted propensity scores.
    pub propensity_scores: Vec<f64>,
    /// Match indices: for each treated subject, the index of the matched control.
    pub matches: Vec<(usize, Vec<usize>)>,
}

/// Estimate ATT via nearest-neighbour propensity score matching.
pub fn psm(
    treatment: &[f64],
    outcome: &[f64],
    covariates: &[&[f64]],
    opts: &PsmOptions,
) -> Result<PsmResult> {
    let n = treatment.len();
    if n == 0 || outcome.len() != n {
        return Err(EpiError::DimensionMismatch {
            a: n,
            b: outcome.len(),
        });
    }
    for &t in treatment {
        if t != 0.0 && t != 1.0 {
            return Err(EpiError::Numerical(format!(
                "treatment must be 0/1, got {t}"
            )));
        }
    }

    // ── Propensity scores ────────────────────────────────────────────────
    let cov_slices: Vec<&[f64]> = covariates.iter().copied().collect();
    let ps_fit = regression::logistic(&cov_slices, treatment, true).map_err(epi_from_stat)?;
    let ps = &ps_fit.fitted;

    // Default caliper: 0.2 × SD(logit(PS)).
    let caliper = opts.caliper.unwrap_or_else(|| {
        let logit_ps: Vec<f64> = ps.iter().map(|p| (p / (1.0 - p)).ln()).collect();
        let mean = logit_ps.iter().sum::<f64>() / logit_ps.len() as f64;
        let var = logit_ps.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / logit_ps.len() as f64;
        0.2 * var.sqrt()
    });

    let treated_idx: Vec<usize> = (0..n).filter(|&i| treatment[i] == 1.0).collect();
    let control_idx: Vec<usize> = (0..n).filter(|&i| treatment[i] == 0.0).collect();

    let n_treated = treated_idx.len();
    let n_control = control_idx.len();

    // ── Greedy nearest-neighbour matching ────────────────────────────────
    let mut used: Vec<bool> = vec![false; n];
    let mut matches: Vec<(usize, Vec<usize>)> = Vec::new();

    for &t_i in &treated_idx {
        let mut distances: Vec<(usize, f64)> = control_idx
            .iter()
            .filter(|&&c_i| opts.replacement || !used[c_i])
            .map(|&c_i| (c_i, (ps[t_i] - ps[c_i]).abs()))
            .filter(|(_, d)| *d <= caliper)
            .collect();
        distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        let matched: Vec<usize> = distances
            .iter()
            .take(opts.ratio)
            .map(|(c_i, _)| {
                used[*c_i] = true;
                *c_i
            })
            .collect();

        if !matched.is_empty() {
            matches.push((t_i, matched));
        }
    }

    // ── ATT = mean(Y_treated - mean(Y_matched_control)) ─────────────────
    let n_matched = matches.len();
    if n_matched == 0 {
        return Err(EpiError::Numerical(
            "no matched pairs found — try widening the caliper".to_string(),
        ));
    }

    let att: f64 = matches
        .iter()
        .map(|(t_i, controls)| {
            let y_c_mean =
                controls.iter().map(|&c| outcome[c]).sum::<f64>() / controls.len() as f64;
            outcome[*t_i] - y_c_mean
        })
        .sum::<f64>()
        / n_matched as f64;

    // ── Bootstrap SE ─────────────────────────────────────────────────────
    let mut rng = ChaCha8Rng::seed_from_u64(opts.seed);
    let mut boot_att = Vec::with_capacity(opts.n_bootstrap);

    for _ in 0..opts.n_bootstrap {
        // Simplified bootstrap: resample matched pairs with replacement.
        let pairs: Vec<f64> = (0..n_matched)
            .map(|_| {
                let j = (rng.random::<f64>() * n_matched as f64) as usize;
                let (t_i, controls) = &matches[j];
                let y_c_mean =
                    controls.iter().map(|&c| outcome[c]).sum::<f64>() / controls.len() as f64;
                outcome[*t_i] - y_c_mean
            })
            .collect();
        boot_att.push(pairs.iter().sum::<f64>() / pairs.len() as f64);
    }

    let (ci_lo, ci_hi) = percentile_ci(&boot_att);
    let se = if boot_att.len() > 1 {
        let mean = boot_att.iter().sum::<f64>() / boot_att.len() as f64;
        (boot_att.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (boot_att.len() - 1) as f64)
            .sqrt()
    } else {
        f64::NAN
    };

    Ok(PsmResult {
        att,
        att_se: se,
        att_ci_lower: ci_lo,
        att_ci_upper: ci_hi,
        n_matched,
        n_treated,
        n_control,
        propensity_scores: ps.clone(),
        matches,
    })
}

// ── Shared helpers ─────────────────────────────────────────────────────────

fn weighted_outcome_effect(
    treatment: &[f64],
    outcome: &[f64],
    weights: &[f64],
) -> Result<(f64, regression::Regression)> {
    let fit = regression::wls(&[treatment], outcome, weights, true).map_err(epi_from_stat)?;
    // Coefficient index 0 = intercept, 1 = treatment effect.
    Ok((fit.coefficients[1], fit))
}

fn percentile_ci(boot: &[f64]) -> (f64, f64) {
    if boot.len() < 2 {
        return (f64::NAN, f64::NAN);
    }
    let mut sorted = boot.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = sorted.len();
    let lo_idx = (0.025 * n as f64).floor() as usize;
    let hi_idx = (0.975 * n as f64).ceil() as usize;
    (sorted[lo_idx.min(n - 1)], sorted[hi_idx.min(n - 1)])
}

fn epi_from_stat(e: statkit::StatError) -> EpiError {
    EpiError::Numerical(e.to_string())
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * b.abs().max(1.0)
    }

    fn make_data(n: usize, te: f64, seed: u64) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
        // Random confounder c affects both treatment and outcome.
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let c: Vec<f64> = (0..n).map(|_| rng.random::<f64>() * 2.0 - 1.0).collect();
        let treatment: Vec<f64> = c
            .iter()
            .map(|&ci| {
                let ps = 1.0 / (1.0 + (-(-0.3 + ci)).exp()); // logistic(-0.3 + c)
                if rng.random::<f64>() < ps { 1.0 } else { 0.0 }
            })
            .collect();
        let outcome: Vec<f64> = (0..n)
            .map(|i| te * treatment[i] + 2.0 * c[i] + rng.random::<f64>() * 0.5 - 0.25)
            .collect();
        (treatment, outcome, c)
    }

    #[test]
    fn iptw_recovers_treatment_effect() {
        let n = 300;
        let te = 1.5;
        let (t, y, c) = make_data(n, te, 42);
        let opts = IptwOptions {
            n_bootstrap: 100,
            ..Default::default()
        };
        let result = iptw(&t, &y, &[&c[..]], &opts).unwrap();
        assert!(
            approx_eq(result.ate, te, 0.3),
            "ATE: got {:.3}, expected ~{te}",
            result.ate
        );
        assert!(result.ess > 0.0);
    }

    #[test]
    fn iptw_stabilized_weights_positive() {
        let n = 200;
        let (t, y, c) = make_data(n, 1.0, 42);
        let opts = IptwOptions::default();
        let result = iptw(&t, &y, &[&c[..]], &opts).unwrap();
        assert!(
            result.weights.iter().all(|w| *w > 0.0),
            "weights must be positive"
        );
    }

    #[test]
    fn psm_recovers_att() {
        let n = 300;
        let te = 1.5;
        let (t, y, c) = make_data(n, te, 42);
        let opts = PsmOptions {
            n_bootstrap: 100,
            ..Default::default()
        };
        let result = psm(&t, &y, &[&c[..]], &opts).unwrap();
        assert!(result.n_matched > 0, "should have matched pairs");
        assert!(
            approx_eq(result.att, te, 0.5),
            "ATT: got {:.3}, expected ~{te}",
            result.att
        );
    }

    #[test]
    fn psm_no_match_with_tight_caliper() {
        let n = 100;
        let (t, y, c) = make_data(n, 1.0, 42);
        let opts = PsmOptions {
            caliper: Some(0.0001),
            n_bootstrap: 10,
            ..Default::default()
        };
        let result = psm(&t, &y, &[&c[..]], &opts);
        // May or may not error depending on data, but should not panic.
        match result {
            Ok(r) => {
                let _ = r;
            }
            Err(_) => {}
        }
    }

    #[test]
    fn iptw_rejects_non_binary() {
        let t = vec![0.0, 0.5, 1.0];
        let y = vec![1.0, 2.0, 3.0];
        let c = vec![1.0, 2.0, 3.0];
        assert!(iptw(&t, &y, &[&c[..]], &IptwOptions::default()).is_err());
    }
}
