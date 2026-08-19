//! Thin linear-algebra helpers over [`faer`], sized to what LDSC needs: weighted
//! least squares via the normal equations and a symmetric-positive-definite solve.
//!
//! faer 0.24 exposes the LLᵀ factorisation as [`Llt`]; bringing the
//! [`Solve`] / [`DenseSolveCore`] traits into scope gives `.solve()` /
//! `.inverse()`. We use [`Llt`] for the weighted normal-equations matrix
//! `Xᵀ diag(w) X`, which is symmetric positive-(semi)definite by construction.

use faer::linalg::solvers::{DenseSolveCore, Llt, Solve};
use faer::{Mat, MatRef, Side};

use crate::{LdscError, Result};

/// Build an owned `Mat<f64>` from row-major `rows` (`rows.len() == nrows`,
/// each row of length `ncols`).
pub fn build_mat_row_major(rows: &[Vec<f64>]) -> Mat<f64> {
    let nrows = rows.len();
    let ncols = rows.first().map_or(0, |r| r.len());
    Mat::from_fn(nrows, ncols, |i, j| rows[i][j])
}

/// Build an owned `Mat<f64>` from a column-major flat buffer.
pub fn build_mat_col_major(nrows: usize, ncols: usize, data: &[f64]) -> Mat<f64> {
    debug_assert_eq!(data.len(), nrows * ncols);
    Mat::from_fn(nrows, ncols, |i, j| data[j * nrows + i])
}

/// Solve `A x = b` for a symmetric positive-definite `A` (p×p), `b` length p.
/// Returns `x`. Errors if `A` is not positive-definite (faer's `LltError`).
pub fn solve_spd(a: MatRef<'_, f64>, b: &[f64]) -> Result<Vec<f64>> {
    let p = b.len();
    if a.nrows() != p || a.ncols() != p {
        return Err(LdscError::DimensionMismatch(format!(
            "solve_spd: A is {}×{} but b has length {p}",
            a.nrows(),
            a.ncols()
        )));
    }
    // b as a p×1 column matrix.
    let rhs = Mat::from_fn(p, 1, |i, _j| b[i]);
    let llt = Llt::new(a, Side::Lower)
        .map_err(|e| LdscError::Linalg(format!("LLᵀ factorisation failed: {e:?}")))?;
    let x = llt.solve(&rhs);
    Ok((0..p).map(|i| x[(i, 0)]).collect())
}

/// 2-norm condition number `σ_max / σ_min` (numpy's `np.linalg.cond` default).
pub fn cond_number(a: MatRef<'_, f64>) -> Result<f64> {
    let sv = a
        .singular_values()
        .map_err(|e| LdscError::Linalg(format!("SVD failed: {e:?}")))?;
    if sv.is_empty() {
        return Ok(f64::NAN);
    }
    let smax = sv[0];
    let smin = *sv.last().unwrap();
    if smin == 0.0 {
        Ok(f64::INFINITY)
    } else {
        Ok(smax / smin)
    }
}

/// Inverse of a symmetric positive-definite matrix (for the coefficient
/// covariance `(Xᵀ W X)⁻¹`).
pub fn inv_spd(a: MatRef<'_, f64>) -> Result<Mat<f64>> {
    let llt = Llt::new(a, Side::Lower)
        .map_err(|e| LdscError::Linalg(format!("LLᵀ factorisation failed: {e:?}")))?;
    Ok(llt.inverse())
}

/// Weighted least squares: minimise `Σ wᵢ (yᵢ − xᵢ β)²` by solving the normal
/// equations `(Xᵀ W X) β = Xᵀ W y`. Weights are normalised to sum to 1 first —
/// this is purely numerical (the scalar cancels in β) and mirrors LDSC's
/// `_weight` convention, which keeps the normal-equations matrix well-scaled.
///
/// `x` is `n×p`, `y` length `n`, `w` length `n`.
pub fn wls(x: MatRef<'_, f64>, y: &[f64], w: &[f64]) -> Result<Vec<f64>> {
    let n = y.len();
    let p = x.ncols();
    if x.nrows() != n || w.len() != n {
        return Err(LdscError::DimensionMismatch(
            "wls: dimension mismatch".into(),
        ));
    }
    let wsum: f64 = w.iter().sum();
    if !(wsum > 0.0) {
        let non_finite_weights = w.iter().filter(|value| !value.is_finite()).count();
        let non_positive_weights = w.iter().filter(|value| **value <= 0.0).count();
        let first_invalid_weight = w
            .iter()
            .position(|value| !value.is_finite() || *value <= 0.0);
        let non_finite_x = (0..n)
            .flat_map(|row| (0..p).map(move |col| x[(row, col)]))
            .filter(|value| !value.is_finite())
            .count();
        let first_non_finite_x = (0..n).find_map(|row| {
            (0..p)
                .find(|&col| !x[(row, col)].is_finite())
                .map(|col| (row, col))
        });
        let non_finite_y = y.iter().filter(|value| !value.is_finite()).count();
        let first_non_finite_y = y.iter().position(|value| !value.is_finite());

        tracing::error!(
            n_rows = n,
            n_columns = p,
            weight_sum = wsum,
            non_finite_weights,
            non_positive_weights,
            first_invalid_weight = ?first_invalid_weight,
            non_finite_design_values = non_finite_x,
            first_non_finite_design_value = ?first_non_finite_x,
            non_finite_response_values = non_finite_y,
            first_non_finite_response_value = ?first_non_finite_y,
            "WLS rejected invalid regression inputs or weights"
        );
        return Err(LdscError::Linalg(format!(
            "wls: weights sum to non-positive {wsum}"
        )));
    }
    // Xᵀ W X (p×p), symmetric. Only accumulate the upper triangle, mirror it.
    let mut xtwx = vec![0.0; p * p];
    let mut xtwy = vec![0.0; p];
    for i in 0..n {
        let wi = w[i] / wsum;
        for a in 0..p {
            let xa = x[(i, a)];
            xtwy[a] += wi * xa * y[i];
            for b in a..p {
                xtwx[a * p + b] += wi * xa * x[(i, b)];
            }
        }
    }
    for a in 0..p {
        for b in (a + 1)..p {
            xtwx[b * p + a] = xtwx[a * p + b];
        }
    }
    let a_mat = build_mat_col_major(p, p, &xtwx);
    solve_spd(a_mat.as_ref(), &xtwy)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * b.abs().max(1.0)
    }

    #[test]
    fn wls_recovers_linear_model_unit_weights() {
        // y = 2 + 3·x; unit weights ⟹ OLS.
        let rows = vec![
            vec![1.0, 0.0],
            vec![1.0, 1.0],
            vec![1.0, 2.0],
            vec![1.0, 3.0],
            vec![1.0, 4.0],
        ];
        let x = build_mat_row_major(&rows);
        let y: Vec<f64> = vec![2.0, 5.0, 8.0, 11.0, 14.0];
        let w = vec![1.0; 5];
        let beta = wls(x.as_ref(), &y, &w).unwrap();
        assert!(approx(beta[0], 2.0, 1e-9), "intercept {}", beta[0]);
        assert!(approx(beta[1], 3.0, 1e-9), "slope {}", beta[1]);
    }

    #[test]
    fn wls_weights_favour_high_weight_rows() {
        // Two points; weight 1e6 on the second forces the line through (1, 10).
        let x = build_mat_row_major(&[vec![1.0, 0.0], vec![1.0, 1.0]]);
        let y = vec![0.0, 10.0];
        let w = vec![1.0, 1e6];
        let beta = wls(x.as_ref(), &y, &w).unwrap();
        assert!(approx(beta[1], 10.0, 1e-3), "slope {}", beta[1]);
    }

    #[test]
    fn solve_spd_known() {
        // A = [[4,2],[2,3]], b = [1,1] → x = [1/8, 1/4]
        let a = build_mat_row_major(&[vec![4.0, 2.0], vec![2.0, 3.0]]);
        let x = solve_spd(a.as_ref(), &[1.0, 1.0]).unwrap();
        assert!(approx(x[0], 0.125, 1e-12));
        assert!(approx(x[1], 0.25, 1e-12));
    }

    #[test]
    fn inv_spd_recovers_identity() {
        let a = build_mat_row_major(&[vec![4.0, 2.0], vec![2.0, 3.0]]);
        let inv = inv_spd(a.as_ref()).unwrap();
        // A·A⁻¹ ≈ I
        let i00 = 4.0 * inv[(0, 0)] + 2.0 * inv[(1, 0)];
        let i11 = 2.0 * inv[(0, 1)] + 3.0 * inv[(1, 1)];
        assert!(approx(i00, 1.0, 1e-12));
        assert!(approx(i11, 1.0, 1e-12));
    }

    #[test]
    fn singular_matrix_errors() {
        // Rank-deficient symmetric matrix.
        let a = build_mat_row_major(&[vec![1.0, 2.0], vec![2.0, 4.0]]);
        assert!(solve_spd(a.as_ref(), &[1.0, 1.0]).is_err());
    }
}
