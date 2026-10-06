//! Causal mediation analysis (counterfactual framework).
//!
//! Implements the two-model approach (VanderWeele 2015) for decomposing the
//! total effect of an exposure `X` on an outcome `Y` through a mediator `M`
//! into:
//!
//! - **NDE** (Natural Direct Effect): effect of `X` on `Y` not mediated through `M`.
//! - **NIE** (Natural Indirect Effect): effect of `X` on `Y` through `M`.
//! - **TE** = NDE + NIE (Total Effect).
//! - **Proportion Mediated** = NIE / TE.
//!
//! # Models
//!
//! **Mediator model** (OLS): `M = α₀ + α₁·X + α₂·C + ε_M`
//!
//! **Outcome model** (OLS): `Y = β₀ + β₁·X + β₂·M + β₃·(X·M) + β₄·C + ε_Y`
//!
//! # Decomposition (no interaction, β₃ = 0)
//!
//! For a binary exposure (X: 0 → 1):
//!
//! ```text
//! NDE = β₁           (direct path X → Y)
//! NIE = β₂ · α₁      (indirect path X → M → Y)
//! TE  = β₁ + β₂·α₁
//! ```
//!
//! With interaction (β₃ ≠ 0), for X: 0 → 1 at covariate means C̄:
//!
//! ```text
//! NDE = β₁ + β₃·(α₀ + Σ α_c·C̄)   (setting M to its value under X=0)
//! NIE = (β₂ + β₃) · α₁
//! ```
//!
//! Bootstrap confidence intervals are computed by resampling the full dataset
//! B times and refitting both models on each resample.

use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

use statkit::regression;

use crate::error::{EpiError, Result};

// ── Options ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MediationOptions {
    /// Bootstrap iterations (default 1000).
    pub n_bootstrap: usize,
    /// Random seed.
    pub seed: u64,
    /// Exposure level for "treated" (default 1.0).
    pub x_treated: f64,
    /// Exposure level for "control" (default 0.0).
    pub x_control: f64,
}

impl Default for MediationOptions {
    fn default() -> Self {
        Self {
            n_bootstrap: 1000,
            seed: 42,
            x_treated: 1.0,
            x_control: 0.0,
        }
    }
}

// ── Result ─────────────────────────────────────────────────────────────────

/// Result of a causal mediation analysis.
#[derive(Debug, Clone)]
pub struct MediationResult {
    /// Natural Direct Effect (point estimate).
    pub nde: f64,
    /// NDE bootstrap 95% CI lower.
    pub nde_ci_lower: f64,
    /// NDE bootstrap 95% CI upper.
    pub nde_ci_upper: f64,
    /// Natural Indirect Effect (point estimate).
    pub nie: f64,
    /// NIE bootstrap 95% CI lower.
    pub nie_ci_lower: f64,
    /// NIE bootstrap 95% CI upper.
    pub nie_ci_upper: f64,
    /// Total Effect = NDE + NIE.
    pub te: f64,
    /// TE bootstrap 95% CI lower.
    pub te_ci_lower: f64,
    /// TE bootstrap 95% CI upper.
    pub te_ci_upper: f64,
    /// Proportion mediated = NIE / TE (NaN if TE ≈ 0).
    pub prop_mediated: f64,
    /// Prop mediated bootstrap 95% CI lower.
    pub pm_ci_lower: f64,
    /// Prop mediated bootstrap 95% CI upper.
    pub pm_ci_upper: f64,
    /// Mediator model coefficient α₁ (effect of X on M).
    pub alpha_x: f64,
    /// Outcome model coefficient β₁ (direct effect of X on Y).
    pub beta_x: f64,
    /// Outcome model coefficient β₂ (effect of M on Y).
    pub beta_m: f64,
    /// Outcome model interaction β₃ (X×M), if interaction term included.
    pub beta_xm: f64,
    /// Number of bootstrap iterations completed.
    pub n_bootstrap: usize,
    /// Number of observations.
    pub n_obs: usize,
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Perform a causal mediation analysis.
///
/// `x` is the exposure, `m` the mediator, `y` the outcome, and `covariates`
/// optional confounders adjusted for in both models. An interaction term
/// `X × M` is included in the outcome model when `interaction = true`.
pub fn mediation(
    x: &[f64],
    m: &[f64],
    y: &[f64],
    covariates: &[&[f64]],
    interaction: bool,
    opts: &MediationOptions,
) -> Result<MediationResult> {
    let n = x.len();
    if n == 0 || m.len() != n || y.len() != n {
        return Err(EpiError::DimensionMismatch {
            a: n,
            b: m.len().max(y.len()),
        });
    }
    for c in covariates.iter() {
        if c.len() != n {
            return Err(EpiError::DimensionMismatch { a: n, b: c.len() });
        }
    }

    // ── Point estimates from full data ────────────────────────────────
    let (nde, nie, te, alpha_x, beta_x, beta_m, beta_xm) =
        fit_and_decompose(x, m, y, covariates, interaction, opts)?;

    // ── Bootstrap ──────────────────────────────────────────────────────
    let mut rng = ChaCha8Rng::seed_from_u64(opts.seed);
    let indices: Vec<usize> = (0..n).collect();

    let mut boot_nde = Vec::with_capacity(opts.n_bootstrap);
    let mut boot_nie = Vec::with_capacity(opts.n_bootstrap);
    let mut boot_te = Vec::with_capacity(opts.n_bootstrap);
    let mut boot_pm = Vec::with_capacity(opts.n_bootstrap);

    for _ in 0..opts.n_bootstrap {
        let boot_idx: Vec<usize> = (0..n)
            .map(|_| indices[(rng.random::<f64>() * n as f64) as usize])
            .collect();

        let x_b: Vec<f64> = boot_idx.iter().map(|&i| x[i]).collect();
        let m_b: Vec<f64> = boot_idx.iter().map(|&i| m[i]).collect();
        let y_b: Vec<f64> = boot_idx.iter().map(|&i| y[i]).collect();
        let cov_b: Vec<Vec<f64>> = covariates
            .iter()
            .map(|c| boot_idx.iter().map(|&i| c[i]).collect())
            .collect();
        let cov_slices: Vec<&[f64]> = cov_b.iter().map(|v| v.as_slice()).collect();

        let result = fit_and_decompose(&x_b, &m_b, &y_b, &cov_slices, interaction, opts);
        if let Ok((b_nde, b_nie, b_te, _, _, _, _)) = result {
            boot_nde.push(b_nde);
            boot_nie.push(b_nie);
            boot_te.push(b_te);
            if b_te.abs() > 1e-10 {
                boot_pm.push(b_nie / b_te);
            }
        }
    }

    // Percentile CIs.
    let (nde_lo, nde_hi) = percentile_ci(&boot_nde);
    let (nie_lo, nie_hi) = percentile_ci(&boot_nie);
    let (te_lo, te_hi) = percentile_ci(&boot_te);
    let (pm_lo, pm_hi) = percentile_ci(&boot_pm);

    let prop_mediated = if te.abs() > 1e-10 { nie / te } else { f64::NAN };

    Ok(MediationResult {
        nde,
        nde_ci_lower: nde_lo,
        nde_ci_upper: nde_hi,
        nie,
        nie_ci_lower: nie_lo,
        nie_ci_upper: nie_hi,
        te,
        te_ci_lower: te_lo,
        te_ci_upper: te_hi,
        prop_mediated,
        pm_ci_lower: pm_lo,
        pm_ci_upper: pm_hi,
        alpha_x,
        beta_x,
        beta_m,
        beta_xm,
        n_bootstrap: boot_nde.len(),
        n_obs: n,
    })
}

// ── Core fitting + decomposition ───────────────────────────────────────────

/// Fit mediator and outcome models, then compute NDE/NIE/TE.
///
/// Returns `(nde, nie, te, alpha_x, beta_x, beta_m, beta_xm)`.
fn fit_and_decompose(
    x: &[f64],
    m: &[f64],
    y: &[f64],
    covariates: &[&[f64]],
    interaction: bool,
    _opts: &MediationOptions,
) -> Result<(f64, f64, f64, f64, f64, f64, f64)> {
    // ── Mediator model: M ~ X + C ──────────────────────────────────────
    let mut m_preds: Vec<&[f64]> = vec![x];
    for c in covariates {
        m_preds.push(*c);
    }
    let m_fit = regression::ols(&m_preds, m, true).map_err(epi_from_stat)?;

    // α₀ = intercept, α₁ = coef on X (index 1).
    let alpha_0 = m_fit.coefficients[0];
    let alpha_1 = m_fit.coefficients[1]; // X coefficient in mediator model

    // ── Outcome model: Y ~ X + M [+ X:M] + C ──────────────────────────
    let xm_interact: Vec<f64> = if interaction {
        x.iter().zip(m).map(|(&xi, &mi)| xi * mi).collect()
    } else {
        vec![]
    };

    let mut y_preds: Vec<&[f64]> = vec![x, m];
    if interaction {
        y_preds.push(&xm_interact[..]);
    }
    for c in covariates {
        y_preds.push(*c);
    }
    let y_fit = regression::ols(&y_preds, y, true).map_err(epi_from_stat)?;

    // Extract outcome coefficients.
    // Index layout: 0=intercept, 1=X, 2=M, [3=X:M if interaction], then covariates.
    let beta_1 = y_fit.coefficients[1]; // β₁ (X direct effect)
    let beta_2 = y_fit.coefficients[2]; // β₂ (M effect)
    let beta_3 = if interaction {
        y_fit.coefficients[3]
    } else {
        0.0
    };

    // ── Decomposition (VanderWeele 2015) ───────────────────────────────
    // For binary X (control=0, treated=1), evaluated at covariate means C̄:
    //
    // E[M | X=0, C̄] = α₀ + α₁·x_control + Σ α_c·C̄   (x_control = 0 here)
    //
    // NDE = β₁ + β₃ · M(0)
    //     = β₁ + β₃ · (α₀ + Σ α_c·C̄)
    //
    // NIE = (β₂ + β₃) · α₁
    //
    // TE = NDE + NIE

    // M(0): mediator-model prediction at X = 0 with covariates held at their
    // sample means (m-coefficient layout: 0=intercept, 1=X, 2+=covariates).
    let n = x.len();
    let m_under_control = alpha_0
        + covariates
            .iter()
            .enumerate()
            .map(|(i, cov)| {
                let mean = cov.iter().sum::<f64>() / n as f64;
                m_fit.coefficients[2 + i] * mean
            })
            .sum::<f64>();
    let nde = beta_1 + beta_3 * m_under_control;
    let nie = (beta_2 + beta_3) * alpha_1;
    let te = nde + nie;

    Ok((nde, nie, te, alpha_1, beta_1, beta_2, beta_3))
}

// ── Helpers ────────────────────────────────────────────────────────────────

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

    #[test]
    fn full_mediation_no_interaction() {
        // X → M → Y with known coefficients.
        // α₁ = 0.5 (X→M), β₁ = 0.0 (no direct effect), β₂ = 1.0 (M→Y).
        // Expected: NDE = 0, NIE = 0.5, TE = 0.5.
        let n = 200;
        let x: Vec<f64> = (0..n).map(|i| (i % 2) as f64).collect();
        let m: Vec<f64> = x
            .iter()
            .enumerate()
            .map(|(i, &xi)| 1.0 + 0.5 * xi + 0.1 * (i as f64 % 7.0 - 3.0))
            .collect();
        let y: Vec<f64> = m.iter().map(|&mi| 2.0 + 1.0 * mi).collect();

        let opts = MediationOptions {
            n_bootstrap: 100,
            ..Default::default()
        };
        let result = mediation(&x, &m, &y, &[], false, &opts).unwrap();

        assert!(
            approx_eq(result.nde, 0.0, 0.1),
            "NDE should be ~0, got {}",
            result.nde
        );
        assert!(
            approx_eq(result.nie, 0.5, 0.1),
            "NIE should be ~0.5, got {}",
            result.nie
        );
        assert!(
            approx_eq(result.te, 0.5, 0.1),
            "TE should be ~0.5, got {}",
            result.te
        );
        assert!(
            approx_eq(result.prop_mediated, 1.0, 0.2),
            "Prop mediated should be ~1.0, got {}",
            result.prop_mediated
        );
    }

    #[test]
    fn partial_mediation() {
        // X affects Y both directly and through M.
        // α₁ = 0.5, β₁ = 0.3 (direct), β₂ = 0.8.
        // Expected: NDE = 0.3, NIE = 0.4, TE = 0.7.
        let n = 200;
        let x: Vec<f64> = (0..n).map(|i| (i % 2) as f64).collect();
        let m: Vec<f64> = x
            .iter()
            .enumerate()
            .map(|(i, &xi)| 0.5 * xi + 0.05 * (i as f64 % 5.0))
            .collect();
        let y: Vec<f64> = (0..n)
            .map(|i| 0.3 * x[i] + 0.8 * m[i] + 0.01 * (i as f64 % 3.0))
            .collect();

        let opts = MediationOptions {
            n_bootstrap: 100,
            ..Default::default()
        };
        let result = mediation(&x, &m, &y, &[], false, &opts).unwrap();

        assert!(
            approx_eq(result.nde, 0.3, 0.05),
            "NDE ~0.3, got {}",
            result.nde
        );
        assert!(
            approx_eq(result.nie, 0.4, 0.05),
            "NIE ~0.4, got {}",
            result.nie
        );
        assert!(
            approx_eq(result.te, 0.7, 0.05),
            "TE ~0.7, got {}",
            result.te
        );
    }

    #[test]
    fn no_indirect_effect() {
        // X affects Y only directly, M is independent of X.
        // α₁ ≈ 0, β₁ = 1.0, β₂ = 0.5.
        // Expected: NIE ≈ 0, NDE ≈ 1.0.
        let n = 200;
        let x: Vec<f64> = (0..n).map(|i| (i % 2) as f64).collect();
        let m: Vec<f64> = (0..n).map(|i| 0.1 * (i as f64 % 10.0)).collect();
        let y: Vec<f64> = x
            .iter()
            .zip(&m)
            .map(|(&xi, &mi)| 1.0 * xi + 0.5 * mi)
            .collect();

        let opts = MediationOptions {
            n_bootstrap: 100,
            ..Default::default()
        };
        let result = mediation(&x, &m, &y, &[], false, &opts).unwrap();

        assert!(
            result.nie.abs() < 0.1,
            "NIE should be ~0, got {}",
            result.nie
        );
        assert!(
            approx_eq(result.nde, 1.0, 0.1),
            "NDE ~1.0, got {}",
            result.nde
        );
    }

    #[test]
    fn bootstrap_cis_bracket_point_estimates() {
        let n = 200;
        let x: Vec<f64> = (0..n).map(|i| (i % 2) as f64).collect();
        let m: Vec<f64> = x
            .iter()
            .enumerate()
            .map(|(i, &xi)| 0.5 * xi + 0.05 * (i as f64 % 5.0))
            .collect();
        let y: Vec<f64> = (0..n).map(|i| 0.3 * x[i] + 0.8 * m[i]).collect();

        let opts = MediationOptions {
            n_bootstrap: 200,
            ..Default::default()
        };
        let result = mediation(&x, &m, &y, &[], false, &opts).unwrap();

        // Point estimates should be within bootstrap CIs (most of the time).
        assert!(result.nde >= result.nde_ci_lower * 0.9 && result.nde <= result.nde_ci_upper * 1.1);
        assert!(result.nie >= result.nie_ci_lower * 0.9 && result.nie <= result.nie_ci_upper * 1.1);
    }

    #[test]
    fn interaction_nde_uses_covariate_means() {
        // Regression: with interaction and a non-zero-mean covariate,
        // M(0) = α₀ + α_c·C̄ (mediator prediction at X=0, covariates at their
        // sample means). The old code used α₀ alone, biasing NDE by
        // β₃·α_c·C̄ (74% relative error on NHANES-like data).
        let n = 240;
        let x: Vec<f64> = (0..n).map(|i| (i % 2) as f64).collect();
        // C ∈ {4.0, 4.7, 5.4, 6.1, 6.8} equally often → mean exactly 5.4.
        let c: Vec<f64> = (0..n).map(|i| 4.0 + ((i % 5) as f64) * 0.7).collect();
        // Deterministic pseudo-noise so the mediator design is full-rank.
        let eps: Vec<f64> = (0..n)
            .map(|i| (((i * 7919 + 13) % 97) as f64 / 97.0 - 0.5) * 0.4)
            .collect();
        let (a0, a1, ac) = (1.0, 0.5, 0.8);
        let m: Vec<f64> = (0..n)
            .map(|i| a0 + a1 * x[i] + ac * c[i] + eps[i])
            .collect();
        let (b1, b2, b3) = (0.3, 1.0, 0.4);
        // Outcome is exact in [1, X, M, X:M] → β̂ recovered exactly.
        let y: Vec<f64> = (0..n)
            .map(|i| 0.2 + b1 * x[i] + b2 * m[i] + b3 * x[i] * m[i] + 0.6 * c[i])
            .collect();

        let (nde, nie, te, _a1, _b1, _b2, _b3) =
            fit_and_decompose(&x, &m, &y, &[&c], true, &MediationOptions::default()).unwrap();

        let c_mean = 5.4;
        let m0 = a0 + ac * c_mean; // 5.32
        let expected_nde = b1 + b3 * m0; // 2.428
        let expected_nie = (b2 + b3) * a1; // 0.70
        assert!(
            (nde - expected_nde).abs() < 0.1,
            "NDE: got {nde}, expected {expected_nde} (pre-fix code gave {})",
            b1 + b3 * a0
        );
        assert!(
            (nie - expected_nie).abs() < 0.1,
            "NIE: got {nie}, expected {expected_nie}"
        );
        assert!(
            (te - (expected_nde + expected_nie)).abs() < 0.2,
            "TE: got {te}, expected {}",
            expected_nde + expected_nie
        );
    }
}
