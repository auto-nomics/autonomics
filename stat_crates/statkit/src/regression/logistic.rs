//! Logistic regression for binary outcomes via IRLS (Newton-Raphson).
//!
//! Each iteration solves the weighted normal equations
//! `(Xᵀ W X) Δ = Xᵀ (y − μ)` using faer's Cholesky factorisation, where
//! `μ = sigmoid(Xβ)` and `W = diag(μ(1−μ))`. The update `β ← β + Δ` is
//! repeated until `max|Δᵢ| < tol` or `max_iter` is reached.
//!
//! Inference is Wald-based: `z = β / SE` with `SE = √diag((XᵀWX)⁻¹)`,
//! two-sided p-values from the standard Normal, odds ratios `exp(β)` with
//! 95% CIs `exp(β ± 1.96·SE)`. The log-likelihood at convergence is also
//! returned for downstream likelihood-ratio tests (e.g. RCS nonlinearity).

use faer::linalg::solvers::{DenseSolveCore, Llt, Solve};
use faer::{Mat, Side};
use statrs::distribution::{ContinuousCDF, Normal};

use crate::error::{Result, StatError};

/// Maximum IRLS iterations.
const MAX_ITER: usize = 50;
/// Convergence threshold on the max coefficient change.
const TOL: f64 = 1.0e-8;
/// Clamp probabilities to avoid log(0) / infinite weights under separation.
const MU_EPS: f64 = 1.0e-10;
/// z-value for a two-sided 95% confidence interval.
const Z_975: f64 = 1.959963984540054;

/// Result of a logistic regression fit.
#[derive(Debug, Clone)]
pub struct LogisticResult {
    /// Estimated coefficients; index 0 is the intercept when fit with one.
    pub coefficients: Vec<f64>,
    /// Standard error of each coefficient (√ diagonal of (XᵀWX)⁻¹).
    pub std_errors: Vec<f64>,
    /// Wald z-statistic `β / SE`.
    pub z_stats: Vec<f64>,
    /// Two-sided p-value from the standard Normal.
    pub p_values: Vec<f64>,
    /// Odds ratio `exp(β)`.
    pub odds_ratios: Vec<f64>,
    /// Lower bound of the 95% CI for the odds ratio.
    pub or_ci_lower: Vec<f64>,
    /// Upper bound of the 95% CI for the odds ratio.
    pub or_ci_upper: Vec<f64>,
    /// Fitted probabilities `μ = sigmoid(Xβ)`.
    pub fitted: Vec<f64>,
    /// Log-likelihood at convergence: `Σ [y log μ + (1−y) log(1−μ)]`.
    pub log_likelihood: f64,
    /// Log-likelihood of the intercept-only (null) model.
    pub null_log_likelihood: f64,
    /// McFadden pseudo-R²: `1 − LL / LL₀`.
    pub pseudo_r_squared: f64,
    /// Number of observations.
    pub n_obs: usize,
    /// Number of estimated parameters.
    pub n_params: usize,
    /// Whether IRLS converged within `max_iter`.
    pub converged: bool,
    /// Number of iterations actually performed.
    pub n_iter: usize,
}

/// Fit a binary logistic regression via IRLS.
///
/// `y` must contain only 0.0 and 1.0 values. `predictors` are parallel
/// numeric columns each of length `y.len()`. When `intercept = true` a
/// column of ones is prepended (so `coefficients[0]` is the intercept).
pub fn logistic(predictors: &[&[f64]], y: &[f64], intercept: bool) -> Result<LogisticResult> {
    let n = y.len();
    if n == 0 {
        return Err(StatError::EmptyInput);
    }
    // Validate binary outcome.
    for &v in y {
        if v != 0.0 && v != 1.0 {
            return Err(StatError::Numerical(format!(
                "logistic outcome must be binary (0/1), got {v}"
            )));
        }
    }

    // Build design columns: optional intercept then each predictor.
    let mut cols: Vec<Vec<f64>> = Vec::with_capacity(predictors.len() + intercept as usize);
    if intercept {
        cols.push(vec![1.0; n]);
    }
    for p in predictors {
        if p.len() != n {
            return Err(StatError::LengthMismatch { a: n, b: p.len() });
        }
        cols.push(p.to_vec());
    }
    let p = cols.len();

    // Null (intercept-only) log-likelihood.
    let n1: f64 = y.iter().map(|&v| v).sum();
    let n0 = n as f64 - n1;
    let p_bar = n1 / n as f64;
    let null_ll = if p_bar > 0.0 && p_bar < 1.0 {
        n1 * p_bar.ln() + n0 * (1.0 - p_bar).ln()
    } else {
        0.0
    };

    // IRLS — start from β = 0 (R's glm default).
    let mut beta = vec![0.0_f64; p];
    let mut mu: Vec<f64> = vec![0.5; n]; // sigmoid(0) = 0.5
    let mut converged = false;
    let mut n_iter = 0;

    for iter in 0..MAX_ITER {
        n_iter = iter + 1;

        // Current log-likelihood (for step-halving).
        let ll_current =
            compensated_sum((0..n).map(|k| y[k] * mu[k].ln() + (1.0 - y[k]) * (1.0 - mu[k]).ln()));

        // Working weights w_i = μ_i (1 − μ_i), gradient g = Xᵀ(y − μ).
        let mut xtwx = vec![vec![0.0; p]; p];
        let mut grad = vec![0.0; p];
        for i in 0..p {
            for j in i..p {
                let s = compensated_sum((0..n).map(|k| {
                    let w = mu[k] * (1.0 - mu[k]);
                    w * cols[i][k] * cols[j][k]
                }));
                xtwx[i][j] = s;
                xtwx[j][i] = s;
            }
            grad[i] = compensated_sum((0..n).map(|k| cols[i][k] * (y[k] - mu[k])));
        }

        // Solve (XᵀWX) Δ = gradient for the Newton update.
        let a = Mat::from_fn(p, p, |i, j| xtwx[i][j]);
        let g = Mat::from_fn(p, 1, |i, _| grad[i]);
        let llt = Llt::new(a.as_ref(), Side::Lower)
            .ok()
            .ok_or(StatError::SingularMatrix)?;
        let delta_mat = llt.solve(&g);
        let delta: Vec<f64> = (0..p).map(|i| delta_mat[(i, 0)]).collect();

        // Step-halving: if the full Newton step worsens the log-likelihood
        // (e.g. near separation where XtWX is ill-conditioned), halve the
        // step until the log-likelihood improves or the step is negligible.
        let mut step = 1.0_f64;
        loop {
            let beta_trial: Vec<f64> = (0..p).map(|i| beta[i] + delta[i] * step).collect();
            let mu_trial: Vec<f64> = (0..n)
                .map(|k| {
                    let eta = compensated_sum((0..p).map(|j| beta_trial[j] * cols[j][k]));
                    sigmoid(eta).clamp(MU_EPS, 1.0 - MU_EPS)
                })
                .collect();
            let ll_trial = compensated_sum(
                (0..n).map(|k| y[k] * mu_trial[k].ln() + (1.0 - y[k]) * (1.0 - mu_trial[k]).ln()),
            );
            if ll_trial >= ll_current || step < 1e-6 {
                beta = beta_trial;
                mu = mu_trial;
                break;
            }
            step *= 0.5;
        }

        let max_delta = delta.iter().map(|d| (d * step).abs()).fold(0.0, f64::max);

        // Convergence: R's `glm` uses relative deviance change.
        // Deviance D = −2·LL, so |ΔD|/(|D|+0.1) = 2|ΔLL|/(2|LL|+0.1).
        let ll_new =
            compensated_sum((0..n).map(|k| y[k] * mu[k].ln() + (1.0 - y[k]) * (1.0 - mu[k]).ln()));
        let dev_change = 2.0 * (ll_new - ll_current).abs();
        let dev_scale = 2.0 * ll_new.abs() + 0.1;
        if dev_change / dev_scale < TOL {
            converged = true;
            break;
        }
        let _ = max_delta;

        // Update μ = sigmoid(Xβ), clamped.
        if max_delta < TOL {
            converged = true;
            break;
        }
    }

    // Final (XᵀWX)⁻¹ for standard errors.
    let mut xtwx = vec![vec![0.0; p]; p];
    for i in 0..p {
        for j in i..p {
            let s = compensated_sum((0..n).map(|k| {
                let w = mu[k] * (1.0 - mu[k]);
                w * cols[i][k] * cols[j][k]
            }));
            xtwx[i][j] = s;
            xtwx[j][i] = s;
        }
    }
    let a = Mat::from_fn(p, p, |i, j| xtwx[i][j]);
    let llt = Llt::new(a.as_ref(), Side::Lower)
        .ok()
        .ok_or(StatError::SingularMatrix)?;
    let inv_mat = llt.inverse();

    // Log-likelihood at convergence.
    let ll = compensated_sum((0..n).map(|k| y[k] * mu[k].ln() + (1.0 - y[k]) * (1.0 - mu[k]).ln()));

    // Wald inference.
    let normal = Normal::new(0.0, 1.0).map_err(|e| StatError::Numerical(format!("Normal: {e}")))?;
    let std_errors: Vec<f64> = (0..p).map(|i| inv_mat[(i, i)].max(0.0).sqrt()).collect();
    let z_stats: Vec<f64> = (0..p)
        .map(|i| {
            if std_errors[i] > 0.0 {
                beta[i] / std_errors[i]
            } else {
                f64::NAN
            }
        })
        .collect();
    let p_values: Vec<f64> = (0..p)
        .map(|i| {
            let z = z_stats[i].abs();
            if z.is_finite() {
                2.0 * normal.sf(z)
            } else {
                f64::NAN
            }
        })
        .collect();
    let odds_ratios: Vec<f64> = beta.iter().map(|&b| b.exp()).collect();
    let or_ci_lower: Vec<f64> = (0..p)
        .map(|i| (beta[i] - Z_975 * std_errors[i]).exp())
        .collect();
    let or_ci_upper: Vec<f64> = (0..p)
        .map(|i| (beta[i] + Z_975 * std_errors[i]).exp())
        .collect();
    let pseudo_r2 = if null_ll < 0.0 {
        1.0 - ll / null_ll
    } else {
        f64::NAN
    };

    Ok(LogisticResult {
        coefficients: beta.clone(),
        std_errors,
        z_stats,
        p_values,
        odds_ratios,
        or_ci_lower,
        or_ci_upper,
        fitted: mu,
        log_likelihood: ll,
        null_log_likelihood: null_ll,
        pseudo_r_squared: pseudo_r2,
        n_obs: n,
        n_params: p,
        converged,
        n_iter,
    })
}

/// Logistic sigmoid: `1 / (1 + e⁻ˣ)`. Numerically stable for large |x|.
fn sigmoid(x: f64) -> f64 {
    if x >= 0.0 {
        1.0 / (1.0 + (-x).exp())
    } else {
        let e = x.exp();
        e / (1.0 + e)
    }
}

/// Compensated (Neumaier) summation — shared with [`super::ols`].
fn compensated_sum(iter: impl IntoIterator<Item = f64>) -> f64 {
    let mut sum = 0.0_f64;
    let mut c = 0.0_f64;
    for value in iter {
        let t = sum + value;
        if sum.abs() > value.abs() {
            c += (sum - t) + value;
        } else {
            c += (value - t) + sum;
        }
        sum = t;
    }
    sum + c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * b.abs().max(1.0)
    }

    #[test]
    fn perfect_separation_intercept_only() {
        // All y = 0 → intercept → −∞. Should still converge (μ clamped).
        let y = vec![0.0; 10];
        let res = logistic(&[], &y, true).unwrap();
        assert!(res.coefficients[0] < 0.0);
    }

    #[test]
    fn balanced_half_half() {
        // y = [0,0,0,0,0,1,1,1,1,1], no predictors, intercept only.
        // μ̂ = 0.5, β₀ = log(0.5/0.5) = 0.
        let y = vec![0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0];
        let res = logistic(&[], &y, true).unwrap();
        assert!(res.converged);
        assert!(approx_eq(res.coefficients[0], 0.0, 1e-6));
        assert!(approx_eq(res.fitted[0], 0.5, 1e-6));
    }

    #[test]
    fn simple_predictor_recovers_direction() {
        // x strongly positively associated with y=1 → β₁ > 0.
        let x = [0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0];
        let y = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0];
        let res = logistic(&[&x[..]], &y, true).unwrap();
        assert!(res.converged);
        assert!(res.coefficients[1] > 0.0, "slope should be positive");
    }

    #[test]
    fn odds_ratio_exponential_of_coefficient() {
        let x = [0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0];
        let y = vec![0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0];
        let res = logistic(&[&x[..]], &y, true).unwrap();
        for i in 0..res.n_params {
            assert!(approx_eq(
                res.odds_ratios[i],
                res.coefficients[i].exp(),
                1e-9
            ));
        }
    }

    #[test]
    fn p_values_in_range() {
        let x = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        let y = vec![0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0];
        let res = logistic(&[&x[..]], &y, true).unwrap();
        for &p in &res.p_values {
            assert!(p.is_finite() && (0.0..=1.0).contains(&p), "p={p}");
        }
    }

    #[test]
    fn log_likelihood_non_positive() {
        let x = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0];
        let y = vec![0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0];
        let res = logistic(&[&x[..]], &y, true).unwrap();
        assert!(res.log_likelihood <= 0.0);
        assert!(res.null_log_likelihood <= 0.0);
        assert!(res.pseudo_r_squared >= 0.0);
    }

    #[test]
    fn rejects_non_binary_outcome() {
        let x = [0.0, 1.0, 2.0];
        let y = vec![0.0, 0.5, 1.0];
        let res = logistic(&[&x[..]], &y, true);
        assert!(res.is_err());
    }

    #[test]
    fn sigmoid_stability() {
        assert!(approx_eq(sigmoid(0.0), 0.5, 1e-15));
        assert!(sigmoid(100.0) > 0.999999);
        assert!(sigmoid(-100.0) < 1e-43);
        assert!(sigmoid(-100.0) >= 0.0);
    }
}
