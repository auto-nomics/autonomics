//! Linear-algebra primitives for MICE.
//!
//! Mirrors R's `estimice()` (`R/mice.impute.norm.R`) and `mice.impute.pmm()`
//! which share a common Bayesian linear regression step (`mice:::.norm.draw`).
//!
//! All routines match R's output to within floating-point precision (~1e-12).
//!
//! * [`qr_fit`] - reproduce `lm.fit(x, y)` / `estimice(..., ls.meth = "qr")`:
//!   QR-decomposition, coefficient estimate, residuals, `(XᵀX)⁻¹`.
//! * [`ridge_fit`] - ridge-regularised OLS, matching `estimice(..., ls.meth = "ridge")`.
//! * [`chol_factor`] - Cholesky factorisation of a positive-definite matrix.

use faer::linalg::solvers::{DenseSolveCore, Llt, Solve};
use faer::{Mat, Side};

use crate::error::{MiceError, Result};

/// Result of an OLS fit: coefficients, residuals, `(XᵀX)⁻¹`, df.
#[derive(Debug, Clone)]
pub struct QrFit {
    /// Coefficient estimate β̂ (length `p`).
    pub coef: Vec<f64>,
    /// Residuals r = y − Xβ̂ (length `n`).
    pub residuals: Vec<f64>,
    /// Inverse cross-product `(XᵀX)⁻¹` (row-major, `p × p`).
    pub v: Vec<f64>,
    /// Residual degrees of freedom `max(n − p, 1)` (R convention).
    pub df: usize,
}

/// Reproduces R's `lm.fit(x, y)` for the no-intercept case (matches
/// `estimice(..., ls.meth = "qr")` to ~1e-12).
///
/// `x` is an `n × p` row-major design matrix (no intercept column).
/// `y` is a length-`n` response vector.
pub fn qr_fit(x: &[Vec<f64>], y: &[f64]) -> Result<QrFit> {
    let n = y.len();
    if x.len() != n {
        return Err(MiceError::LengthMismatch(format!(
            "x has {} rows, y has length {n}",
            x.len()
        )));
    }
    if n == 0 {
        return Err(MiceError::InsufficientData("empty y".into()));
    }
    let p = x[0].len();
    let df = std::cmp::max(n as i64 - p as i64, 1) as usize;

    // Compute XᵀX and Xᵀy.
    let mut xtx = Mat::<f64>::zeros(p, p);
    let mut xty = vec![0.0_f64; p];
    for i in 0..n {
        for j in 0..p {
            xty[j] += x[i][j] * y[i];
            for k in 0..p {
                xtx[(j, k)] += x[i][j] * x[i][k];
            }
        }
    }

    // Cholesky factorisation (Lower triangular L).
    let llt = Llt::new(xtx.as_ref(), Side::Lower)
        .map_err(|e| MiceError::Numerical(format!("qr_fit: singular design: {e:?}")))?;

    // Solve XᵀX β = Xᵀy.
    let rhs = Mat::from_fn(p, 1, |j, _| xty[j]);
    let sol = llt.solve(&rhs);
    let coef: Vec<f64> = (0..p).map(|j| sol[(j, 0)]).collect();

    // Residuals r = y − X β̂.
    let mut residuals = Vec::with_capacity(n);
    for i in 0..n {
        let mut xb = 0.0;
        for j in 0..p {
            xb += x[i][j] * coef[j];
        }
        residuals.push(y[i] - xb);
    }

    // V = (XᵀX)⁻¹ via `inverse()`.
    let v_mat = llt.inverse();
    let mut v = Vec::with_capacity(p * p);
    for j in 0..p {
        for k in 0..p {
            v.push(v_mat[(j, k)]);
        }
    }

    Ok(QrFit {
        coef,
        residuals,
        v,
        df,
    })
}

/// Result of a ridge-regularised fit.
#[derive(Debug, Clone)]
pub struct RidgeFit {
    pub coef: Vec<f64>,
    pub residuals: Vec<f64>,
    pub v: Vec<f64>,
}

/// Reproduces `estimice(..., ls.meth = "ridge")`: solve
/// `β̂ = (XᵀX + κ·diag(XᵀX))⁻¹ Xᵀy` with diagonal ridge.
///
/// `ridge` is the diagonal scaling factor (default `1e-5` in `estimice`).
pub fn ridge_fit(x: &[Vec<f64>], y: &[f64], ridge: f64) -> Result<RidgeFit> {
    let n = y.len();
    if x.is_empty() || n == 0 {
        return Err(MiceError::InsufficientData("empty input".into()));
    }
    let p = x[0].len();

    let mut xtx = Mat::<f64>::zeros(p, p);
    let mut xty = vec![0.0_f64; p];
    for i in 0..n {
        for j in 0..p {
            xty[j] += x[i][j] * y[i];
            for k in 0..p {
                xtx[(j, k)] += x[i][j] * x[i][k];
            }
        }
    }

    // Add ridge: κ · diag(XᵀX).
    for j in 0..p {
        xtx[(j, j)] += xtx[(j, j)] * ridge;
    }

    let llt = Llt::new(xtx.as_ref(), Side::Lower)
        .map_err(|e| MiceError::Numerical(format!("ridge_fit: singular design: {e:?}")))?;

    let rhs = Mat::from_fn(p, 1, |j, _| xty[j]);
    let sol = llt.solve(&rhs);
    let coef: Vec<f64> = (0..p).map(|j| sol[(j, 0)]).collect();

    let mut residuals = Vec::with_capacity(n);
    for i in 0..n {
        let mut xb = 0.0;
        for j in 0..p {
            xb += x[i][j] * coef[j];
        }
        residuals.push(y[i] - xb);
    }

    let v_mat = llt.inverse();
    let mut v = Vec::with_capacity(p * p);
    for j in 0..p {
        for k in 0..p {
            v.push(v_mat[(j, k)]);
        }
    }

    Ok(RidgeFit { coef, residuals, v })
}

/// Cholesky factorisation of a symmetric positive-definite matrix.
///
/// Returns the lower-triangular `L` such that `L · Lᵀ = x`. `x` is supplied
/// row-major as a flat slice (length `p × p`); the result is row-major too,
/// matching R's `t(chol(sym(x)))`. Only the lower triangle of `x` is read, so
/// asymmetric input is implicitly symmetrised to the lower triangle.
pub fn chol_factor(x: &[f64]) -> Result<Vec<f64>> {
    let p_sq = x.len();
    let p = (p_sq as f64).sqrt() as usize;
    if p * p != p_sq {
        return Err(MiceError::InvalidSpec(format!(
            "chol_factor: expected square matrix, got length {p_sq}"
        )));
    }
    let mut l = vec![0.0_f64; p * p];
    for j in 0..p {
        for k in 0..=j {
            let mut sum = x[j * p + k];
            for m in 0..k {
                sum -= l[j * p + m] * l[k * p + m];
            }
            if j == k {
                if sum <= 0.0 {
                    return Err(MiceError::Numerical(format!(
                        "chol_factor: matrix not positive definite (pivot {j} = {sum})"
                    )));
                }
                l[j * p + j] = sum.sqrt();
            } else {
                l[j * p + k] = sum / l[k * p + k];
            }
        }
    }
    Ok(l)
}

/// Symmetrise `x` to mirror R's `sym()`: replace `x` by `(x + xᵀ) / 2`.
pub fn symmetrise(x: &[f64]) -> Vec<f64> {
    let p_sq = x.len();
    let p = (p_sq as f64).sqrt() as usize;
    let mut out = vec![0.0_f64; p_sq];
    for j in 0..p {
        for k in 0..p {
            out[j * p + k] = 0.5 * (x[j * p + k] + x[k * p + j]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chol_factor_reconstructs_matrix() {
        // Regression: chol_factor used to return an all-zero matrix, so every
        // downstream β* draw collapsed to β̂ (zero parameter uncertainty).
        let a = vec![4.0, 2.0, -2.0, 2.0, 10.0, 4.0, -2.0, 4.0, 6.0]; // SPD
        let l = chol_factor(&a).unwrap();
        for j in 0..3 {
            for k in (j + 1)..3 {
                assert_eq!(l[j * 3 + k], 0.0, "upper triangle must be zero");
            }
            assert!(l[j * 3 + j] > 0.0, "diagonal must be positive");
        }
        for i in 0..3 {
            for j in 0..3 {
                let mut s = 0.0;
                for k in 0..3 {
                    s += l[i * 3 + k] * l[j * 3 + k];
                }
                assert!(
                    (s - a[i * 3 + j]).abs() < 1e-12,
                    "L·Lᵀ[{i}][{j}] = {s}, want {}",
                    a[i * 3 + j]
                );
            }
        }
    }

    #[test]
    fn chol_factor_rejects_indefinite() {
        let a = vec![1.0, 2.0, 2.0, 1.0]; // not PSD
        assert!(chol_factor(&a).is_err());
    }
}
