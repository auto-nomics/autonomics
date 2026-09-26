//! Survey-weighted causal mediation with bias-corrected bootstrap CIs.
//!
//! The weighted counterpart of [`crate::mediation`], matching how
//! NHANES-style papers report mediation (Li et al. 2026, Figure 4): the
//! two-model VanderWeele decomposition fitted by WLS under sampling
//! weights, with uncertainty from the design-aware bootstrap of
//! [`crate::bootstrap`]. For a linear mediator and outcome the WLS
//! coefficients are the survey-population estimand — linear WLS is
//! invariant to weight scaling, so only the *design* (which rows cluster
//! together) matters for the intervals.
//!
//! # Models (WLS, same weights throughout)
//!
//! **Mediator model**: `M = α₀ + α₁·X + Σⱼ α_cⱼ·Cⱼ + ε_M`
//!
//! **Outcome model**: `Y = β₀ + β₁·X + β₂·M [+ β₃·(X·M)] + Σⱼ β_cⱼ·Cⱼ + ε_Y`
//!
//! # Decomposition (Δx = x_treated − x_control)
//!
//! Unlike the unweighted [`crate::mediation`] (which evaluates the mediator
//! model at bare α₀), the controlled mediator level is standardised to the
//! **weighted covariate means** — the marginal mediator level of the survey
//! population at `X = x_control`:
//!
//! ```text
//! m_under_control = α₀ + α₁·x_control + Σⱼ α_cⱼ·mean_w(Cⱼ)
//! CDE = NDE = (β₁ + β₃·m_under_control) · Δx      (identical here)
//! NIE = (β₂ + β₃·x_treated) · α₁ · Δx
//! TE  = NDE + NIE,   prop_mediated = NIE / TE
//! ```
//!
//! CDE and NDE coincide because both evaluate the mediator model at the
//! same controlled level; both are reported for output-schema parity with
//! [`crate::cmest`].

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

use statkit::regression;

use crate::bootstrap::{self, BootstrapDesign, CiMethod};
use crate::error::{EpiError, Result};

// ── Options ────────────────────────────────────────────────────────────────

/// Options for [`mediation_weighted`].
#[derive(Debug, Clone)]
pub struct MediationWeightedOptions {
    /// Include an `X × M` interaction in the outcome model (default false).
    pub interaction: bool,
    /// Bootstrap CI construction (default bias-corrected).
    pub ci_method: CiMethod,
    /// Bootstrap iterations (default 1000).
    pub n_bootstrap: usize,
    /// Random seed.
    pub seed: u64,
    /// Exposure level for "treated" (default 1.0).
    pub x_treated: f64,
    /// Exposure level for "control" (default 0.0).
    pub x_control: f64,
}

impl Default for MediationWeightedOptions {
    fn default() -> Self {
        Self {
            interaction: false,
            ci_method: CiMethod::BiasCorrected,
            n_bootstrap: 1000,
            seed: 42,
            x_treated: 1.0,
            x_control: 0.0,
        }
    }
}

// ── Result ─────────────────────────────────────────────────────────────────

/// Result of a survey-weighted causal mediation analysis.
///
/// Intervals are `(lower, upper)` tuples; `n_bootstrap` reports the number
/// of *successful* replicates (failed refits are skipped and counted out).
#[derive(Debug, Clone)]
pub struct MediationWeightedResult {
    /// Controlled direct effect (point estimate).
    pub cde: f64,
    /// CDE bootstrap 95% CI.
    pub cde_ci: (f64, f64),
    /// Natural direct effect — identical to CDE in this parameterization.
    pub nde: f64,
    /// NDE bootstrap 95% CI.
    pub nde_ci: (f64, f64),
    /// Natural indirect effect (point estimate).
    pub nie: f64,
    /// NIE bootstrap 95% CI.
    pub nie_ci: (f64, f64),
    /// Total effect = NDE + NIE.
    pub te: f64,
    /// TE bootstrap 95% CI.
    pub te_ci: (f64, f64),
    /// Proportion mediated = NIE / TE (NaN if TE ≈ 0).
    pub prop_mediated: f64,
    /// Proportion mediated bootstrap 95% CI.
    pub pm_ci: (f64, f64),
    /// Mediator model coefficient α₁ (effect of X on M).
    pub alpha_x: f64,
    /// Outcome model coefficient β₁ (direct effect of X on Y).
    pub beta_x: f64,
    /// Outcome model coefficient β₂ (effect of M on Y).
    pub beta_m: f64,
    /// Outcome model interaction β₃ (X×M), zero when `interaction = false`.
    pub beta_xm: f64,
    /// Mediator level under control at weighted covariate means.
    pub m_under_control: f64,
    /// Successful bootstrap replicates.
    pub n_bootstrap: usize,
    /// Number of observations.
    pub n_obs: usize,
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Perform a survey-weighted causal mediation analysis.
///
/// `x` is the exposure, `m` the mediator, `y` the outcome, `covariates`
/// optional confounders adjusted for in both models, and `design` carries
/// the sampling weights (plus optional strata/PSU for the bootstrap).
pub fn mediation_weighted(
    x: &[f64],
    m: &[f64],
    y: &[f64],
    covariates: &[&[f64]],
    design: &BootstrapDesign,
    opts: &MediationWeightedOptions,
) -> Result<MediationWeightedResult> {
    run_mediation(x, m, y, covariates, design, opts, fit_and_decompose)
}

/// Survey-weighted causal mediation with a **binary outcome**.
///
/// The mediator model stays WLS (`M ~ X + C`), but the outcome model is a
/// weighted logistic regression (`Y ~ X + M [+ X:M] + C`, IRLS — the same
/// estimating equations as `svyglm(family = quasibinomial)`), so the
/// decomposition of [`MediationWeightedResult`] is on the **log-odds
/// scale**:
///
/// ```text
/// CDE = (β₁ + β₃·m_under_control) · Δx      (log-odds ratio)
/// NIE = (β₂ + β₃·x_treated) · α₁ · Δx       (log-odds ratio)
/// ```
///
/// The VanderWeele algebra carries over verbatim because it is linear in
/// the coefficients; `exp(effect)` gives the odds-ratio scale. Intervals
/// come from the same design-aware bootstrap as [`mediation_weighted`].
pub fn mediation_weighted_binary(
    x: &[f64],
    m: &[f64],
    y: &[f64],
    covariates: &[&[f64]],
    design: &BootstrapDesign,
    opts: &MediationWeightedOptions,
) -> Result<MediationWeightedResult> {
    for (i, &yi) in y.iter().enumerate() {
        if yi != 0.0 && yi != 1.0 {
            return Err(EpiError::Numerical(format!(
                "binary mediation requires y ∈ {{0,1}}, got {yi} at row {i}"
            )));
        }
    }
    run_mediation(x, m, y, covariates, design, opts, fit_and_decompose_binary)
}

/// Shared driver: point estimates + design-aware bootstrap + CIs.
///
/// `fit_and_decompose` is the family-specific two-model fit (linear vs
/// logistic outcome); everything else (weights normalisation, resampling,
/// CI construction) is family-independent.
fn run_mediation(
    x: &[f64],
    m: &[f64],
    y: &[f64],
    covariates: &[&[f64]],
    design: &BootstrapDesign,
    opts: &MediationWeightedOptions,
    fit_and_decompose: fn(
        &[f64],
        &[f64],
        &[f64],
        &[&[f64]],
        &[f64],
        &MediationWeightedOptions,
    ) -> Result<(f64, f64, f64, f64, f64, f64, f64, f64)>,
) -> Result<MediationWeightedResult> {
    let n = design.n();
    if n == 0 || m.len() != n || y.len() != n || x.len() != n {
        return Err(EpiError::DimensionMismatch {
            a: n,
            b: x.len().max(m.len()).max(y.len()),
        });
    }
    for c in covariates.iter() {
        if c.len() != n {
            return Err(EpiError::DimensionMismatch { a: n, b: c.len() });
        }
    }

    // Weights normalised to mean 1 — numerically friendly for the normal
    // equations; coefficients are invariant to the scaling.
    let w = design.normalized_weights();

    // ── Point estimates from the full data ─────────────────────────────
    let (cde, nie, te, alpha_x, beta_x, beta_m, beta_xm, m_uc) =
        fit_and_decompose(x, m, y, covariates, &w, opts)?;

    // ── Design-aware bootstrap ─────────────────────────────────────────
    let mut rng = ChaCha8Rng::seed_from_u64(opts.seed);

    let mut boot_cde = Vec::with_capacity(opts.n_bootstrap);
    let mut boot_nie = Vec::with_capacity(opts.n_bootstrap);
    let mut boot_te = Vec::with_capacity(opts.n_bootstrap);
    let mut boot_pm = Vec::with_capacity(opts.n_bootstrap);

    for _ in 0..opts.n_bootstrap {
        let idx = design.replicate_indices(&mut rng);
        let x_b: Vec<f64> = idx.iter().map(|&i| x[i]).collect();
        let m_b: Vec<f64> = idx.iter().map(|&i| m[i]).collect();
        let y_b: Vec<f64> = idx.iter().map(|&i| y[i]).collect();
        let w_b: Vec<f64> = idx.iter().map(|&i| w[i]).collect();
        let cov_b: Vec<Vec<f64>> = covariates
            .iter()
            .map(|c| idx.iter().map(|&i| c[i]).collect())
            .collect();
        let cov_slices: Vec<&[f64]> = cov_b.iter().map(|v| v.as_slice()).collect();

        if let Ok((b_cde, b_nie, b_te, _, _, _, _, _)) =
            fit_and_decompose(&x_b, &m_b, &y_b, &cov_slices, &w_b, opts)
        {
            boot_cde.push(b_cde);
            boot_nie.push(b_nie);
            boot_te.push(b_te);
            if b_te.abs() > 1e-10 {
                boot_pm.push(b_nie / b_te);
            }
        }
    }

    let method = opts.ci_method;
    let cde_ci = bootstrap::ci(method, &boot_cde, cde);
    let nie_ci = bootstrap::ci(method, &boot_nie, nie);
    let te_ci = bootstrap::ci(method, &boot_te, te);
    let pm = if te.abs() > 1e-10 { nie / te } else { f64::NAN };
    let pm_ci = bootstrap::ci(method, &boot_pm, pm);

    Ok(MediationWeightedResult {
        cde,
        cde_ci,
        nde: cde,
        nde_ci: cde_ci,
        nie,
        nie_ci,
        te,
        te_ci,
        prop_mediated: pm,
        pm_ci,
        alpha_x,
        beta_x,
        beta_m,
        beta_xm,
        m_under_control: m_uc,
        n_bootstrap: boot_te.len(),
        n_obs: n,
    })
}

// ── Core fitting + decomposition ───────────────────────────────────────────

/// Fit both WLS models and decompose.
///
/// Returns `(cde, nie, te, alpha_x, beta_x, beta_m, beta_xm, m_under_control)`.
fn fit_and_decompose(
    x: &[f64],
    m: &[f64],
    y: &[f64],
    covariates: &[&[f64]],
    w: &[f64],
    opts: &MediationWeightedOptions,
) -> Result<(f64, f64, f64, f64, f64, f64, f64, f64)> {
    let dx = opts.x_treated - opts.x_control;

    // ── Mediator model: M ~ X + C (WLS) ────────────────────────────────
    let mut m_preds: Vec<&[f64]> = vec![x];
    for c in covariates {
        m_preds.push(*c);
    }
    let m_fit = regression::wls(&m_preds, m, w, true).map_err(epi_from_stat)?;

    let alpha_0 = m_fit.coefficients[0];
    let alpha_1 = m_fit.coefficients[1];

    // Weighted covariate means (survey-population standardisation).
    let wsum: f64 = w.iter().sum();
    let m_uc = alpha_0
        + alpha_1 * opts.x_control
        + covariates
            .iter()
            .zip(&m_fit.coefficients[2..])
            .map(|(c, &coef)| {
                let mean = c.iter().zip(w).map(|(&ci, &wi)| wi * ci).sum::<f64>() / wsum;
                coef * mean
            })
            .sum::<f64>();

    // ── Outcome model: Y ~ X + M [+ X:M] + C (WLS) ─────────────────────
    let xm_interact: Vec<f64> = if opts.interaction {
        x.iter().zip(m).map(|(&xi, &mi)| xi * mi).collect()
    } else {
        vec![]
    };
    let mut y_preds: Vec<&[f64]> = vec![x, m];
    if opts.interaction {
        y_preds.push(&xm_interact[..]);
    }
    for c in covariates {
        y_preds.push(*c);
    }
    let y_fit = regression::wls(&y_preds, y, w, true).map_err(epi_from_stat)?;

    // Index layout: 0=intercept, 1=X, 2=M, [3=X:M if interaction], then C.
    let beta_1 = y_fit.coefficients[1];
    let beta_2 = y_fit.coefficients[2];
    let beta_3 = if opts.interaction {
        y_fit.coefficients[3]
    } else {
        0.0
    };

    let cde = (beta_1 + beta_3 * m_uc) * dx;
    let nie = (beta_2 + beta_3 * opts.x_treated) * alpha_1 * dx;
    let te = cde + nie;

    Ok((cde, nie, te, alpha_1, beta_1, beta_2, beta_3, m_uc))
}

fn epi_from_stat(e: statkit::StatError) -> EpiError {
    EpiError::Numerical(e.to_string())
}

/// Binary-outcome variant: WLS mediator + weighted logistic outcome.
///
/// Returns the same tuple as [`fit_and_decompose`]; the effects are on the
/// log-odds scale (the decomposition is linear in the coefficients, so the
/// algebra is unchanged from the linear-outcome case).
fn fit_and_decompose_binary(
    x: &[f64],
    m: &[f64],
    y: &[f64],
    covariates: &[&[f64]],
    w: &[f64],
    opts: &MediationWeightedOptions,
) -> Result<(f64, f64, f64, f64, f64, f64, f64, f64)> {
    let dx = opts.x_treated - opts.x_control;

    // ── Mediator model: M ~ X + C (WLS, unchanged) ─────────────────────
    let mut m_preds: Vec<&[f64]> = vec![x];
    for c in covariates {
        m_preds.push(*c);
    }
    let m_fit = regression::wls(&m_preds, m, w, true).map_err(epi_from_stat)?;

    let alpha_0 = m_fit.coefficients[0];
    let alpha_1 = m_fit.coefficients[1];

    let wsum: f64 = w.iter().sum();
    let m_uc = alpha_0
        + alpha_1 * opts.x_control
        + covariates
            .iter()
            .zip(&m_fit.coefficients[2..])
            .map(|(c, &coef)| {
                let mean = c.iter().zip(w).map(|(&ci, &wi)| wi * ci).sum::<f64>() / wsum;
                coef * mean
            })
            .sum::<f64>();

    // ── Outcome model: Y ~ X + M [+ X:M] + C (weighted logistic) ───────
    let xm_interact: Vec<f64> = if opts.interaction {
        x.iter().zip(m).map(|(&xi, &mi)| xi * mi).collect()
    } else {
        vec![]
    };
    let mut y_preds: Vec<&[f64]> = vec![x, m];
    if opts.interaction {
        y_preds.push(&xm_interact[..]);
    }
    for c in covariates {
        y_preds.push(*c);
    }
    let y_fit = regression::logistic_weighted(&y_preds, y, Some(w), true)
        .map_err(|e| EpiError::Numerical(format!("logistic outcome model: {e}")))?;

    // Same index layout as the linear variant.
    let beta_1 = y_fit.coefficients[1];
    let beta_2 = y_fit.coefficients[2];
    let beta_3 = if opts.interaction {
        y_fit.coefficients[3]
    } else {
        0.0
    };

    let cde = (beta_1 + beta_3 * m_uc) * dx;
    let nie = (beta_2 + beta_3 * opts.x_treated) * alpha_1 * dx;
    let te = cde + nie;

    Ok((cde, nie, te, alpha_1, beta_1, beta_2, beta_3, m_uc))
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * b.abs().max(1.0)
    }

    /// Weighted linear system with known coefficients:
    /// α = (2, 0.8, 0.05, 0.3), β = (5, 0.4, 0.9, 0.01, 0.5).
    /// Expected: NIE ≈ 0.9·0.8 = 0.72, NDE ≈ 0.4, TE ≈ 1.12 (the mediator
    /// carries pseudo-noise so the outcome model is not collinear).
    fn system() -> (Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>) {
        let n = 240;
        let x: Vec<f64> = (0..n).map(|i| ((i % 5) / 3) as f64).collect();
        let c1: Vec<f64> = (0..n).map(|i| 55.0 + (i as f64 % 17.0)).collect();
        let c2: Vec<f64> = (0..n).map(|i| ((i % 4) / 2) as f64).collect();
        let w: Vec<f64> = (0..n).map(|i| 5000.0 + 250.0 * (i as f64 % 11.0)).collect();
        let m: Vec<f64> = (0..n)
            .map(|i| {
                2.0 + 0.8 * x[i]
                    + 0.05 * c1[i]
                    + 0.3 * c2[i]
                    + 1.2 * ((i as f64 * 7.0) % 13.0 - 6.0) / 6.0
            })
            .collect();
        let y: Vec<f64> = (0..n)
            .map(|i| 5.0 + 0.4 * x[i] + 0.9 * m[i] + 0.01 * c1[i] + 0.5 * c2[i])
            .collect();
        (x, m, y, c1, c2, w)
    }

    #[test]
    fn recovers_known_decomposition() {
        let (x, m, y, c1, c2, w) = system();
        let design = BootstrapDesign::new(&w).unwrap();
        let opts = MediationWeightedOptions {
            n_bootstrap: 0,
            ..Default::default()
        };
        let r = mediation_weighted(&x, &m, &y, &[&c1, &c2], &design, &opts).unwrap();
        assert!(approx_eq(r.nie, 0.72, 0.05), "nie {}", r.nie);
        assert!(approx_eq(r.nde, 0.4, 0.05), "nde {}", r.nde);
        assert!(approx_eq(r.cde, 0.4, 0.05), "cde {}", r.cde);
        assert!(approx_eq(r.te, 1.12, 0.05), "te {}", r.te);
        assert!(approx_eq(r.prop_mediated, 0.72 / 1.12, 0.15));
        assert_eq!(r.n_obs, 240);
    }

    #[test]
    fn weight_scaling_leaves_point_estimates_unchanged() {
        let (x, m, y, c1, c2, w) = system();
        let w100: Vec<f64> = w.iter().map(|&v| 100.0 * v).collect();
        let design_a = BootstrapDesign::new(&w).unwrap();
        let design_b = BootstrapDesign::new(&w100).unwrap();
        let opts = MediationWeightedOptions {
            n_bootstrap: 0,
            ..Default::default()
        };
        let a = mediation_weighted(&x, &m, &y, &[&c1, &c2], &design_a, &opts).unwrap();
        let b = mediation_weighted(&x, &m, &y, &[&c1, &c2], &design_b, &opts).unwrap();
        assert!(approx_eq(a.nie, b.nie, 1e-9));
        assert!(approx_eq(a.te, b.te, 1e-9));
        assert!(approx_eq(a.m_under_control, b.m_under_control, 1e-9));
    }

    #[test]
    fn effects_scale_with_exposure_contrast() {
        let (x, m, y, c1, c2, w) = system();
        let design = BootstrapDesign::new(&w).unwrap();
        let one = MediationWeightedOptions {
            n_bootstrap: 0,
            ..Default::default()
        };
        let two = MediationWeightedOptions {
            x_treated: 2.0,
            n_bootstrap: 0,
            ..Default::default()
        };
        let a = mediation_weighted(&x, &m, &y, &[&c1, &c2], &design, &one).unwrap();
        let b = mediation_weighted(&x, &m, &y, &[&c1, &c2], &design, &two).unwrap();
        assert!(approx_eq(b.te, 2.0 * a.te, 1e-9));
        assert!(approx_eq(b.nie, 2.0 * a.nie, 1e-9));
    }

    #[test]
    fn bootstrap_bc_ci_brackets_point_estimates() {
        let (x, m, y, c1, c2, w) = system();
        // Noisy version so bootstrap spread is non-degenerate.
        let mut y = y;
        for (i, yi) in y.iter_mut().enumerate() {
            *yi += 0.7 * ((i as f64 * 7.0) % 13.0 - 6.0) / 6.0;
        }
        let design = BootstrapDesign::new(&w).unwrap();
        let opts = MediationWeightedOptions {
            n_bootstrap: 200,
            ..Default::default()
        };
        let r = mediation_weighted(&x, &m, &y, &[&c1, &c2], &design, &opts).unwrap();
        assert!(r.n_bootstrap >= 190, "too many failed replicates");
        let bracketed = |v: f64, (lo, hi): (f64, f64)| v >= lo - 1e-9 && v <= hi + 1e-9;
        assert!(bracketed(r.nie, r.nie_ci));
        assert!(bracketed(r.te, r.te_ci));
        assert!(r.nie_ci.0 < r.nie_ci.1);
    }

    #[test]
    fn dimension_mismatches_are_errors() {
        let (x, m, y, _c1, c2, w) = system();
        let design = BootstrapDesign::new(&w).unwrap();
        let opts = MediationWeightedOptions {
            n_bootstrap: 0,
            ..Default::default()
        };
        let short: Vec<f64> = c2[..c2.len() - 1].to_vec();
        assert!(matches!(
            mediation_weighted(&x, &m, &y, &[&short], &design, &opts),
            Err(EpiError::DimensionMismatch { .. })
        ));
        let xs: Vec<f64> = x[..x.len() - 1].to_vec();
        assert!(matches!(
            mediation_weighted(&xs, &m, &y, &[], &design, &opts),
            Err(EpiError::DimensionMismatch { .. })
        ));
    }

    // ── Binary outcome ────────────────────────────────────────────────────

    /// Logistic-generating weighted system (all covariates centred so the
    /// outcome stays away from separation):
    /// M  = 0.5·X + 0.3·C1 + 0.3·C2 + 0.3·noise
    /// logit(P(Y=1)) = −1.2 + 0.6·X + 0.5·M + 0.3·C1 + 0.4·C2
    /// Expected (log-odds scale): NIE ≈ 0.5·0.5 = 0.25, CDE ≈ 0.6,
    /// TE ≈ 0.85 — recovered with sampling noise.
    fn binary_system() -> (Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>) {
        let n = 600;
        let x: Vec<f64> = (0..n).map(|i| (i % 5 % 2) as f64).collect();
        let c1: Vec<f64> = (0..n).map(|i| (i % 23) as f64 / 23.0 - 0.5).collect();
        let c2: Vec<f64> = (0..n).map(|i| ((i % 4) / 2) as f64).collect();
        let w: Vec<f64> = (0..n).map(|i| 4000.0 + 300.0 * (i as f64 % 9.0)).collect();
        let m: Vec<f64> = (0..n)
            .map(|i| {
                0.5 * x[i] + 0.3 * c1[i] + 0.3 * c2[i] + 0.3 * ((i as f64 * 7.0) % 13.0 - 6.0) / 6.0
            })
            .collect();
        let y: Vec<f64> = (0..n)
            .map(|i| {
                let eta = -1.2 + 0.6 * x[i] + 0.5 * m[i] + 0.3 * c1[i] + 0.4 * c2[i];
                // pseudo-Bernoulli draw from a golden-ratio low-discrepancy
                // sequence (independent of every generator above).
                let u = (i as f64 * 0.6180339887498949).fract();
                let p = 1.0 / (1.0 + (-eta).exp());
                if u < p { 1.0 } else { 0.0 }
            })
            .collect();
        (x, m, y, c1, c2, w)
    }

    #[test]
    fn binary_recovers_known_decomposition() {
        let (x, m, y, c1, c2, w) = binary_system();
        let design = BootstrapDesign::new(&w).unwrap();
        let opts = MediationWeightedOptions {
            n_bootstrap: 0,
            ..Default::default()
        };
        let r = mediation_weighted_binary(&x, &m, &y, &[&c1, &c2], &design, &opts).unwrap();
        // Smoke bands (the periodic pseudo-random system has internal
        // correlations that bias the point estimates upward); exact R parity
        // is pinned by tests/xval_weighted_mediation_binary.rs.
        assert!((0.1..0.6).contains(&r.nie), "nie {}", r.nie);
        assert!((0.3..0.9).contains(&r.cde), "cde {}", r.cde);
        assert!((r.te - r.cde - r.nie).abs() < 1e-9);
        assert!(
            (0.0..1.0).contains(&r.prop_mediated),
            "pm {}",
            r.prop_mediated
        );
        assert_eq!(r.n_obs, 600);
        // Odds-ratio scale: exp of the log-odds effects.
        assert!(
            r.nie.exp() > 1.0 && r.nie.exp() < 2.0,
            "or-nie {}",
            r.nie.exp()
        );
    }

    #[test]
    fn binary_rejects_non_01_outcome() {
        let (x, m, mut y, c1, _c2, w) = binary_system();
        y[3] = 2.0;
        let design = BootstrapDesign::new(&w).unwrap();
        let opts = MediationWeightedOptions {
            n_bootstrap: 0,
            ..Default::default()
        };
        let err = mediation_weighted_binary(&x, &m, &y, &[&c1], &design, &opts);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("y ∈ {0,1}"));
    }

    #[test]
    fn binary_point_estimates_invariant_to_weight_scaling() {
        let (x, m, y, c1, c2, w) = binary_system();
        let w100: Vec<f64> = w.iter().map(|&v| 100.0 * v).collect();
        let design_a = BootstrapDesign::new(&w).unwrap();
        let design_b = BootstrapDesign::new(&w100).unwrap();
        let opts = MediationWeightedOptions {
            n_bootstrap: 0,
            ..Default::default()
        };
        let a = mediation_weighted_binary(&x, &m, &y, &[&c1, &c2], &design_a, &opts).unwrap();
        let b = mediation_weighted_binary(&x, &m, &y, &[&c1, &c2], &design_b, &opts).unwrap();
        assert!(approx_eq(a.nie, b.nie, 1e-8), "{} vs {}", a.nie, b.nie);
        assert!(approx_eq(a.cde, b.cde, 1e-8), "{} vs {}", a.cde, b.cde);
    }

    #[test]
    fn binary_bootstrap_ci_brackets_point_estimates() {
        let (x, m, y, c1, c2, w) = binary_system();
        // 6 strata × 5 PSUs per stratum (PSU codes nested in strata).
        let strata: Vec<u64> = (0..600).map(|i| (i / 100) as u64).collect();
        let psu: Vec<u64> = (0..600)
            .map(|i| ((i / 4) % 2 + 2 * (i / 100)) as u64)
            .collect();
        let design = BootstrapDesign::with_clusters(&w, Some(&strata), Some(&psu)).unwrap();
        let opts = MediationWeightedOptions {
            n_bootstrap: 100,
            ..Default::default()
        };
        let r = mediation_weighted_binary(&x, &m, &y, &[&c1, &c2], &design, &opts).unwrap();
        assert!(r.n_bootstrap >= 90, "too many failed replicates");
        let bracketed = |v: f64, (lo, hi): (f64, f64)| v >= lo - 1e-9 && v <= hi + 1e-9;
        assert!(bracketed(r.nie, r.nie_ci));
        assert!(bracketed(r.te, r.te_ci));
        assert!(r.nie_ci.0 < r.nie_ci.1);
    }
}
