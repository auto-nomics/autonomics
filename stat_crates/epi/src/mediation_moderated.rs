//! Survey-weighted moderated mediation: conditional indirect effects and
//! the index of moderated mediation.
//!
//! Implements the regression-based approach of Hayes (2015) under WLS
//! sampling weights, for the moderated-mediation arm of Li et al. 2026
//! (Figure 6C): the indirect effect of X on Y through M is allowed to
//! vary with a moderator W, and the *index of moderated mediation*
//! summarises whether that variation is real. The moderator parameter is
//! named `moderator` precisely to avoid clashing with the sampling
//! `weights` of [`BootstrapDesign`].
//!
//! # Models (WLS, same weights; stage picks the interaction columns)
//!
//! ```text
//! First:  M = α + a_x·X + a_w·W + a_xw·(X·W) + Σ α_c·C + ε_M
//!         Y = β + c_x·X + b_m·M + b_w·W [+ β_xm·(X·M)] + Σ β_c·C + ε_Y
//! Second: M = α + a_x·X + a_w·W + Σ α_c·C + ε_M
//!         Y = β + c_x·X + b_m·M + b_w·W + b_mw·(M·W) [+ β_xm·(X·M)] + …
//! Both:   M = α + a_x·X + a_w·W + a_xw·(X·W) + Σ α_c·C + ε_M
//!         Y = β + c_x·X + b_m·M + b_w·W + c_xw·(X·W) + b_mw·(M·W) [+ β_xm·(X·M)] + …
//! ```
//!
//! # Effects (Δx = x_treated − x_control, evaluated at moderator level w)
//!
//! ```text
//! a(w)  = a_x + a_xw·w
//! b(w)  = b_m + b_mw·w + β_xm·x_treated
//! ω(w)  = a(w) · b(w) · Δx          (conditional indirect effect)
//! c′(w) = (c_x + c_xw·w) · Δx       (conditional direct effect)
//! index of moderated mediation (Hayes 2015):
//!   first stage  = a_xw · b_m        (non-zero ⇒ ω varies with W)
//!   second stage = a_x  · b_mw
//! ```
//!
//! Under `stage = Both`, ω(w) is quadratic in w; the two single-path
//! indices are its linear-component parts — read the conditional curve.
//! With `include_xm_interaction` the indices keep their pure path-product
//! definitions above.
//!
//! # Moderator grid
//!
//! Defaults to the weighted mean of W flanked by ± one weighted
//! population SD (`sqrt(Σw(w−w̄)²/Σw)`); pass `w_grid` to override. The
//! grid is fixed once on the observed data and never recomputed inside
//! bootstrap replicates, so replicate distributions stay comparable.

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

use statkit::regression;

use crate::bootstrap::{self, BootstrapDesign, CiMethod};
use crate::error::{EpiError, Result};

// ── Options ────────────────────────────────────────────────────────────────

/// Where the moderation enters the causal chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModerationStage {
    /// `X × W` in the mediator model (PROCESS model 7).
    #[default]
    First,
    /// `M × W` in the outcome model (PROCESS model 14).
    Second,
    /// Both interactions (PROCESS model 8).
    Both,
}

/// Options for [`mediation_moderated`].
#[derive(Debug, Clone)]
pub struct ModeratedMediationOptions {
    /// Which stage(s) carry the moderation (default first).
    pub stage: ModerationStage,
    /// Include an `X × M` interaction in the outcome model (default false).
    pub include_xm_interaction: bool,
    /// Explicit moderator evaluation points; `None` ⇒ weighted mean ±
    /// weighted SD of the moderator (default).
    pub w_grid: Option<Vec<f64>>,
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

impl Default for ModeratedMediationOptions {
    fn default() -> Self {
        Self {
            stage: ModerationStage::First,
            include_xm_interaction: false,
            w_grid: None,
            ci_method: CiMethod::BiasCorrected,
            n_bootstrap: 1000,
            seed: 42,
            x_treated: 1.0,
            x_control: 0.0,
        }
    }
}

// ── Result ─────────────────────────────────────────────────────────────────

/// Conditional indirect effect at one moderator level.
#[derive(Debug, Clone)]
pub struct ConditionalIndirect {
    /// Moderator level `w`.
    pub w: f64,
    /// Conditional a-path `a(w) = a_x + a_xw·w`.
    pub a_path: f64,
    /// Conditional b-path `b(w) = b_m + b_mw·w + β_xm·x_treated`.
    pub b_path: f64,
    /// `a(w) · b(w) · Δx`.
    pub indirect: f64,
    /// Bootstrap 95% CI for `indirect`.
    pub ci: (f64, f64),
}

/// Result of a survey-weighted moderated mediation analysis.
#[derive(Debug, Clone)]
pub struct ModeratedMediationResult {
    /// Moderator evaluation points (as supplied or defaulted).
    pub w_points: Vec<f64>,
    /// Conditional indirect effects, one per grid point.
    pub conditional: Vec<ConditionalIndirect>,
    /// Conditional direct effects `(w, direct, ci)` per grid point.
    pub conditional_direct: Vec<(f64, f64, (f64, f64))>,
    /// First-stage index of moderated mediation `a_xw·b_m`.
    pub index_first_stage: f64,
    /// `index_first_stage` bootstrap 95% CI.
    pub index_first_stage_ci: (f64, f64),
    /// Second-stage index `a_x·b_mw`.
    pub index_second_stage: f64,
    /// `index_second_stage` bootstrap 95% CI.
    pub index_second_stage_ci: (f64, f64),
    /// Mediator-model coefficient on X.
    pub a_x: f64,
    /// Mediator-model coefficient on X:W (0 unless stage First/Both).
    pub a_xw: f64,
    /// Outcome-model coefficient on M.
    pub b_m: f64,
    /// Outcome-model coefficient on M:W (0 unless stage Second/Both).
    pub b_mw: f64,
    /// Outcome-model coefficient on X (conditional direct intercept).
    pub c_x: f64,
    /// Outcome-model coefficient on X:W (0 unless stage Both).
    pub c_xw: f64,
    /// Full mediator-model coefficients (intercept first).
    pub m_model: Vec<f64>,
    /// Full outcome-model coefficients (intercept first).
    pub y_model: Vec<f64>,
    /// Successful bootstrap replicates.
    pub n_bootstrap: usize,
    /// Number of observations.
    pub n_obs: usize,
}

/// Everything one two-model fit produces.
struct ModeratedFit {
    a_x: f64,
    a_xw: f64,
    b_m: f64,
    b_mw: f64,
    beta_xm: f64,
    c_x: f64,
    c_xw: f64,
    m_model: Vec<f64>,
    y_model: Vec<f64>,
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Perform a survey-weighted moderated mediation analysis.
///
/// `x` is the exposure, `m` the mediator, `moderator` the moderator W,
/// `y` the outcome, `covariates` optional confounders adjusted for in
/// both models, and `design` the sampling design (weights plus optional
/// strata/PSU for the bootstrap).
pub fn mediation_moderated(
    x: &[f64],
    m: &[f64],
    moderator: &[f64],
    y: &[f64],
    covariates: &[&[f64]],
    design: &BootstrapDesign,
    opts: &ModeratedMediationOptions,
) -> Result<ModeratedMediationResult> {
    let n = design.n();
    if n == 0 || x.len() != n || m.len() != n || moderator.len() != n || y.len() != n {
        return Err(EpiError::DimensionMismatch {
            a: n,
            b: x.len().max(m.len()).max(moderator.len()).max(y.len()),
        });
    }
    for c in covariates.iter() {
        if c.len() != n {
            return Err(EpiError::DimensionMismatch { a: n, b: c.len() });
        }
    }

    let w = design.normalized_weights();

    // ── Moderator grid: fixed once on the observed data ────────────────
    let w_points: Vec<f64> = match &opts.w_grid {
        Some(grid) => {
            if grid.iter().any(|v| !v.is_finite()) {
                return Err(EpiError::Numerical(
                    "w_grid contains non-finite values".to_string(),
                ));
            }
            grid.clone()
        }
        None => {
            let mu = weighted_mean(moderator, &w);
            let sd = weighted_sd_pop(moderator, &w);
            vec![mu - sd, mu, mu + sd]
        }
    };

    // ── Point estimates from the full data ─────────────────────────────
    let fit = fit_moderated(x, m, moderator, y, covariates, &w, opts)?;

    // ── Design-aware bootstrap (same grid every replicate) ─────────────
    let mut rng = ChaCha8Rng::seed_from_u64(opts.seed);
    let n_grid = w_points.len();
    let mut boot_indirect: Vec<Vec<f64>> = vec![Vec::new(); n_grid];
    let mut boot_direct: Vec<Vec<f64>> = vec![Vec::new(); n_grid];
    let mut boot_first: Vec<f64> = Vec::with_capacity(opts.n_bootstrap);
    let mut boot_second: Vec<f64> = Vec::with_capacity(opts.n_bootstrap);

    for _ in 0..opts.n_bootstrap {
        let idx = design.replicate_indices(&mut rng);
        let x_b: Vec<f64> = idx.iter().map(|&i| x[i]).collect();
        let m_b: Vec<f64> = idx.iter().map(|&i| m[i]).collect();
        let mod_b: Vec<f64> = idx.iter().map(|&i| moderator[i]).collect();
        let y_b: Vec<f64> = idx.iter().map(|&i| y[i]).collect();
        let w_b: Vec<f64> = idx.iter().map(|&i| w[i]).collect();
        let cov_b: Vec<Vec<f64>> = covariates
            .iter()
            .map(|c| idx.iter().map(|&i| c[i]).collect())
            .collect();
        let cov_slices: Vec<&[f64]> = cov_b.iter().map(|v| v.as_slice()).collect();

        if let Ok(b) = fit_moderated(&x_b, &m_b, &mod_b, &y_b, &cov_slices, &w_b, opts) {
            for (j, &wj) in w_points.iter().enumerate() {
                let (a_wj, b_wj) = conditional_paths(&b, opts, wj);
                boot_indirect[j].push(a_wj * b_wj);
                boot_direct[j].push((b.c_x + b.c_xw * wj) * dx(opts));
            }
            boot_first.push(b.a_xw * b.b_m);
            boot_second.push(b.a_x * b.b_mw);
        }
    }

    let method = opts.ci_method;
    let conditional: Vec<ConditionalIndirect> = w_points
        .iter()
        .enumerate()
        .map(|(j, &wj)| {
            let (a_wj, b_wj) = conditional_paths(&fit, opts, wj);
            let indirect = a_wj * b_wj * dx(opts);
            ConditionalIndirect {
                w: wj,
                a_path: a_wj,
                b_path: b_wj,
                indirect,
                ci: bootstrap::ci(method, &boot_indirect[j], indirect),
            }
        })
        .collect();
    let conditional_direct: Vec<(f64, f64, (f64, f64))> = w_points
        .iter()
        .enumerate()
        .map(|(j, &wj)| {
            let direct = (fit.c_x + fit.c_xw * wj) * dx(opts);
            (wj, direct, bootstrap::ci(method, &boot_direct[j], direct))
        })
        .collect();

    let index_first = fit.a_xw * fit.b_m;
    let index_second = fit.a_x * fit.b_mw;
    let n_success = boot_first.len();

    Ok(ModeratedMediationResult {
        w_points,
        conditional,
        conditional_direct,
        index_first_stage: index_first,
        index_first_stage_ci: bootstrap::ci(method, &boot_first, index_first),
        index_second_stage: index_second,
        index_second_stage_ci: bootstrap::ci(method, &boot_second, index_second),
        a_x: fit.a_x,
        a_xw: fit.a_xw,
        b_m: fit.b_m,
        b_mw: fit.b_mw,
        c_x: fit.c_x,
        c_xw: fit.c_xw,
        m_model: fit.m_model,
        y_model: fit.y_model,
        n_bootstrap: n_success,
        n_obs: n,
    })
}

// ── Core fitting ───────────────────────────────────────────────────────────

/// Fit the two WLS models for the selected stage.
fn fit_moderated(
    x: &[f64],
    m: &[f64],
    moderator: &[f64],
    y: &[f64],
    covariates: &[&[f64]],
    w: &[f64],
    opts: &ModeratedMediationOptions,
) -> Result<ModeratedFit> {
    // Materialised interaction columns.
    let xw: Vec<f64> = x.iter().zip(moderator).map(|(&xi, &wi)| xi * wi).collect();
    let mw: Vec<f64> = m.iter().zip(moderator).map(|(&mi, &wi)| mi * wi).collect();
    let xm: Vec<f64> = x.iter().zip(m).map(|(&xi, &mi)| xi * mi).collect();

    // ── Mediator model ─────────────────────────────────────────────────
    // Layout: [X, W, (X:W if stage First/Both), C…]. Positions recorded
    // are coefficient indices — +1 past the predictor index for the
    // leading intercept.
    let mut m_preds: Vec<&[f64]> = vec![x, moderator];
    let mut pos_axw: Option<usize> = None;
    if matches!(opts.stage, ModerationStage::First | ModerationStage::Both) {
        let coef_idx = m_preds.len() + 1;
        m_preds.push(&xw[..]);
        pos_axw = Some(coef_idx);
    }
    for c in covariates {
        m_preds.push(*c);
    }
    let m_fit = regression::wls(&m_preds, m, w, true).map_err(epi_from_stat)?;
    let a_x = m_fit.coefficients[1];
    let a_xw = pos_axw.map_or(0.0, |p| m_fit.coefficients[p]);

    // ── Outcome model ──────────────────────────────────────────────────
    // Layout: [X, M, W, (X:W if Both), (M:W if Second/Both), (X:M if
    // requested), C…]. Positions are coefficient indices (+1 intercept).
    let mut y_preds: Vec<&[f64]> = vec![x, m, moderator];
    let mut pos_cxw: Option<usize> = None;
    let mut pos_bmw: Option<usize> = None;
    let mut pos_xm: Option<usize> = None;
    if opts.stage == ModerationStage::Both {
        let coef_idx = y_preds.len() + 1;
        y_preds.push(&xw[..]);
        pos_cxw = Some(coef_idx);
    }
    if matches!(opts.stage, ModerationStage::Second | ModerationStage::Both) {
        let coef_idx = y_preds.len() + 1;
        y_preds.push(&mw[..]);
        pos_bmw = Some(coef_idx);
    }
    if opts.include_xm_interaction {
        let coef_idx = y_preds.len() + 1;
        y_preds.push(&xm[..]);
        pos_xm = Some(coef_idx);
    }
    for c in covariates {
        y_preds.push(*c);
    }
    let y_fit = regression::wls(&y_preds, y, w, true).map_err(epi_from_stat)?;
    let c_x = y_fit.coefficients[1];
    let b_m = y_fit.coefficients[2];
    let b_mw = pos_bmw.map_or(0.0, |p| y_fit.coefficients[p]);
    let beta_xm = pos_xm.map_or(0.0, |p| y_fit.coefficients[p]);
    let c_xw = pos_cxw.map_or(0.0, |p| y_fit.coefficients[p]);

    Ok(ModeratedFit {
        a_x,
        a_xw,
        b_m,
        b_mw,
        beta_xm,
        c_x,
        c_xw,
        m_model: m_fit.coefficients,
        y_model: y_fit.coefficients,
    })
}

/// Conditional paths at moderator level `w`: `(a(w), b(w))`.
fn conditional_paths(fit: &ModeratedFit, opts: &ModeratedMediationOptions, w: f64) -> (f64, f64) {
    (
        fit.a_x + fit.a_xw * w,
        fit.b_m + fit.b_mw * w + fit.beta_xm * opts.x_treated,
    )
}

/// Exposure contrast Δx.
fn dx(opts: &ModeratedMediationOptions) -> f64 {
    opts.x_treated - opts.x_control
}

fn weighted_mean(v: &[f64], w: &[f64]) -> f64 {
    let wsum: f64 = w.iter().sum();
    v.iter().zip(w).map(|(&vi, &wi)| wi * vi).sum::<f64>() / wsum
}

/// Weighted population SD `sqrt(Σw(v−v̄_w)²/Σw)`.
fn weighted_sd_pop(v: &[f64], w: &[f64]) -> f64 {
    let mu = weighted_mean(v, w);
    let wsum: f64 = w.iter().sum();
    (v.iter()
        .zip(w)
        .map(|(&vi, &wi)| wi * (vi - mu) * (vi - mu))
        .sum::<f64>()
        / wsum)
        .sqrt()
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

    /// First-stage system with known paths:
    /// a_x = 0.5, a_w = 0.3, a_xw = 0.4, c_x = 0.2, b_m = 0.7.
    /// ⇒ ω(w) = (0.5 + 0.4w)·0.7 = 0.35 + 0.28w, direct = 0.2,
    ///   index_first = 0.4·0.7 = 0.28. Noise is W-orthogonalized against
    /// each model's regressors, so WLS recovers the coefficients exactly.
    fn system() -> (Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>) {
        let n = 240;
        let x: Vec<f64> = (0..n).map(|i| ((i % 5) / 3) as f64).collect();
        let wm: Vec<f64> = (0..n).map(|i| ((i as f64 % 7.0) - 3.0) / 3.0).collect();
        let c1: Vec<f64> = (0..n).map(|i| 40.0 + (i as f64 % 19.0)).collect();
        let w: Vec<f64> = (0..n).map(|i| 4000.0 + 300.0 * (i as f64 % 13.0)).collect();
        let xw: Vec<f64> = x.iter().zip(&wm).map(|(&a, &b)| a * b).collect();
        let n1_raw: Vec<f64> = (0..n)
            .map(|i| ((i as f64 * 7.0) % 13.0 - 6.0) / 6.0)
            .collect();
        let n1 = orthogonalize(&n1_raw, &[&x, &wm, &xw, &c1], &w);
        let m: Vec<f64> = (0..n)
            .map(|i| 1.0 + 0.5 * x[i] + 0.3 * wm[i] + 0.4 * xw[i] + 2.0 * n1[i])
            .collect();
        let n2_raw: Vec<f64> = (0..n)
            .map(|i| ((i as f64 * 3.0) % 17.0 - 8.0) / 8.0)
            .collect();
        let n2 = orthogonalize(&n2_raw, &[&x, &m, &wm, &c1], &w);
        let y: Vec<f64> = (0..n)
            .map(|i| 2.0 + 0.2 * x[i] + 0.7 * m[i] + 0.1 * wm[i] + 0.05 * c1[i] + 2.5 * n2[i])
            .collect();
        (x, m, wm, y, c1, w)
    }

    #[test]
    fn first_stage_index_and_conditional_closed_form() {
        let (x, m, wm, y, c1, w) = system();
        let design = BootstrapDesign::new(&w).unwrap();
        let opts = ModeratedMediationOptions {
            w_grid: Some(vec![-1.0, 0.0, 1.0]),
            n_bootstrap: 0,
            ..Default::default()
        };
        let r = mediation_moderated(&x, &m, &wm, &y, &[&c1], &design, &opts).unwrap();
        assert!(approx_eq(r.a_x, 0.5, 1e-6), "a_x {}", r.a_x);
        assert!(approx_eq(r.a_xw, 0.4, 1e-6), "a_xw {}", r.a_xw);
        assert!(approx_eq(r.b_m, 0.7, 1e-6), "b_m {}", r.b_m);
        assert!(approx_eq(r.c_x, 0.2, 1e-6), "c_x {}", r.c_x);
        assert!(
            approx_eq(r.index_first_stage, 0.28, 1e-6),
            "index_first {}",
            r.index_first_stage
        );
        // ω(w) = 0.35 + 0.28w at w = −1, 0, +1.
        assert!(
            approx_eq(r.conditional[0].indirect, 0.07, 1e-6),
            "ω(-1) {}",
            r.conditional[0].indirect
        );
        assert!(
            approx_eq(r.conditional[1].indirect, 0.35, 1e-6),
            "ω(0) {}",
            r.conditional[1].indirect
        );
        assert!(
            approx_eq(r.conditional[2].indirect, 0.63, 1e-6),
            "ω(1) {}",
            r.conditional[2].indirect
        );
        // Conditional paths themselves.
        assert!(approx_eq(r.conditional[2].a_path, 0.9, 1e-6));
        assert!(approx_eq(r.conditional[2].b_path, 0.7, 1e-6));
        // Direct effect is unmoderated at this stage: 0.2 everywhere.
        for (wj, direct, _) in &r.conditional_direct {
            assert!(approx_eq(*direct, 0.2, 1e-6), "direct({wj}) {direct}");
        }
        assert_eq!(r.n_obs, 240);
    }

    #[test]
    fn no_moderation_gives_constant_indirect_and_zero_index() {
        // Same chain but a_xw = 0 in the DGP: ω(w) must not vary with w
        // and both indices must vanish. n1 is orthogonalized against the
        // *fitted* column set (which includes X:W even at DGP a_xw = 0),
        // so the fitted a_xw is exactly zero.
        let (x, _m, wm, _y, c1, w) = system();
        let n = x.len();
        let xw: Vec<f64> = x.iter().zip(&wm).map(|(&a, &b)| a * b).collect();
        let n1_raw: Vec<f64> = (0..n)
            .map(|i| ((i as f64 * 7.0) % 13.0 - 6.0) / 6.0)
            .collect();
        let n1 = orthogonalize(&n1_raw, &[&x, &wm, &xw, &c1], &w);
        let m: Vec<f64> = (0..n)
            .map(|i| 1.0 + 0.5 * x[i] + 0.3 * wm[i] + 2.0 * n1[i])
            .collect();
        let n2_raw: Vec<f64> = (0..n)
            .map(|i| ((i as f64 * 3.0) % 17.0 - 8.0) / 8.0)
            .collect();
        let n2 = orthogonalize(&n2_raw, &[&x, &m, &wm, &c1], &w);
        let y: Vec<f64> = (0..n)
            .map(|i| 2.0 + 0.2 * x[i] + 0.7 * m[i] + 0.1 * wm[i] + 2.5 * n2[i])
            .collect();

        let design = BootstrapDesign::new(&w).unwrap();
        let opts = ModeratedMediationOptions {
            w_grid: Some(vec![-1.0, 0.0, 1.0]),
            n_bootstrap: 0,
            ..Default::default()
        };
        let r = mediation_moderated(&x, &m, &wm, &y, &[&c1], &design, &opts).unwrap();
        assert!(r.a_xw.abs() < 1e-8, "a_xw {}", r.a_xw);
        assert!(r.index_first_stage.abs() < 1e-8);
        assert!(r.index_second_stage.abs() < 1e-8);
        let base = r.conditional[1].indirect;
        for c in &r.conditional {
            assert!(
                (c.indirect - base).abs() < 1e-8,
                "ω({}) {}",
                c.w,
                c.indirect
            );
        }
    }

    #[test]
    fn w_grid_used_verbatim() {
        let (x, m, wm, y, c1, w) = system();
        let design = BootstrapDesign::new(&w).unwrap();
        let opts = ModeratedMediationOptions {
            w_grid: Some(vec![0.25, 1.5, 7.0]),
            n_bootstrap: 0,
            ..Default::default()
        };
        let r = mediation_moderated(&x, &m, &wm, &y, &[&c1], &design, &opts).unwrap();
        assert_eq!(r.w_points, vec![0.25, 1.5, 7.0]);
        assert_eq!(r.conditional.len(), 3);
        assert_eq!(r.conditional_direct.len(), 3);
        for (c, &wj) in r.conditional.iter().zip(&r.w_points) {
            assert_eq!(c.w, wj);
        }
    }

    #[test]
    fn moderator_shift_leaves_conditional_effects_invariant() {
        // Relabelling the moderator W′ = W + 10 with a correspondingly
        // shifted grid reproduces the same conditional effects: a′(w+10)
        // = a(w), so ω and the indices are unchanged.
        let (x, m, wm, y, c1, w) = system();
        let wm_shift: Vec<f64> = wm.iter().map(|&v| v + 10.0).collect();
        let design = BootstrapDesign::new(&w).unwrap();
        let base_opts = ModeratedMediationOptions {
            w_grid: Some(vec![0.0, 1.0]),
            n_bootstrap: 0,
            ..Default::default()
        };
        let shift_opts = ModeratedMediationOptions {
            w_grid: Some(vec![10.0, 11.0]),
            n_bootstrap: 0,
            ..Default::default()
        };
        let a = mediation_moderated(&x, &m, &wm, &y, &[&c1], &design, &base_opts).unwrap();
        let b = mediation_moderated(&x, &m, &wm_shift, &y, &[&c1], &design, &shift_opts).unwrap();
        for (ca, cb) in a.conditional.iter().zip(&b.conditional) {
            assert!(
                (ca.indirect - cb.indirect).abs() < 1e-8,
                "ω shift: {} vs {}",
                ca.indirect,
                cb.indirect
            );
            assert!((ca.a_path - cb.a_path).abs() < 1e-8);
        }
        for ((_, da, _), (_, db, _)) in a.conditional_direct.iter().zip(&b.conditional_direct) {
            assert!((da - db).abs() < 1e-8);
        }
        assert!((a.index_first_stage - b.index_first_stage).abs() < 1e-8);
        assert!((a.a_xw - b.a_xw).abs() < 1e-8);
    }

    #[test]
    fn dimension_mismatches_are_errors() {
        let (x, m, wm, y, c1, w) = system();
        let design = BootstrapDesign::new(&w).unwrap();
        let opts = ModeratedMediationOptions {
            n_bootstrap: 0,
            ..Default::default()
        };
        let short: Vec<f64> = wm[..wm.len() - 1].to_vec();
        assert!(matches!(
            mediation_moderated(&x, &m, &short, &y, &[&c1], &design, &opts),
            Err(EpiError::DimensionMismatch { .. })
        ));
    }
}
