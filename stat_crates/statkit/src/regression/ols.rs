//! Ordinary and weighted least-squares regression via faer + statrs.
//!
//! Solves the normal equations `(Xᵀ W X) β = Xᵀ W y` using faer's Cholesky
//! (LLᵀ) factorisation — the Gram matrix `Xᵀ W X` is symmetric positive
//! definite whenever the design has full column rank. Coefficient standard
//! errors come from `σ̂² · (Xᵀ W X)⁻¹`, and two-sided p-values from the
//! Student-t survival function (statrs).
//!
//! Replaces the hand-written Gauss-Jordan + betai implementation that lived
//! in the original `stat-primitives` crate and was temporarily inlined into
//! `data-engine/src/nodes/linear_regression.rs`.

use faer::linalg::solvers::{DenseSolveCore, Llt, Solve};
use faer::{Mat, Side};
use statrs::distribution::{ContinuousCDF, StudentsT};

use crate::error::{Result, StatError};

/// Result of an OLS or WLS fit.
#[derive(Debug, Clone, PartialEq)]
pub struct Regression {
    /// Estimated coefficients; index 0 is the intercept when fit with one.
    pub coefficients: Vec<f64>,
    /// Standard error of each coefficient.
    pub std_errors: Vec<f64>,
    /// `t = coefficient / std_error`.
    pub t_stats: Vec<f64>,
    /// Two-sided p-value from Student-t with `df_residual` d.o.f.
    pub p_values: Vec<f64>,
    /// Fitted values `ŷ = Xβ`.
    pub fitted: Vec<f64>,
    /// Residuals `y − ŷ`.
    pub residuals: Vec<f64>,
    /// Weighted residual sum of squares `Σ wᵢ (yᵢ − ŷᵢ)²`.
    pub rss: f64,
    /// Weighted total sum of squares (about the weighted mean if intercept).
    pub tss: f64,
    /// Coefficient of determination `1 − RSS/TSS`.
    pub r_squared: f64,
    /// Adjusted R².
    pub adj_r_squared: f64,
    /// Number of observations.
    pub n_obs: usize,
    /// Number of estimated parameters (predictors + intercept if fit).
    pub n_params: usize,
    /// Residual degrees of freedom `n_obs − n_params`.
    pub df_residual: usize,
}

/// Weighted least squares: regress `y` on `predictors` with observation
/// `weights`, optionally fitting an intercept.
pub fn wls(
    predictors: &[&[f64]],
    y: &[f64],
    weights: &[f64],
    intercept: bool,
) -> Result<Regression> {
    fit(predictors, y, weights, intercept)
}

/// Ordinary least squares: [`wls`](fn.wls.html) with unit weights.
pub fn ols(predictors: &[&[f64]], y: &[f64], intercept: bool) -> Result<Regression> {
    let unit: Vec<f64> = vec![1.0; y.len()];
    fit(predictors, y, &unit, intercept)
}

fn fit(predictors: &[&[f64]], y: &[f64], weights: &[f64], intercept: bool) -> Result<Regression> {
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
    if weights.iter().any(|&w| w < 0.0 || w.is_nan()) {
        return Err(StatError::InvalidWeights);
    }
    for p in predictors.iter() {
        if p.len() != n {
            return Err(StatError::LengthMismatch { a: n, b: p.len() });
        }
    }

    // Build design columns: optional intercept (all ones) then each predictor.
    let mut cols: Vec<Vec<f64>> = Vec::with_capacity(predictors.len() + intercept as usize);
    if intercept {
        cols.push(vec![1.0; n]);
    }
    for p in predictors {
        cols.push(p.to_vec());
    }
    let p = cols.len();
    let df_residual = n
        .checked_sub(p)
        .filter(|&d| d > 0)
        .ok_or(StatError::InsufficientData {
            min: p + 1,
            actual: n,
        })?;

    // Normal equations: XtWX (p×p) and XtWy (p).
    let mut xtwx = vec![vec![0.0; p]; p];
    let mut xtwy = vec![0.0; p];
    for i in 0..p {
        for j in i..p {
            let s = compensated_sum((0..n).map(|k| weights[k] * cols[i][k] * cols[j][k]));
            xtwx[i][j] = s;
            xtwx[j][i] = s;
        }
        xtwy[i] = compensated_sum((0..n).map(|k| weights[k] * cols[i][k] * y[k]));
    }

    // Solve via faer Cholesky (XtWX is SPD for full-rank designs).
    let a = Mat::from_fn(p, p, |i, j| xtwx[i][j]);
    let b = Mat::from_fn(p, 1, |i, _| xtwy[i]);
    let llt = Llt::new(a.as_ref(), Side::Lower)
        .ok()
        .ok_or(StatError::SingularMatrix)?;
    let beta_mat = llt.solve(&b);
    let inv_mat = llt.inverse();

    let coefficients: Vec<f64> = (0..p).map(|i| beta_mat[(i, 0)]).collect();

    // Fitted values and residuals.
    let mut fitted = vec![0.0; n];
    for k in 0..n {
        fitted[k] = compensated_sum((0..p).map(|j| coefficients[j] * cols[j][k]));
    }
    let residuals: Vec<f64> = (0..n).map(|k| y[k] - fitted[k]).collect();

    let rss = compensated_sum((0..n).map(|k| weights[k] * residuals[k] * residuals[k]));

    // Total SS about the weighted mean if intercept, else uncentered.
    let tss = if intercept {
        let wsum = compensated_sum(weights.iter().copied());
        let wy = compensated_sum((0..n).map(|k| weights[k] * y[k]));
        let mean = wy / wsum;
        compensated_sum((0..n).map(|k| weights[k] * (y[k] - mean) * (y[k] - mean)))
    } else {
        compensated_sum((0..n).map(|k| weights[k] * y[k] * y[k]))
    };

    let r_squared = if tss > 0.0 { 1.0 - rss / tss } else { f64::NAN };
    let adj_r_squared = 1.0 - (1.0 - r_squared) * (n as f64 - 1.0) / df_residual as f64;

    // Variance estimate and coefficient standard errors.
    let sigma2 = rss / df_residual as f64;
    let t_dist = StudentsT::new(0.0, 1.0, df_residual as f64)
        .map_err(|e| StatError::Numerical(format!("StudentsT: {e}")))?;
    let std_errors: Vec<f64> = (0..p)
        .map(|i| (sigma2 * inv_mat[(i, i)]).max(0.0).sqrt())
        .collect();
    let t_stats: Vec<f64> = (0..p)
        .map(|i| {
            if std_errors[i] > 0.0 {
                coefficients[i] / std_errors[i]
            } else {
                f64::NAN
            }
        })
        .collect();
    let p_values: Vec<f64> = (0..p)
        .map(|i| {
            let t = t_stats[i].abs();
            if t.is_finite() {
                2.0 * t_dist.sf(t)
            } else {
                f64::NAN
            }
        })
        .collect();

    Ok(Regression {
        coefficients,
        std_errors,
        t_stats,
        p_values,
        fitted,
        residuals,
        rss,
        tss,
        r_squared,
        adj_r_squared,
        n_obs: n,
        n_params: p,
        df_residual,
    })
}

/// Compensated (Neumaier / Kahan–Babuška) summation.
///
/// Limits catastrophic cancellation when adding many values of differing
/// magnitude — exactly the case in the XtWX / XtWy accumulations above.
fn compensated_sum(iter: impl IntoIterator<Item = f64>) -> f64 {
    let mut sum = 0.0_f64;
    let mut c = 0.0_f64; // compensation
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

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * b.abs().max(1.0)
    }

    #[test]
    fn simple_linear_regression_closed_form() {
        // x = [1,2,3,4,5], y = [2,4,5,4,5]: slope = 0.6, intercept = 2.2.
        let x = [1.0, 2.0, 3.0, 4.0, 5.0];
        let y = [2.0, 4.0, 5.0, 4.0, 5.0];
        let r = ols(&[&x[..]], &y, true).unwrap();
        assert!(approx_eq(r.coefficients[0], 2.2, 1e-9)); // intercept
        assert!(approx_eq(r.coefficients[1], 0.6, 1e-9)); // slope
        assert!(approx_eq(r.r_squared, 0.6, 1e-9));
    }

    #[test]
    fn through_origin_fit() {
        // y = 2x exactly, no intercept: β = Σxy/Σx² = 28/14 = 2.
        let x = [1.0, 2.0, 3.0];
        let y = [2.0, 4.0, 6.0];
        let r = ols(&[&x[..]], &y, false).unwrap();
        assert_eq!(r.n_params, 1);
        assert!(approx_eq(r.coefficients[0], 2.0, 1e-9));
    }

    #[test]
    fn wls_unit_weights_equals_ols() {
        let x = [1.0, 2.0, 3.0, 4.0, 5.0];
        let y = [3.0, 5.0, 7.0, 9.0, 11.0];
        let w = [1.0; 5];
        let a = ols(&[&x[..]], &y, true).unwrap();
        let b = wls(&[&x[..]], &y, &w, true).unwrap();
        assert_eq!(a.coefficients, b.coefficients);
    }

    #[test]
    fn perfect_fit_zero_rss() {
        let x = [1.0, 2.0, 3.0, 4.0];
        let y = [3.0, 5.0, 7.0, 9.0]; // y = 2x + 1
        let r = ols(&[&x[..]], &y, true).unwrap();
        assert!(approx_eq(r.coefficients[0], 1.0, 1e-9));
        assert!(approx_eq(r.coefficients[1], 2.0, 1e-9));
        assert!(r.rss.abs() < 1e-9);
        assert!(approx_eq(r.r_squared, 1.0, 1e-9));
    }

    #[test]
    fn multiple_regression_recovers_coefficients() {
        let x1 = [0.0, 1.0, 2.0, 3.0, 4.0];
        let x2 = [0.0, 1.0, 0.0, 1.0, 0.0];
        let y: Vec<f64> = (0..5).map(|i| 1.0 + 2.0 * x1[i] + 3.0 * x2[i]).collect();
        let r = ols(&[&x1[..], &x2[..]], &y, true).unwrap();
        assert!(approx_eq(r.coefficients[0], 1.0, 1e-9));
        assert!(approx_eq(r.coefficients[1], 2.0, 1e-9));
        assert!(approx_eq(r.coefficients[2], 3.0, 1e-9));
    }

    #[test]
    fn singular_design_errors() {
        let x = [1.0, 2.0, 3.0, 4.0];
        let y = [2.0, 4.0, 6.0, 8.0];
        let res = ols(&[&x[..], &x[..]], &y, true);
        assert!(matches!(res, Err(StatError::SingularMatrix)));
    }

    #[test]
    fn insufficient_observations_errors() {
        let x = [1.0, 2.0];
        let y = [3.0, 5.0];
        let res = ols(&[&x[..]], &y, true);
        assert!(matches!(res, Err(StatError::InsufficientData { .. })));
    }

    #[test]
    fn p_values_finite_and_in_range() {
        let x = [1.0, 2.0, 3.0, 4.0, 5.0];
        let y = [2.0, 4.0, 5.0, 4.0, 5.0];
        let r = ols(&[&x[..]], &y, true).unwrap();
        for &p in &r.p_values {
            assert!(p.is_finite() && (0.0..=1.0).contains(&p));
        }
    }

    #[test]
    fn validation_errors() {
        let x = [1.0, 2.0, 3.0];
        let y = [1.0, 2.0, 3.0];
        assert!(matches!(
            wls(&[&x[..]], &y, &[1.0, 1.0], false),
            Err(StatError::LengthMismatch { .. })
        ));
        assert!(matches!(
            wls(&[&x[..]], &y, &[1.0, -1.0, 1.0], false),
            Err(StatError::InvalidWeights)
        ));
    }
}
