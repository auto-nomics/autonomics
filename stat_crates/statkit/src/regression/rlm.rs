//! Robust linear regression (M-estimation) with prior case weights.
//!
//! Sensitivity-analysis counterpart to WLS for survey-style analyses (e.g.
//! NHANES readouts re-fit after excluding influential points): least-squares
//! estimates are pulled by gross outliers, and the weighted robust fit asks
//! whether conclusions survive down-weighting them.
//!
//! Estimation follows `MASS::rlm` with `method = "M"`, `wt.method = "case"`,
//! `scale.est = "MAD"` (Venables & Ripley, *Modern Applied Statistics with
//! S*, §8.3):
//!
//! * the objective `Σᵢ wᵢ·s²·ρ(rᵢ/s)` is minimised by iteratively
//!   reweighted least squares with per-observation weight
//!   `wᵢ·ψ(uᵢ)/uᵢ`, `uᵢ = rᵢ/s` — i.e. a plain WLS solve of `y` on `X`
//!   each iteration, started from the prior-weighted least-squares fit
//!   (`lm.wfit` start in MASS);
//! * the residual scale is MASS's `wmad`: the *weighted* median of `|r|`
//!   (uncentred) divided by 0.6745, recomputed every iteration;
//! * convergence is MASS's `irls.delta` on residuals,
//!   `‖r_new − r_old‖ / ‖r_old‖ ≤ acc`;
//! * Huber's ψ with `k = 1.345` and Tukey's biweight with `c = 4.685` are
//!   the classical 95%-efficiency choices at the Gaussian model — Huber
//!   keeps all observations with bounded influence, Tukey actually rejects
//!   gross outliers (weight exactly 0 beyond `c`).
//!
//! Standard errors reproduce `summary.rlm` with its default
//! `method = "XtX"` under case weights — a ψ′-corrected sandwich:
//! `seⱼ = √S·κ/mn · √[(Xᵀ diag(w) X)⁻¹]ⱼⱼ` with
//! `S = Σwᵢ(ψ̂ᵢrᵢ)²/(Σwᵢ−p)`, `mn = Σwᵢψ′ᵢ/Σwᵢ` and the small-sample
//! inflation `κ = 1 + p(m₂ − m₁²/Σw)/(Σw−1)·Σw·mn²` — where the
//! design enters through the *prior* weights only, not the robust ones.
//! p-values use Student-t on `n − p` residual degrees of freedom (MASS
//! prints t values only).
//!
//! Parity notes: non-convergence within `max_iter` returns
//! `converged: false` with the last iterate (MASS warns and does the same),
//! while only numerical breakdown (singular normal equations, non-finite
//! coefficients) is an `Err`.

use statrs::distribution::{ContinuousCDF, StudentsT};

use super::wls;
use crate::error::{Result, StatError};

/// MAD consistency constant, hardcoded in MASS's `wmad`.
const MAD_CONSTANT: f64 = 0.6745;

// ── Options ────────────────────────────────────────────────────────────────

/// The ψ (psi) function defining the M-estimator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PsiFunction {
    /// Huber's ψ: quadratic core, linear tails — `w = 1` for `|u| ≤ k`,
    /// `w = k/|u|` beyond. Default `k = 1.345`.
    Huber,
    /// Tukey's biweight: `w = (1 − u²/c²)²` for `|u| ≤ c`, exactly `0`
    /// beyond (gross outliers rejected). Default `c = 4.685`.
    TukeyBisquare,
}

/// Options for [`rlm`].
#[derive(Debug, Clone)]
pub struct RlmOptions {
    /// ψ function (default Huber, matching `MASS::rlm`).
    pub psi: PsiFunction,
    /// Huber tuning constant `k` (default 1.345).
    pub huber_k: f64,
    /// Tukey biweight constant `c` (default 4.685).
    pub tukey_c: f64,
    /// Maximum IRLS iterations (default 20, as MASS `maxit`).
    pub max_iter: usize,
    /// Convergence tolerance on the largest coefficient step
    /// (default 1e-4, as MASS `acc`).
    pub acc: f64,
}

impl Default for RlmOptions {
    fn default() -> Self {
        Self {
            psi: PsiFunction::Huber,
            huber_k: 1.345,
            tukey_c: 4.685,
            max_iter: 20,
            acc: 1e-4,
        }
    }
}

// ── Result ─────────────────────────────────────────────────────────────────

/// Result of a robust linear fit.
#[derive(Debug, Clone)]
pub struct RlmResult {
    /// Fitted coefficients, intercept first (an intercept is always fit).
    pub coefficients: Vec<f64>,
    /// ψ′-corrected sandwich SEs (see module docs), matching
    /// `summary.rlm` with case weights.
    pub std_errors: Vec<f64>,
    /// `t = coefficient / std_error`.
    pub t_stats: Vec<f64>,
    /// Two-sided p-value from Student-t with `n − p` d.o.f.
    pub p_values: Vec<f64>,
    /// Fitted values `ŷ = Xβ`.
    pub fitted: Vec<f64>,
    /// Residuals `y − ŷ`.
    pub residuals: Vec<f64>,
    /// Final IRLS weights `wᵢ·ψ(uᵢ)/uᵢ` — inspect these to spot the
    /// observations the fit down-weighted or rejected.
    pub robust_weights: Vec<f64>,
    /// Final residual scale `MAD(r)/0.6745`.
    pub scale: f64,
    /// Number of observations.
    pub n_obs: usize,
    /// Number of parameters (predictors + intercept).
    pub n_params: usize,
    /// Whether the IRLS loop met the step tolerance.
    pub converged: bool,
    /// IRLS iterations at the reported fit.
    pub n_iter: usize,
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Robustly regress `y` on `predictors` with prior case `weights`.
///
/// An intercept is always included as coefficient 0. `weights` are
/// frequency-style case weights (`wt.method = "case"` in MASS): they enter
/// the objective multiplicatively, and scaling all of them by a constant
/// leaves coefficients (and the MAD scale) unchanged while shrinking the
/// standard errors by `1/√c`.
pub fn rlm(
    predictors: &[&[f64]],
    y: &[f64],
    weights: &[f64],
    opts: &RlmOptions,
) -> Result<RlmResult> {
    let n = y.len();
    if n == 0 {
        return Err(StatError::EmptyInput);
    }
    if weights.len() != n {
        return Err(StatError::LengthMismatch {
            a: n,
            b: weights.len(),
        });
    }
    for &w in weights.iter() {
        if !w.is_finite() || w < 0.0 {
            return Err(StatError::InvalidWeights);
        }
    }
    for p in predictors.iter() {
        if p.len() != n {
            return Err(StatError::LengthMismatch { a: n, b: p.len() });
        }
    }
    for (i, &v) in y.iter().enumerate() {
        if !v.is_finite() {
            return Err(StatError::InvalidInput(format!(
                "response at position {i} is not finite"
            )));
        }
    }

    let p = predictors.len() + 1; // intercept always present
    if n <= p {
        return Err(StatError::InsufficientData {
            min: p + 1,
            actual: n,
        });
    }

    // ── IRLS from the prior-weighted least-squares start ──────────────
    let mut beta = wls(predictors, y, weights, true)?.coefficients;
    let mut converged = false;
    let mut n_iter = 0usize;

    for it in 0..opts.max_iter {
        n_iter = it + 1;
        let r_old = residuals(predictors, y, &beta);
        let s = wmad(&r_old, weights);
        if !s.is_finite() {
            return Err(StatError::Numerical(
                "residual scale is not finite".to_string(),
            ));
        }
        if s < 1e-10 {
            // Residuals vanished — the current fit is exact (MASS: scale == 0).
            converged = true;
            break;
        }
        let w = robust_weights(&r_old, weights, s, opts);
        let new_beta = solve_normal(predictors, y, &w, p)?;
        if new_beta.iter().any(|b| !b.is_finite()) {
            return Err(StatError::Numerical(
                "coefficients diverged during IRLS".to_string(),
            ));
        }
        // MASS irls.delta on residuals: ‖r_new − r_old‖ / ‖r_old‖.
        let r_new = residuals(predictors, y, &new_beta);
        let num: f64 = r_old
            .iter()
            .zip(&r_new)
            .map(|(&a, &b)| (a - b) * (a - b))
            .sum();
        let den = r_old.iter().map(|&a| a * a).sum::<f64>().max(1e-20);
        beta = new_beta;
        if (num / den).sqrt() <= opts.acc {
            converged = true;
            break;
        }
    }

    // ── Final quantities at the reported fit ───────────────────────────
    let fitted = fitted_values(predictors, &beta);
    let r: Vec<f64> = y.iter().zip(&fitted).map(|(&yi, &fi)| yi - fi).collect();
    let s = wmad(&r, weights);
    let psi_only: Vec<f64> = r
        .iter()
        .map(|&ri| psi_weight(opts.psi, ri / s, opts.huber_k, opts.tukey_c))
        .collect();
    let w: Vec<f64> = psi_only
        .iter()
        .zip(weights)
        .map(|(&pi, &wi)| wi * pi)
        .collect();

    // summary.rlm(method = "XtX", wt.method = "case") sandwich:
    // se_j = sqrt(S)·κ/mn · sqrt(diag((Xᵀ diag(prior) X)⁻¹)_jj).
    let std_errors: Vec<f64> = if s > 0.0 {
        let nn: f64 = weights.iter().sum();
        let big_s = weights
            .iter()
            .zip(&r)
            .zip(&psi_only)
            .map(|((&wi, &ri), &pi)| wi * (ri * pi) * (ri * pi))
            .sum::<f64>()
            / (nn - p as f64);
        let psip: Vec<f64> = r
            .iter()
            .map(|&ri| psi_prime(opts.psi, ri / s, opts.huber_k, opts.tukey_c))
            .collect();
        let m1 = weights
            .iter()
            .zip(&psip)
            .map(|(&wi, &pi)| wi * pi)
            .sum::<f64>();
        let m2 = weights
            .iter()
            .zip(&psip)
            .map(|(&wi, &pi)| wi * pi * pi)
            .sum::<f64>();
        let mn = m1 / nn;
        let kappa = 1.0 + p as f64 * (m2 - m1 * m1 / nn) / ((nn - 1.0) * nn * mn * mn);
        let stddev = big_s.sqrt() * (kappa / mn);
        let xtx = normal_matrix(predictors, weights, p);
        let inv = invert_symmetric(&xtx, p)?;
        (0..p)
            .map(|j| (stddev * stddev * inv[j][j]).max(0.0).sqrt())
            .collect()
    } else {
        vec![0.0; p]
    };

    let t_dist = StudentsT::new(0.0, 1.0, (n - p) as f64)
        .map_err(|e| StatError::Numerical(format!("StudentsT: {e}")))?;
    let t_stats: Vec<f64> = (0..p)
        .map(|j| {
            if std_errors[j] > 0.0 {
                beta[j] / std_errors[j]
            } else {
                f64::NAN
            }
        })
        .collect();
    let p_values: Vec<f64> = (0..p)
        .map(|j| {
            let t = t_stats[j].abs();
            if t.is_finite() {
                2.0 * t_dist.sf(t)
            } else {
                f64::NAN
            }
        })
        .collect();

    Ok(RlmResult {
        coefficients: beta,
        std_errors,
        t_stats,
        p_values,
        fitted,
        residuals: r,
        robust_weights: w,
        scale: s,
        n_obs: n,
        n_params: p,
        converged,
        n_iter,
    })
}

// ── Private helpers ────────────────────────────────────────────────────────

/// IRLS weight `ψ(u)/u` for one standardised residual.
fn psi_weight(psi: PsiFunction, u: f64, huber_k: f64, tukey_c: f64) -> f64 {
    let au = u.abs();
    match psi {
        PsiFunction::Huber => {
            if au <= huber_k {
                1.0
            } else {
                huber_k / au
            }
        }
        PsiFunction::TukeyBisquare => {
            if au <= tukey_c {
                let t = 1.0 - (u * u) / (tukey_c * tukey_c);
                t * t
            } else {
                0.0
            }
        }
    }
}

/// Derivative `dψ̂/du` of the estimating function at `u`, as supplied by
/// MASS's psi functions with `deriv = 1` — enters the sandwich standard
/// errors through `mn`/`κ`.
fn psi_prime(psi: PsiFunction, u: f64, huber_k: f64, tukey_c: f64) -> f64 {
    let au = u.abs();
    match psi {
        // Inside the core ψ̂(u) = u → slope 1; flat tails beyond.
        PsiFunction::Huber => {
            if au <= huber_k {
                1.0
            } else {
                0.0
            }
        }
        // ψ̂(u) = u(1−u²/c²)² → (1−t)(1−5t) with t = (u/c)², zero beyond c.
        PsiFunction::TukeyBisquare => {
            let t = (u * u) / (tukey_c * tukey_c);
            if t < 1.0 {
                (1.0 - t) * (1.0 - 5.0 * t)
            } else {
                0.0
            }
        }
    }
}

/// Combined case×ψ weights at residual scale `s`.
fn robust_weights(r: &[f64], prior: &[f64], s: f64, opts: &RlmOptions) -> Vec<f64> {
    r.iter()
        .zip(prior)
        .map(|(&ri, &wi)| wi * psi_weight(opts.psi, ri / s, opts.huber_k, opts.tukey_c))
        .collect()
}

/// `ŷ = Xβ` (intercept in `beta[0]`).
fn fitted_values(predictors: &[&[f64]], beta: &[f64]) -> Vec<f64> {
    let n = predictors.first().map_or(0, |p| p.len());
    (0..n)
        .map(|i| {
            beta[0]
                + predictors
                    .iter()
                    .zip(&beta[1..])
                    .map(|(col, &b)| col[i] * b)
                    .sum::<f64>()
        })
        .collect()
}

/// Residuals at the current coefficients.
fn residuals(predictors: &[&[f64]], y: &[f64], beta: &[f64]) -> Vec<f64> {
    let f = fitted_values(predictors, beta);
    y.iter().zip(&f).map(|(&yi, &fi)| yi - fi).collect()
}

/// MASS `wmad`: weighted median of `|r|` (uncentred) over 0.6745, computed
/// on the weighted empirical CDF — the scale estimate of `rlm` with
/// `scale.est = "MAD"` and case weights.
fn wmad(r: &[f64], prior: &[f64]) -> f64 {
    let mut idx: Vec<usize> = (0..r.len()).collect();
    idx.sort_by(|&a, &b| {
        r[a].abs()
            .partial_cmp(&r[b].abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let total: f64 = prior.iter().sum();
    let p: Vec<f64> = {
        let mut cum = 0.0;
        idx.iter()
            .map(|&i| {
                cum += prior[i];
                cum / total
            })
            .collect()
    };
    // Number of strictly-below-median order statistics (R: sum(p < 0.5)).
    let n_lt = p.iter().filter(|&&v| v < 0.5).count();
    let a = r[idx[n_lt]].abs();
    if p.get(n_lt).is_some_and(|&v| v > 0.5) {
        a / MAD_CONSTANT
    } else {
        // Weighted CDF hits ½ exactly: average the bracketing order stats.
        let b = r[idx[(n_lt + 1).min(idx.len() - 1)]].abs();
        (a + b) / (2.0 * MAD_CONSTANT)
    }
}

/// Assemble `Xᵀ W X` (intercept column first).
fn normal_matrix(predictors: &[&[f64]], w: &[f64], p: usize) -> Vec<Vec<f64>> {
    let n = w.len();
    let mut xtwx = vec![vec![0.0_f64; p]; p];
    for i in 0..n {
        // Row of the design matrix, intercept first.
        let row = |j: usize| -> f64 { if j == 0 { 1.0 } else { predictors[j - 1][i] } };
        for a in 0..p {
            for b in a..p {
                let s = w[i] * row(a) * row(b);
                if a == b {
                    xtwx[a][b] += s;
                } else {
                    xtwx[a][b] += s;
                    xtwx[b][a] += s;
                }
            }
        }
    }
    xtwx
}

/// Solve `(Xᵀ W X) β = Xᵀ W y` by Cholesky (intercept column first).
fn solve_normal(predictors: &[&[f64]], y: &[f64], w: &[f64], p: usize) -> Result<Vec<f64>> {
    let n = y.len();
    let xtwx = normal_matrix(predictors, w, p);
    let mut xtwy = vec![0.0_f64; p];
    for i in 0..n {
        for a in 0..p {
            let x_a = if a == 0 { 1.0 } else { predictors[a - 1][i] };
            xtwy[a] += w[i] * x_a * y[i];
        }
    }
    solve_symmetric(&xtwx, &xtwy)
}

/// Cholesky solve of a small symmetric positive-definite system.
fn solve_symmetric(a: &[Vec<f64>], b: &[f64]) -> Result<Vec<f64>> {
    let n = b.len();
    let mut l = vec![vec![0.0_f64; n]; n];
    for i in 0..n {
        for j in 0..=i {
            let mut s = a[i][j];
            for k in 0..j {
                s -= l[i][k] * l[j][k];
            }
            if i == j {
                if !(s.is_finite() && s > 0.0) {
                    return Err(StatError::SingularMatrix);
                }
                l[i][j] = s.sqrt();
            } else {
                l[i][j] = s / l[j][j];
            }
        }
    }
    // Forward substitution.
    let mut yv = vec![0.0_f64; n];
    for i in 0..n {
        let mut s = b[i];
        for k in 0..i {
            s -= l[i][k] * yv[k];
        }
        yv[i] = s / l[i][i];
    }
    // Back substitution.
    let mut xv = vec![0.0_f64; n];
    for i in (0..n).rev() {
        let mut s = yv[i];
        for k in (i + 1)..n {
            s -= l[k][i] * xv[k];
        }
        xv[i] = s / l[i][i];
    }
    Ok(xv)
}

/// Invert a small symmetric positive-definite matrix via its Cholesky
/// factor.
fn invert_symmetric(a: &[Vec<f64>], n: usize) -> Result<Vec<Vec<f64>>> {
    let mut l = vec![vec![0.0_f64; n]; n];
    for i in 0..n {
        for j in 0..=i {
            let mut s = a[i][j];
            for k in 0..j {
                s -= l[i][k] * l[j][k];
            }
            if i == j {
                if !(s.is_finite() && s > 0.0) {
                    return Err(StatError::SingularMatrix);
                }
                l[i][j] = s.sqrt();
            } else {
                l[i][j] = s / l[j][j];
            }
        }
    }
    // (L Lᵀ)⁻¹ = L⁻ᵀ L⁻¹; invert L by solving against unit vectors.
    let mut inv = vec![vec![0.0_f64; n]; n];
    for col in 0..n {
        let mut e = vec![0.0_f64; n];
        e[col] = 1.0;
        let mut u = vec![0.0_f64; n];
        for i in 0..n {
            let mut s = e[i];
            for k in 0..i {
                s -= l[i][k] * u[k];
            }
            u[i] = s / l[i][i];
        }
        let mut v = vec![0.0_f64; n];
        for i in (0..n).rev() {
            let mut s = u[i];
            for k in (i + 1)..n {
                s -= l[k][i] * v[k];
            }
            v[i] = s / l[i][i];
        }
        for i in 0..n {
            inv[i][col] = v[i];
        }
    }
    Ok(inv)
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::regression::ols;

    fn approx_eq(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * b.abs().max(1.0)
    }

    /// Deterministic "clean" data: residuals stay far inside the Huber
    /// threshold, so every ψ weight is exactly 1 and rlm ≡ OLS.
    fn clean_data() -> (Vec<f64>, Vec<f64>, Vec<f64>) {
        let n = 40;
        let x: Vec<f64> = (0..n).map(|i| (i as f64) / 4.0).collect();
        let y: Vec<f64> = (0..n)
            .map(|i| 1.0 + 2.0 * x[i] + 0.01 * ((i as f64) % 5.0 - 2.0))
            .collect();
        let w = vec![1.0; n];
        (x, y, w)
    }

    #[test]
    fn clean_data_matches_ols_exactly() {
        let (x, y, w) = clean_data();
        let o = ols(&[&x[..]], &y, true).unwrap();
        let r = rlm(&[&x[..]], &y, &w, &RlmOptions::default()).unwrap();
        for j in 0..2 {
            assert!(
                approx_eq(r.coefficients[j], o.coefficients[j], 1e-8),
                "coef[{j}]: rlm {} vs ols {}",
                r.coefficients[j],
                o.coefficients[j]
            );
        }
        assert!(r.robust_weights.iter().all(|&wi| (wi - 1.0).abs() < 1e-12));
        assert!(r.converged);
    }

    #[test]
    fn outlier_downweighted_slope_recovered() {
        // True slope 2; one gross outlier that would drag OLS to ~2.3.
        let (x, y, w) = clean_data();
        let mut y = y;
        y[7] += 40.0;
        let o = ols(&[&x[..]], &y, true).unwrap();
        let hub = rlm(&[&x[..]], &y, &w, &RlmOptions::default()).unwrap();
        let tuk = rlm(
            &[&x[..]],
            &y,
            &w,
            &RlmOptions {
                psi: PsiFunction::TukeyBisquare,
                ..RlmOptions::default()
            },
        )
        .unwrap();
        let err = |f: &dyn Fn() -> f64| (f() - 2.0).abs();
        let ols_slope = || o.coefficients[1];
        let hub_slope = || hub.coefficients[1];
        let tuk_slope = || tuk.coefficients[1];
        // Both robust fits recover the slope where OLS is dragged off.
        assert!(err(&hub_slope) < err(&ols_slope) / 2.0);
        assert!(err(&tuk_slope) < err(&ols_slope) / 2.0);
        // Tukey rejects the outlier outright; Huber only bounds it.
        assert_eq!(tuk.robust_weights[7], 0.0);
        assert!(hub.robust_weights[7] > 0.0 && hub.robust_weights[7] < 1.0);
        // Inliers keep (approximately) full weight under Huber; Tukey's
        // redescending ψ shaves even moderate residuals (u ≈ 1.35 → 0.84).
        assert!(hub.robust_weights[0] > 0.99);
        assert!(tuk.robust_weights[0] > 0.8);
    }

    #[test]
    fn prior_weight_scaling_leaves_fit_unchanged() {
        let (x, y, w) = clean_data();
        let mut w100 = w.clone();
        for wi in &mut w100 {
            *wi *= 100.0;
        }
        let a = rlm(&[&x[..]], &y, &w, &RlmOptions::default()).unwrap();
        let b = rlm(&[&x[..]], &y, &w100, &RlmOptions::default()).unwrap();
        for j in 0..2 {
            assert!(approx_eq(a.coefficients[j], b.coefficients[j], 1e-8));
        }
        // robust_weights carry the prior, so they scale with it.
        assert_eq!(a.robust_weights.len(), b.robust_weights.len());
        for i in 0..a.robust_weights.len() {
            assert!(approx_eq(
                100.0 * a.robust_weights[i],
                b.robust_weights[i],
                1e-8
            ));
        }
        assert!(approx_eq(a.scale, b.scale, 1e-8));
    }

    #[test]
    fn psi_weight_closed_form() {
        let o = RlmOptions::default();
        // Huber: 1 inside k, k/|u| outside.
        assert_eq!(
            psi_weight(PsiFunction::Huber, 0.0, o.huber_k, o.tukey_c),
            1.0
        );
        assert_eq!(
            psi_weight(PsiFunction::Huber, o.huber_k, o.huber_k, o.tukey_c),
            1.0
        );
        assert!(
            (psi_weight(PsiFunction::Huber, 2.0 * o.huber_k, o.huber_k, o.tukey_c) - 0.5).abs()
                < 1e-12
        );
        // Tukey: (1 − u²/c²)² inside c, 0 outside.
        let c = o.tukey_c;
        assert_eq!(
            psi_weight(PsiFunction::TukeyBisquare, 0.0, o.huber_k, c),
            1.0
        );
        let u = c / 2.0_f64.sqrt();
        assert!((psi_weight(PsiFunction::TukeyBisquare, u, o.huber_k, c) - 0.25).abs() < 1e-12);
        assert_eq!(psi_weight(PsiFunction::TukeyBisquare, c, o.huber_k, c), 0.0);
        assert_eq!(
            psi_weight(PsiFunction::TukeyBisquare, 2.0 * c, o.huber_k, c),
            0.0
        );
    }

    #[test]
    fn se_formula_matches_recomputation() {
        // Rebuild the summary.rlm XtX sandwich from the returned quantities
        // for a 2-parameter fit: se_j = sqrt(S)·κ/mn · sqrt(inv_jj) with the
        // design entering through prior weights only.
        let (x, y, w) = clean_data();
        let mut y = y;
        y[11] -= 15.0; // ensure non-trivial robust weights
        let o = RlmOptions::default();
        let r = rlm(&[&x[..]], &y, &w, &o).unwrap();
        let n = r.n_obs;
        let s = r.scale;

        let psi_only: Vec<f64> = r
            .residuals
            .iter()
            .map(|&ri| psi_weight(o.psi, ri / s, o.huber_k, o.tukey_c))
            .collect();
        let psip: Vec<f64> = r
            .residuals
            .iter()
            .map(|&ri| psi_prime(o.psi, ri / s, o.huber_k, o.tukey_c))
            .collect();
        let nn: f64 = w.iter().sum();
        let big_s = (0..n)
            .map(|i| w[i] * (r.residuals[i] * psi_only[i]).powi(2))
            .sum::<f64>()
            / (nn - 2.0);
        let m1 = (0..n).map(|i| w[i] * psip[i]).sum::<f64>();
        let m2 = (0..n).map(|i| w[i] * psip[i] * psip[i]).sum::<f64>();
        let mn = m1 / nn;
        let kappa = 1.0 + 2.0 * (m2 - m1 * m1 / nn) / ((nn - 1.0) * nn * mn * mn);
        let stddev = big_s.sqrt() * (kappa / mn);

        // (Xᵀ diag(w) X)⁻¹ diagonal by hand for the 2×2 case.
        let sw: f64 = w.iter().sum();
        let swx: f64 = (0..n).map(|i| w[i] * x[i]).sum();
        let swxx: f64 = (0..n).map(|i| w[i] * x[i] * x[i]).sum();
        let det = sw * swxx - swx * swx;
        let inv00 = swxx / det;
        let inv11 = sw / det;
        assert!((r.std_errors[0] - (stddev * stddev * inv00).sqrt()).abs() < 1e-10);
        assert!((r.std_errors[1] - (stddev * stddev * inv11).sqrt()).abs() < 1e-10);
    }

    #[test]
    fn rejects_singular_and_invalid_inputs() {
        let (x, y, w) = clean_data();
        // Duplicated predictor → singular normal equations.
        assert!(matches!(
            rlm(&[&x[..], &x[..]], &y, &w, &RlmOptions::default()),
            Err(StatError::SingularMatrix)
        ));
        // Tukey with a tiny cutoff kills every observation.
        assert!(matches!(
            rlm(
                &[&x[..]],
                &y,
                &w,
                &RlmOptions {
                    psi: PsiFunction::TukeyBisquare,
                    tukey_c: 1e-6,
                    ..RlmOptions::default()
                }
            ),
            Err(StatError::SingularMatrix)
        ));
        // Validation.
        assert!(matches!(
            rlm(&[&x[..]], &y, &[1.0, 1.0], &RlmOptions::default()),
            Err(StatError::LengthMismatch { .. })
        ));
        let mut neg_w = vec![1.0; y.len()];
        neg_w[5] = -1.0;
        assert!(matches!(
            rlm(&[&x[..]], &y, &neg_w, &RlmOptions::default()),
            Err(StatError::InvalidWeights)
        ));
        let bad_y: Vec<f64> = y
            .iter()
            .enumerate()
            .map(|(i, &v)| if i == 3 { f64::NAN } else { v })
            .collect();
        assert!(rlm(&[&x[..]], &bad_y, &w, &RlmOptions::default()).is_err());
    }
}
