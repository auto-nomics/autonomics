//! Negative-binomial (NB2) regression with a log link and a fixed
//! `log(exposure)` offset.
//!
//! This is the SAP's primary histology readout model (§10.5): patient-level
//! intratumoral CD8 counts modelled by negative-binomial regression with the
//! log of the evaluable tumor-nest area as offset, the cross-scale score as
//! a continuous covariate, and per-SD count rate ratios reported — counts
//! from variable-area ROIs are over-dispersed relative to Poisson and the
//! offset makes the response a density rather than a raw count.
//!
//! Estimation follows the classical NB2 scheme (cf. `MASS::glm.nb`):
//!
//! * for a fixed dispersion `α` the mean model `μ_i = exp(x_iᵀβ + o_i)` with
//!   variance `μ_i + α μ_i²` is a GLM with log link, fit here by IRLS with
//!   weights `w_i = μ_i / (1 + α μ_i)` and working response
//!   `z_i = η_i + (y_i − μ_i)/μ_i`;
//! * `α` is profiled by golden-section search on the full log-likelihood
//!   over `log α`, each evaluation refitting `β`.
//!
//! Standard errors and Wald intervals are conditional on the fitted `α`
//! (the usual convention for profiled NB2), from the weighted least-squares
//! covariance `(Xᵀ W X)⁻¹` at convergence.

use crate::error::{Result, StatError};
use statrs::function::gamma::ln_gamma;

/// Options for [`negbin`].
#[derive(Debug, Clone)]
pub struct NegbinOptions {
    /// Include an intercept as coefficient 0.
    pub intercept: bool,
    /// Dispersion `α`; `None` profiles it by golden-section search.
    /// `Some(0.0)` is rejected — use Poisson tooling for the limit case.
    pub alpha: Option<f64>,
    /// Maximum IRLS iterations per fit.
    pub max_iter: usize,
    /// IRLS convergence tolerance on the largest coefficient step.
    pub tol: f64,
    /// Golden-section iterations for profiling `log α`.
    pub profile_iter: usize,
    /// Bracket for the profiling search, as `(lo, hi)` on `log α`.
    pub log_alpha_bracket: (f64, f64),
}

impl Default for NegbinOptions {
    fn default() -> Self {
        Self {
            intercept: true,
            alpha: None,
            max_iter: 200,
            tol: 1e-10,
            profile_iter: 80,
            log_alpha_bracket: (-18.4, 2.3), // α ∈ [~1e-8, ~10]
        }
    }
}

/// Result of an NB2 fit.
#[derive(Debug, Clone)]
pub struct NegbinResult {
    /// Fitted coefficients, intercept first when requested.
    pub coefficients: Vec<f64>,
    /// Wald standard errors, conditional on `α`.
    pub std_errors: Vec<f64>,
    /// Rate ratios `exp(β)` with 95% Wald intervals on the ratio scale.
    pub rate_ratios: Vec<(f64, f64, f64)>, // (rr, lo, hi)
    /// Fitted dispersion `α` (the profiled or fixed value).
    pub alpha: f64,
    /// Full NB2 log-likelihood at the optimum.
    pub log_likelihood: f64,
    /// Number of observations.
    pub n: usize,
    /// Whether the IRLS loop met the tolerance.
    pub converged: bool,
    /// IRLS iterations at the reported fit.
    pub n_iter: usize,
}

/// Fit an NB2 regression of `y` on the columns of `x` with offset `o`.
///
/// * `y` — non-negative counts;
/// * `x` — parallel predictor columns, each of length `n`;
/// * `offset` — pre-specified linear predictor contribution, i.e. the
///   `log(exposure)` (log evaluable area for density readouts).
pub fn negbin(
    y: &[f64],
    x: &[&[f64]],
    offset: &[f64],
    opts: &NegbinOptions,
) -> Result<NegbinResult> {
    let n = y.len();
    if n == 0 {
        return Err(StatError::InvalidInput("y must be non-empty".to_string()));
    }
    for col in x {
        if col.len() != n {
            return Err(StatError::LengthMismatch { a: col.len(), b: n });
        }
    }
    if offset.len() != n {
        return Err(StatError::LengthMismatch {
            a: offset.len(),
            b: n,
        });
    }
    for (i, &v) in y.iter().enumerate() {
        if !v.is_finite() || v < 0.0 {
            return Err(StatError::InvalidInput(format!(
                "count at position {i} must be finite and non-negative"
            )));
        }
    }
    for (i, &o) in offset.iter().enumerate() {
        if !o.is_finite() {
            return Err(StatError::InvalidInput(format!(
                "offset at position {i} is not finite"
            )));
        }
    }
    if let Some(a) = opts.alpha {
        if !a.is_finite() || a <= 0.0 {
            return Err(StatError::InvalidInput(
                "alpha must be positive and finite (use Poisson tooling for the α → 0 limit)"
                    .to_string(),
            ));
        }
    }

    let p = x.len() + usize::from(opts.intercept);
    if n <= p {
        return Err(StatError::InsufficientData { min: p + 1, actual: n });
    }

    // Resolve α: fixed, or profiled over log α by golden section.
    let (alpha, fit) = match opts.alpha {
        Some(a) => (a, irls_fit(y, x, offset, a, opts)?),
        None => {
            let (lo, hi) = opts.log_alpha_bracket;
            let mut a = lo;
            let mut b = hi;
            let phi = (5.0_f64.sqrt() - 1.0) / 2.0;
            let mut c = b - phi * (b - a);
            let mut d = a + phi * (b - a);
            let mut fc = irls_fit(y, x, offset, c.exp(), opts)?.loglik;
            let mut fd = irls_fit(y, x, offset, d.exp(), opts)?.loglik;
            for _ in 0..opts.profile_iter {
                if fc > fd {
                    b = d;
                    d = c;
                    fd = fc;
                    c = b - phi * (b - a);
                    fc = irls_fit(y, x, offset, c.exp(), opts)?.loglik;
                } else {
                    a = c;
                    c = d;
                    fc = fd;
                    d = a + phi * (b - a);
                    fd = irls_fit(y, x, offset, d.exp(), opts)?.loglik;
                }
            }
            let log_alpha = (a + b) / 2.0;
            let fit = irls_fit(y, x, offset, log_alpha.exp(), opts)?;
            (log_alpha.exp(), fit)
        }
    };

    let se = covariance_se(&fit.xtwx, p)?;
    let mut rate_ratios = Vec::with_capacity(p);
    let z = 1.959963984540054_f64; // Φ^{-1}(0.975)
    for j in 0..p {
        let rr = fit.beta[j].exp();
        let lo = ((fit.beta[j] - z * se[j]) as f64).exp();
        let hi = ((fit.beta[j] + z * se[j]) as f64).exp();
        rate_ratios.push((rr, lo, hi));
    }

    Ok(NegbinResult {
        coefficients: fit.beta,
        std_errors: se,
        rate_ratios,
        alpha,
        log_likelihood: fit.loglik,
        n,
        converged: fit.converged,
        n_iter: fit.n_iter,
    })
}

/// One IRLS fit at fixed `α`; also returns `Xᵀ W X` for standard errors.
struct FixedAlphaFit {
    beta: Vec<f64>,
    xtwx: Vec<Vec<f64>>,
    loglik: f64,
    converged: bool,
    n_iter: usize,
}

fn irls_fit(
    y: &[f64],
    x: &[&[f64]],
    offset: &[f64],
    alpha: f64,
    opts: &NegbinOptions,
) -> Result<FixedAlphaFit> {
    let n = y.len();
    let p = x.len() + usize::from(opts.intercept);

    // Design matrix rows.
    let row_i = |i: usize| -> Vec<f64> {
        let mut r = Vec::with_capacity(p);
        if opts.intercept {
            r.push(1.0);
        }
        for col in x {
            r.push(col[i]);
        }
        r
    };

    // Cold start: intercept-only rate, offset included.
    let sum_y: f64 = y.iter().sum();
    let sum_eo: f64 = offset.iter().map(|o| o.exp()).sum();
    let beta0 = (sum_y / sum_eo.max(1e-12)).ln();
    let mut beta = vec![0.0_f64; p];
    if opts.intercept {
        beta[0] = beta0;
    }

    let mut w = vec![0.0_f64; n];
    let mut z = vec![0.0_f64; n];
    let mut converged = false;
    let mut n_iter = 0usize;

    for it in 0..opts.max_iter {
        n_iter = it + 1;
        // μ and IRLS weights/working response.
        for i in 0..n {
            let row = row_i(i);
            let mut eta = offset[i];
            for j in 0..p {
                eta += row[j] * beta[j];
            }
            let eta = eta.clamp(-30.0, 30.0);
            let mu = eta.exp();
            if !mu.is_finite() || mu <= 0.0 {
                return Err(StatError::Numerical(
                    "mean collapsed to zero or overflowed during IRLS".to_string(),
                ));
            }
            w[i] = mu / (1.0 + alpha * mu);
            // Working target for the regression on X only: the offset is a
            // fixed part of η, so it must not be regressed — subtract it.
            z[i] = (eta - offset[i]) + (y[i] - mu) / mu;
        }

        // Weighted normal equations (Xᵀ W X) β = Xᵀ W z via Cholesky on a
        // small p × p system, solved in plain f64.
        let mut xtwx = vec![vec![0.0_f64; p]; p];
        let mut xtwz = vec![0.0_f64; p];
        for i in 0..n {
            let row = row_i(i);
            for a in 0..p {
                xtwz[a] += w[i] * row[a] * z[i];
                for b in 0..p {
                    xtwx[a][b] += w[i] * row[a] * row[b];
                }
            }
        }
        let new_beta = solve_symmetric(&xtwx, &xtwz)?;

        let step = new_beta
            .iter()
            .zip(beta.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f64, f64::max);
        beta = new_beta;
        if step < opts.tol {
            converged = true;
            break;
        }
    }

    // Log-likelihood at the solution (NB2).
    let mut loglik = 0.0_f64;
    let r = ln_gamma(1.0 / alpha);
    for i in 0..n {
        let row = row_i(i);
        let mut eta = offset[i];
        for j in 0..p {
            eta += row[j] * beta[j];
        }
        let mu = eta.clamp(-30.0, 30.0).exp();
        let t = 1.0 / alpha;
        loglik += y[i] * (alpha * mu).ln()
            - (y[i] + t) * (1.0 + alpha * mu).ln()
            + ln_gamma(y[i] + t)
            - r
            - ln_gamma(y[i] + 1.0);
    }

    // Recompute Xᵀ W X at the solution for standard errors.
    let mut xtwx = vec![vec![0.0_f64; p]; p];
    for i in 0..n {
        let row = row_i(i);
        let mut eta = offset[i];
        for j in 0..p {
            eta += row[j] * beta[j];
        }
        let mu = eta.clamp(-30.0, 30.0).exp();
        let wi = mu / (1.0 + alpha * mu);
        for a in 0..p {
            for b in 0..p {
                xtwx[a][b] += wi * row[a] * row[b];
            }
        }
    }

    Ok(FixedAlphaFit {
        beta,
        xtwx,
        loglik,
        converged,
        n_iter,
    })
}

/// Standard errors as `sqrt(diag((Xᵀ W X)⁻¹))`.
fn covariance_se(xtwx: &[Vec<f64>], p: usize) -> Result<Vec<f64>> {
    let inv = invert_symmetric(xtwx, p)?;
    Ok((0..p).map(|j| inv[j][j].max(0.0).sqrt()).collect())
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
                if s <= 0.0 {
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
    // Factor once.
    let mut l = vec![vec![0.0_f64; n]; n];
    for i in 0..n {
        for j in 0..=i {
            let mut s = a[i][j];
            for k in 0..j {
                s -= l[i][k] * l[j][k];
            }
            if i == j {
                if s <= 0.0 {
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
        // Forward: L u = e.
        let mut u = vec![0.0_f64; n];
        for i in 0..n {
            let mut s = e[i];
            for k in 0..i {
                s -= l[i][k] * u[k];
            }
            u[i] = s / l[i][i];
        }
        // Back: Lᵀ v = u.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_shift_leaves_rates_invariant() {
        let y = [12.0, 9.0, 20.0, 7.0, 15.0, 11.0];
        let x = [&[0.0, 0.0, 1.0, 1.0, 1.0, 1.0][..]];
        let area = [2.0_f64, 3.0, 2.5, 3.5, 2.2, 2.8];
        let o1: Vec<f64> = area.iter().map(|&a| a.ln()).collect();
        let o2: Vec<f64> = o1.iter().map(|o| o + 1.7).collect();
        let r1 = negbin(&y, &x, &o1, &NegbinOptions::default()).unwrap();
        let r2 = negbin(&y, &x, &o2, &NegbinOptions::default()).unwrap();
        // A constant added to log-offset shifts only the intercept.
        assert!((r1.coefficients[1] - r2.coefficients[1]).abs() < 1e-8);
        assert!((r1.coefficients[0] - (r2.coefficients[0] + 1.7)).abs() < 1e-6);
    }

    #[test]
    fn poisson_limit_recovers_two_group_rate_ratio() {
        // With α pinned near zero the NB2 MLE matches the closed-form
        // Poisson rate ratio (Σy₁/E₁)/(Σy₀/E₀) = (11/50)/(5/25) = 1.1.
        let y = [2.0, 3.0, 4.0, 7.0];
        let x = [&[0.0, 0.0, 1.0, 1.0][..]];
        let e = [10.0_f64, 15.0, 20.0, 30.0];
        let offset: Vec<f64> = e.iter().map(|&v| v.ln()).collect();
        let r = negbin(
            &y,
            &x,
            &offset,
            &NegbinOptions {
                alpha: Some(1e-8),
                ..NegbinOptions::default()
            },
        )
        .unwrap();
        assert!((r.coefficients[1] - 1.1_f64.ln()).abs() < 1e-4);
    }

    #[test]
    fn dispersion_widens_intervals() {
        // Same data fit at small and large fixed α: Wald intervals for the
        // rate ratio must widen with dispersion.
        let y = [12.0, 9.0, 20.0, 7.0, 15.0, 11.0];
        let x = [&[0.0, 0.0, 1.0, 1.0, 1.0, 1.0][..]];
        let offset: Vec<f64> = [2.0_f64, 3.0, 2.5, 3.5, 2.2, 2.8]
            .iter()
            .map(|&a| a.ln())
            .collect();
        let small = negbin(
            &y,
            &x,
            &offset,
            &NegbinOptions {
                alpha: Some(1e-6),
                ..NegbinOptions::default()
            },
        )
        .unwrap();
        let big = negbin(
            &y,
            &x,
            &offset,
            &NegbinOptions {
                alpha: Some(1.0),
                ..NegbinOptions::default()
            },
        )
        .unwrap();
        let w_small = small.rate_ratios[1].2 - small.rate_ratios[1].1;
        let w_big = big.rate_ratios[1].2 - big.rate_ratios[1].1;
        assert!(w_big > w_small);
    }

    #[test]
    fn profiled_alpha_recovers_overdispersion() {
        // Data with strong extra-Poisson scatter: profiled α > 0.
        let y = [30.0, 5.0, 25.0, 2.0, 28.0, 4.0, 22.0, 6.0];
        let x = [&[0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0][..]];
        let offset: Vec<f64> = [2.0_f64.ln(); 8].to_vec();
        let r = negbin(&y, &x, &offset, &NegbinOptions::default()).unwrap();
        assert!(r.alpha > 0.05, "profiled alpha = {}", r.alpha);
        assert!(r.converged);
    }

    #[test]
    fn rejects_bad_inputs() {
        let y = [1.0, 2.0];
        let x = [&[0.0, 1.0][..]];
        let o = [0.0_f64, 0.0];
        assert!(negbin(&y, &x, &o, &NegbinOptions { alpha: Some(0.0), ..NegbinOptions::default() }).is_err());
        assert!(negbin(&[-1.0, 2.0], &x, &o, &NegbinOptions::default()).is_err());
        let bad_o = [0.0_f64, f64::NAN];
        assert!(negbin(&y, &x, &bad_o, &NegbinOptions::default()).is_err());
    }
}
