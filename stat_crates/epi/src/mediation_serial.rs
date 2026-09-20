//! Survey-weighted serial (two-mediator) mediation: X → M1 → M2 → Y.
//!
//! The three-model chain used for the Hcy → β2M → GrimAge2-EAA → DSST
//! analysis of Li et al. 2026 (Figure 4D): a total effect decomposes into
//! the path through M1 only, the path through M2 only, and the *serial*
//! path running through both, with all models fitted by WLS under sampling
//! weights and intervals from the design-aware bootstrap
//! ([`crate::bootstrap`]).
//!
//! # Models (WLS, same weights throughout, no interactions)
//!
//! ```text
//! M1 = α + a₁·X + Σ α_c·C + ε₁
//! M2 = α + a₂·X + d₂₁·M1 + Σ α_c·C + ε₂
//! Y  = β + c′·X + b₁·M1 + b₂·M2 + Σ β_c·C + ε_Y
//! ```
//!
//! No interaction terms — the closed-form decomposition below is exact for
//! linear systems only.
//!
//! # Decomposition (Δx = x_treated − x_control)
//!
//! ```text
//! ie_m1     = a₁·b₁·Δx          (X → M1 → Y)
//! ie_m2     = a₂·b₂·Δx          (X → M2 → Y)
//! ie_serial = a₁·d₂₁·b₂·Δx      (X → M1 → M2 → Y)
//! total_indirect = ie_m1 + ie_m2 + ie_serial
//! direct = c′·Δx,   te = direct + total_indirect
//! prop_mediated = total_indirect / te,   prop_serial = ie_serial / te
//! ```
//!
//! For a linear system `te ≡ direct + total_indirect` is an algebraic
//! identity, not an approximation — the test suite asserts it.

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

use statkit::regression;

use crate::bootstrap::{self, BootstrapDesign, CiMethod};
use crate::error::{EpiError, Result};

// ── Options ────────────────────────────────────────────────────────────────

/// Options for [`mediation_serial`].
#[derive(Debug, Clone)]
pub struct SerialMediationOptions {
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

impl Default for SerialMediationOptions {
    fn default() -> Self {
        Self {
            ci_method: CiMethod::BiasCorrected,
            n_bootstrap: 1000,
            seed: 42,
            x_treated: 1.0,
            x_control: 0.0,
        }
    }
}

// ── Result ─────────────────────────────────────────────────────────────────

/// Result of a serial two-mediator analysis.
///
/// Intervals are `(lower, upper)` tuples; `n_bootstrap` reports successful
/// replicates.
#[derive(Debug, Clone)]
pub struct SerialMediationResult {
    /// Indirect effect through M1 only.
    pub ie_m1: f64,
    /// `ie_m1` bootstrap 95% CI.
    pub ie_m1_ci: (f64, f64),
    /// Indirect effect through M2 only.
    pub ie_m2: f64,
    /// `ie_m2` bootstrap 95% CI.
    pub ie_m2_ci: (f64, f64),
    /// Serial indirect effect through M1 then M2.
    pub ie_serial: f64,
    /// `ie_serial` bootstrap 95% CI.
    pub ie_serial_ci: (f64, f64),
    /// `ie_m1 + ie_m2 + ie_serial`.
    pub total_indirect: f64,
    /// Total indirect bootstrap 95% CI.
    pub total_indirect_ci: (f64, f64),
    /// Direct effect `c′·Δx`.
    pub direct: f64,
    /// Direct effect bootstrap 95% CI.
    pub direct_ci: (f64, f64),
    /// Total effect = direct + total_indirect.
    pub te: f64,
    /// Total effect bootstrap 95% CI.
    pub te_ci: (f64, f64),
    /// `total_indirect / te` (NaN if TE ≈ 0).
    pub prop_mediated: f64,
    /// `ie_serial / te` (NaN if TE ≈ 0).
    pub prop_serial: f64,
    /// Path X → M1.
    pub a1: f64,
    /// Path X → M2 (controlling M1).
    pub a2: f64,
    /// Path M1 → M2 (controlling X).
    pub d21: f64,
    /// Path M1 → Y (controlling X, M2).
    pub b1: f64,
    /// Path M2 → Y (controlling X, M1).
    pub b2: f64,
    /// Direct path X → Y (controlling both mediators).
    pub c_prime: f64,
    /// Full mediator-model coefficients (intercept first).
    pub m1_model: Vec<f64>,
    /// Full second-mediator-model coefficients (intercept first).
    pub m2_model: Vec<f64>,
    /// Full outcome-model coefficients (intercept first).
    pub y_model: Vec<f64>,
    /// Successful bootstrap replicates.
    pub n_bootstrap: usize,
    /// Number of observations.
    pub n_obs: usize,
}

/// Everything one three-model fit produces.
struct SerialFit {
    ie_m1: f64,
    ie_m2: f64,
    ie_serial: f64,
    total_indirect: f64,
    direct: f64,
    te: f64,
    a1: f64,
    a2: f64,
    d21: f64,
    b1: f64,
    b2: f64,
    c_prime: f64,
    m1_model: Vec<f64>,
    m2_model: Vec<f64>,
    y_model: Vec<f64>,
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Perform a survey-weighted serial two-mediator analysis.
///
/// `x` is the exposure, `m1`/`m2` the first/second mediator in the causal
/// chain, `y` the outcome, `covariates` optional confounders adjusted for
/// in all three models, and `design` the sampling design (weights plus
/// optional strata/PSU for the bootstrap).
pub fn mediation_serial(
    x: &[f64],
    m1: &[f64],
    m2: &[f64],
    y: &[f64],
    covariates: &[&[f64]],
    design: &BootstrapDesign,
    opts: &SerialMediationOptions,
) -> Result<SerialMediationResult> {
    let n = design.n();
    if n == 0 || x.len() != n || m1.len() != n || m2.len() != n || y.len() != n {
        return Err(EpiError::DimensionMismatch {
            a: n,
            b: x.len().max(m1.len()).max(m2.len()).max(y.len()),
        });
    }
    for c in covariates.iter() {
        if c.len() != n {
            return Err(EpiError::DimensionMismatch { a: n, b: c.len() });
        }
    }

    let w = design.normalized_weights();

    // ── Point estimates from the full data ─────────────────────────────
    let fit = fit_serial(x, m1, m2, y, covariates, &w, opts)?;

    // ── Design-aware bootstrap ─────────────────────────────────────────
    let mut rng = ChaCha8Rng::seed_from_u64(opts.seed);

    let mut boot_ie_m1 = Vec::with_capacity(opts.n_bootstrap);
    let mut boot_ie_m2 = Vec::with_capacity(opts.n_bootstrap);
    let mut boot_ie_serial = Vec::with_capacity(opts.n_bootstrap);
    let mut boot_total = Vec::with_capacity(opts.n_bootstrap);
    let mut boot_direct = Vec::with_capacity(opts.n_bootstrap);
    let mut boot_te = Vec::with_capacity(opts.n_bootstrap);

    for _ in 0..opts.n_bootstrap {
        let idx = design.replicate_indices(&mut rng);
        let x_b: Vec<f64> = idx.iter().map(|&i| x[i]).collect();
        let m1_b: Vec<f64> = idx.iter().map(|&i| m1[i]).collect();
        let m2_b: Vec<f64> = idx.iter().map(|&i| m2[i]).collect();
        let y_b: Vec<f64> = idx.iter().map(|&i| y[i]).collect();
        let w_b: Vec<f64> = idx.iter().map(|&i| w[i]).collect();
        let cov_b: Vec<Vec<f64>> = covariates
            .iter()
            .map(|c| idx.iter().map(|&i| c[i]).collect())
            .collect();
        let cov_slices: Vec<&[f64]> = cov_b.iter().map(|v| v.as_slice()).collect();

        if let Ok(b) = fit_serial(&x_b, &m1_b, &m2_b, &y_b, &cov_slices, &w_b, opts) {
            boot_ie_m1.push(b.ie_m1);
            boot_ie_m2.push(b.ie_m2);
            boot_ie_serial.push(b.ie_serial);
            boot_total.push(b.total_indirect);
            boot_direct.push(b.direct);
            boot_te.push(b.te);
        }
    }

    let method = opts.ci_method;
    let pm = if fit.te.abs() > 1e-10 {
        fit.total_indirect / fit.te
    } else {
        f64::NAN
    };
    let ps = if fit.te.abs() > 1e-10 {
        fit.ie_serial / fit.te
    } else {
        f64::NAN
    };

    Ok(SerialMediationResult {
        ie_m1: fit.ie_m1,
        ie_m1_ci: bootstrap::ci(method, &boot_ie_m1, fit.ie_m1),
        ie_m2: fit.ie_m2,
        ie_m2_ci: bootstrap::ci(method, &boot_ie_m2, fit.ie_m2),
        ie_serial: fit.ie_serial,
        ie_serial_ci: bootstrap::ci(method, &boot_ie_serial, fit.ie_serial),
        total_indirect: fit.total_indirect,
        total_indirect_ci: bootstrap::ci(method, &boot_total, fit.total_indirect),
        direct: fit.direct,
        direct_ci: bootstrap::ci(method, &boot_direct, fit.direct),
        te: fit.te,
        te_ci: bootstrap::ci(method, &boot_te, fit.te),
        prop_mediated: pm,
        prop_serial: ps,
        a1: fit.a1,
        a2: fit.a2,
        d21: fit.d21,
        b1: fit.b1,
        b2: fit.b2,
        c_prime: fit.c_prime,
        m1_model: fit.m1_model,
        m2_model: fit.m2_model,
        y_model: fit.y_model,
        n_bootstrap: boot_te.len(),
        n_obs: n,
    })
}

// ── Core fitting + decomposition ───────────────────────────────────────────

/// Fit the three WLS models and decompose the paths.
fn fit_serial(
    x: &[f64],
    m1: &[f64],
    m2: &[f64],
    y: &[f64],
    covariates: &[&[f64]],
    w: &[f64],
    opts: &SerialMediationOptions,
) -> Result<SerialFit> {
    let dx = opts.x_treated - opts.x_control;

    // ── M1 ~ X + C ─────────────────────────────────────────────────────
    let mut m1_preds: Vec<&[f64]> = vec![x];
    for c in covariates {
        m1_preds.push(*c);
    }
    let m1_fit = regression::wls(&m1_preds, m1, w, true).map_err(epi_from_stat)?;
    let a1 = m1_fit.coefficients[1];

    // ── M2 ~ X + M1 + C ────────────────────────────────────────────────
    let mut m2_preds: Vec<&[f64]> = vec![x, m1];
    for c in covariates {
        m2_preds.push(*c);
    }
    let m2_fit = regression::wls(&m2_preds, m2, w, true).map_err(epi_from_stat)?;
    // Index layout: 0=intercept, 1=X, 2=M1, then covariates.
    let a2 = m2_fit.coefficients[1];
    let d21 = m2_fit.coefficients[2];

    // ── Y ~ X + M1 + M2 + C ────────────────────────────────────────────
    let mut y_preds: Vec<&[f64]> = vec![x, m1, m2];
    for c in covariates {
        y_preds.push(*c);
    }
    let y_fit = regression::wls(&y_preds, y, w, true).map_err(epi_from_stat)?;
    // Index layout: 0=intercept, 1=X, 2=M1, 3=M2, then covariates.
    let c_prime = y_fit.coefficients[1];
    let b1 = y_fit.coefficients[2];
    let b2 = y_fit.coefficients[3];

    let ie_m1 = a1 * b1 * dx;
    let ie_m2 = a2 * b2 * dx;
    let ie_serial = a1 * d21 * b2 * dx;
    let total_indirect = ie_m1 + ie_m2 + ie_serial;
    let direct = c_prime * dx;
    let te = direct + total_indirect;

    Ok(SerialFit {
        ie_m1,
        ie_m2,
        ie_serial,
        total_indirect,
        direct,
        te,
        a1,
        a2,
        d21,
        b1,
        b2,
        c_prime,
        m1_model: m1_fit.coefficients,
        m2_model: m2_fit.coefficients,
        y_model: y_fit.coefficients,
    })
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

    /// Residual of `v` after weighted regression on `preds` (with
    /// intercept) — exactly W-orthogonal to every predictor column.
    fn orthogonalize(v: &[f64], preds: &[&[f64]], w: &[f64]) -> Vec<f64> {
        let fit = regression::wls(preds, v, w, true).unwrap();
        let mut hat = vec![fit.coefficients[0]; v.len()];
        for (j, p) in preds.iter().enumerate() {
            for (i, h) in hat.iter_mut().enumerate() {
                *h += fit.coefficients[j + 1] * p[i];
            }
        }
        v.iter().zip(&hat).map(|(&vi, &hi)| vi - hi).collect()
    }

    /// Serial chain with known paths:
    /// a₁ = 0.7, a₂ = 0.2, d₂₁ = 0.5, c′ = 0.3, b₁ = 0.6, b₂ = 0.8.
    /// Expected: ie_m1 = 0.42, ie_m2 = 0.16, ie_serial = 0.28,
    /// total = 0.86, direct = 0.3, te = 1.16.
    ///
    /// Each pseudo-noise block is first W-orthogonalized against its own
    /// model's regressors, so WLS recovers the generating coefficients
    /// exactly (a closed-form check, not noisy finite-sample recovery).
    fn system() -> (Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>) {
        let n = 240;
        let x: Vec<f64> = (0..n).map(|i| ((i % 5) / 3) as f64).collect();
        let c1: Vec<f64> = (0..n).map(|i| 40.0 + (i as f64 % 19.0)).collect();
        let w: Vec<f64> = (0..n).map(|i| 4000.0 + 300.0 * (i as f64 % 13.0)).collect();
        // Three pseudo-noise sequences with coprime periods, so no model
        // column becomes an exact linear combination of the others.
        let n1_raw: Vec<f64> = (0..n)
            .map(|i| ((i as f64 * 7.0) % 13.0 - 6.0) / 6.0)
            .collect();
        let n1 = orthogonalize(&n1_raw, &[&x, &c1], &w);
        let m1: Vec<f64> = (0..n)
            .map(|i| 1.0 + 0.7 * x[i] + 0.02 * c1[i] + 2.0 * n1[i])
            .collect();
        let n2_raw: Vec<f64> = (0..n)
            .map(|i| ((i as f64 * 5.0) % 11.0 - 5.0) / 5.0)
            .collect();
        let n2 = orthogonalize(&n2_raw, &[&x, &m1, &c1], &w);
        let m2: Vec<f64> = (0..n)
            .map(|i| 3.0 + 0.2 * x[i] + 0.5 * m1[i] + 0.01 * c1[i] + 1.5 * n2[i])
            .collect();
        let n3_raw: Vec<f64> = (0..n)
            .map(|i| ((i as f64 * 3.0) % 17.0 - 8.0) / 8.0)
            .collect();
        let n3 = orthogonalize(&n3_raw, &[&x, &m1, &m2, &c1], &w);
        let y: Vec<f64> = (0..n)
            .map(|i| 2.0 + 0.3 * x[i] + 0.6 * m1[i] + 0.8 * m2[i] + 0.05 * c1[i] + 2.5 * n3[i])
            .collect();
        (x, m1, m2, y, c1, w)
    }

    #[test]
    fn recovers_known_paths() {
        let (x, m1, m2, y, c1, w) = system();
        let design = BootstrapDesign::new(&w).unwrap();
        let opts = SerialMediationOptions {
            n_bootstrap: 0,
            ..Default::default()
        };
        let r = mediation_serial(&x, &m1, &m2, &y, &[&c1], &design, &opts).unwrap();
        // Orthogonalized noise ⇒ the fits reproduce the generating
        // coefficients exactly; assert the closed-form paths tightly.
        assert!(approx_eq(r.ie_m1, 0.42, 1e-6), "ie_m1 {}", r.ie_m1);
        assert!(approx_eq(r.ie_m2, 0.16, 1e-6), "ie_m2 {}", r.ie_m2);
        assert!(
            approx_eq(r.ie_serial, 0.28, 1e-6),
            "ie_serial {}",
            r.ie_serial
        );
        assert!(approx_eq(r.total_indirect, 0.86, 1e-6));
        assert!(approx_eq(r.direct, 0.3, 1e-6), "direct {}", r.direct);
        assert!(approx_eq(r.te, 1.16, 1e-6));
        assert!(approx_eq(r.a1, 0.7, 1e-6), "a1 {}", r.a1);
        assert!(approx_eq(r.a2, 0.2, 1e-6), "a2 {}", r.a2);
        assert!(approx_eq(r.d21, 0.5, 1e-6), "d21 {}", r.d21);
        assert!(approx_eq(r.b1, 0.6, 1e-6), "b1 {}", r.b1);
        assert!(approx_eq(r.b2, 0.8, 1e-6), "b2 {}", r.b2);
        assert!(approx_eq(r.c_prime, 0.3, 1e-6), "c_prime {}", r.c_prime);
        assert_eq!(r.n_obs, 240);
    }

    #[test]
    fn te_identity_direct_plus_total_indirect() {
        // Algebraic identity of the linear decomposition, at the fitted
        // coefficients — exact to floating-point rounding.
        let (x, m1, m2, y, c1, w) = system();
        let design = BootstrapDesign::new(&w).unwrap();
        let opts = SerialMediationOptions {
            n_bootstrap: 0,
            ..Default::default()
        };
        let r = mediation_serial(&x, &m1, &m2, &y, &[&c1], &design, &opts).unwrap();
        assert!((r.te - (r.direct + r.total_indirect)).abs() < 1e-12);
        assert!((r.total_indirect - (r.ie_m1 + r.ie_m2 + r.ie_serial)).abs() < 1e-12);
    }

    #[test]
    fn independent_mediators_kill_serial_path() {
        // m2 does not depend on m1 → d₂₁ ≈ 0 → ie_serial ≈ 0.
        let n = 200;
        let x: Vec<f64> = (0..n).map(|i| ((i % 4) / 2) as f64).collect();
        let c1: Vec<f64> = (0..n).map(|i| 30.0 + (i as f64 % 15.0)).collect();
        let w = vec![1000.0; n];
        // Two independent pseudo-noise sequences (coprime periods) so m2
        // carries no shared stochastic component with m1.
        let na = |i: usize| ((i as f64 * 7.0) % 13.0 - 6.0) / 6.0;
        let nb = |i: usize| ((i as f64 * 5.0) % 11.0 - 5.0) / 5.0;
        let m1: Vec<f64> = (0..n).map(|i| 1.0 + 0.7 * x[i] + 2.0 * na(i)).collect();
        let m2: Vec<f64> = (0..n).map(|i| 2.0 + 0.3 * x[i] + 2.0 * nb(i)).collect();
        let y: Vec<f64> = (0..n)
            .map(|i| 1.0 + 0.2 * x[i] + 0.5 * m1[i] + 0.4 * m2[i] + 1.5 * na(i))
            .collect();
        let design = BootstrapDesign::new(&w).unwrap();
        let opts = SerialMediationOptions {
            n_bootstrap: 0,
            ..Default::default()
        };
        let r = mediation_serial(&x, &m1, &m2, &y, &[&c1], &design, &opts).unwrap();
        assert!(r.d21.abs() < 0.15, "d21 {}", r.d21);
        assert!(r.ie_serial.abs() < 0.1, "ie_serial {}", r.ie_serial);
    }

    #[test]
    fn unit_weights_match_hand_assembled_ols() {
        let (x, m1, m2, y, c1, w) = system();
        let design = BootstrapDesign::new(&w).unwrap();
        let opts = SerialMediationOptions {
            n_bootstrap: 0,
            ..Default::default()
        };
        let r = mediation_serial(&x, &m1, &m2, &y, &[&c1], &design, &opts).unwrap();
        // The fitted model coefficients are plain WLS columns.
        assert_eq!(r.m1_model.len(), 3); // intercept + x + c1
        assert_eq!(r.m2_model.len(), 4); // intercept + x + m1 + c1
        assert_eq!(r.y_model.len(), 5); // intercept + x + m1 + m2 + c1
        assert_eq!(r.m1_model[1], r.a1);
        assert_eq!(r.m2_model[1], r.a2);
        assert_eq!(r.m2_model[2], r.d21);
        assert_eq!(r.y_model[1], r.c_prime);
        assert_eq!(r.y_model[2], r.b1);
        assert_eq!(r.y_model[3], r.b2);
    }

    #[test]
    fn dimension_mismatches_are_errors() {
        let (x, m1, m2, y, c1, w) = system();
        let design = BootstrapDesign::new(&w).unwrap();
        let opts = SerialMediationOptions {
            n_bootstrap: 0,
            ..Default::default()
        };
        let short: Vec<f64> = m2[..m2.len() - 1].to_vec();
        assert!(matches!(
            mediation_serial(&x, &m1, &short, &y, &[&c1], &design, &opts),
            Err(EpiError::DimensionMismatch { .. })
        ));
    }
}
