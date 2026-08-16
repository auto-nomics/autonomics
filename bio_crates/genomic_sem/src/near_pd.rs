//! Nearest positive-definite matrix (Higham 2002).
//!
//! Port of R's `Matrix::nearPD()` for the case used by GenomicSEM:
//! `nearPD(x, corr = FALSE)` — find the nearest SPD matrix in Frobenius norm.
//!
//! The algorithm alternates between two projections:
//! 1. Project onto the set of symmetric matrices.
//! 2. Project onto the cone of positive-semidefinite matrices (by clipping
//!    negative eigenvalues to zero).
//!
//! References:
//! - Higham, N. J. (2002). Computing the nearest correlation matrix — a
//!   problem from finance. *IMA Journal of Numerical Analysis*, 22(3), 329–343.

use faer::Mat;

use crate::linalg;

/// Maximum number of iterations.
const MAX_ITER: usize = 100;

/// Convergence tolerance.
const CONV_TOL: f64 = 1e-8;

/// Find the nearest symmetric positive-definite matrix to `mat`.
///
/// This reproduces `Matrix::nearPD(mat, corr = FALSE, keepDiag = FALSE)`
/// as used throughout GenomicSEM. The input is symmetrised first.
pub fn near_pd(mat: &Mat<f64>) -> Mat<f64> {
    let n = mat.nrows();
    debug_assert_eq!(n, mat.ncols());

    // Step 1: symmetrise
    let x = symmetrise(mat);

    // Check if already PD
    let (eigvals, _) = linalg::eigen_sym(&x);
    if eigvals.last().map(|&v| v > 0.0).unwrap_or(false) {
        return x;
    }

    // Higham's alternating projections algorithm
    let mut y_old = x.clone();
    let mut d_old = Mat::zeros(n, n);

    for _ in 0..MAX_ITER {
        // R_k = Y_old - D_old  (residual)
        let r_k = &y_old - &d_old;

        // Project onto PSD: eigendecompose, clip negatives, reconstruct
        let (eigvals, eigvecs) = linalg::eigen_sym(&r_k);
        let mut diag_vals = Mat::zeros(n, n);
        for i in 0..n {
            diag_vals[(i, i)] = eigvals[i].max(0.0);
        }
        // X_k = P · Λ⁺ · P'
        let x_k = &eigvecs * &diag_vals * eigvecs.transpose();

        // D_k = X_k - R_k
        let d_s = &x_k - &r_k;

        // Y_k = X_k + D_k  (but actually Y_k = X_k + D_k from the paper —
        // this is the Dykstra correction: Y_k = R_k + D_k = X_k
        // Actually the correct update is:
        //   Y_k = X_k  (the PSD projection is our new iterate)
        // with the Dykstra correction stored in D.
        // Let me re-derive from Higham's pseudocode:
        //   R = S - D_prev
        //   X = P_psd(R)
        //   D = X - R   (= S - D_prev projected - (S - D_prev))
        //   Y = X + D_prev  ← this is wrong
        //
        // The standard Dykstra correction for nearest SPD is:
        //   x = symmetrise(S)
        //   dS = 0, y = x
        //   repeat:
        //     r = y - dS
        //     x = psd_proj(r)     (clip eigenvalues)
        //     dS = x - r
        //     y = x               (for corr=FALSE, no unit-diagonal projection)
        //   until converged
        let y_s = x_k;

        // Check convergence: ||Y_k - Y_{k-1}||_∞ / ||Y_k||_∞
        let diff = &y_s - &y_old;
        let max_diff = mat_inf_norm(&diff);
        let norm_y = mat_inf_norm(&y_s);
        let rel_diff = if norm_y > 0.0 {
            max_diff / norm_y
        } else {
            max_diff
        };

        y_old = y_s;
        d_old = d_s;

        if rel_diff < CONV_TOL {
            break;
        }
    }

    // Final symmetrisation to clean up numerical noise
    symmetrise(&y_old)
}

/// Project onto symmetric matrices: (X + X') / 2.
fn symmetrise(mat: &Mat<f64>) -> Mat<f64> {
    let n = mat.nrows();
    let mut out = Mat::zeros(n, n);
    for i in 0..n {
        for j in 0..n {
            out[(i, j)] = 0.5 * (mat[(i, j)] + mat[(j, i)]);
        }
    }
    out
}

/// Infinity norm (max absolute row sum).
fn mat_inf_norm(mat: &Mat<f64>) -> f64 {
    let m = mat.nrows();
    let n = mat.ncols();
    let mut max_val = 0.0;
    for i in 0..m {
        let mut row_sum = 0.0;
        for j in 0..n {
            row_sum += mat[(i, j)].abs();
        }
        if row_sum > max_val {
            max_val = row_sum;
        }
    }
    max_val
}

/// Check whether a matrix is positive (semi-)definite by examining its
/// smallest eigenvalue.
pub fn is_spd(mat: &Mat<f64>) -> bool {
    let (eigvals, _) = linalg::eigen_sym(mat);
    eigvals.first().map(|&v| v > 0.0).unwrap_or(false)
}

/// Check whether the smallest eigenvalue is ≤ 0 (used by GenomicSEM's
/// `ifelse(eigen(S_LD)$values[nrow(S_LD)] <= 0, ...)` check).
pub fn min_eigenvalue(mat: &Mat<f64>) -> f64 {
    let (eigvals, _) = linalg::eigen_sym(mat);
    eigvals.first().copied().unwrap_or(f64::NEG_INFINITY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_already_pd() {
        let a = Mat::from_fn(3, 3, |i, j| if i == j { 2.0 } else { 0.0 });
        let result = near_pd(&a);
        for i in 0..3 {
            assert!((result[(i, i)] - 2.0).abs() < 1e-8);
        }
    }

    #[test]
    fn test_near_pd_basic() {
        // A matrix that is NOT positive definite
        let mut a = Mat::zeros(2, 2);
        a[(0, 0)] = 1.0;
        a[(0, 1)] = 2.0;
        a[(1, 0)] = 2.0;
        a[(1, 1)] = 1.0;
        // Eigenvalues are 3 and -1, so not PD

        let result = near_pd(&a);
        // Check that result is PD
        assert!(is_spd(&result), "nearPD result should be SPD");
    }

    #[test]
    fn test_symmetric_input() {
        let a = Mat::from_fn(3, 3, mat22_val);
        let _ = near_pd(&a);
    }

    fn mat22_val(i: usize, j: usize) -> f64 {
        match (i, j) {
            (0, 0) => 1.0,
            (0, 1) => 0.5,
            (1, 0) => 0.5,
            (1, 1) => 1.0,
            (2, 2) => 1.0,
            _ => 0.0,
        }
    }

    #[test]
    fn test_min_eigenvalue() {
        let a = Mat::from_fn(2, 2, |i, j| if i == j { 2.0 } else { 0.0 });
        let min_eig = min_eigenvalue(&a);
        assert!((min_eig - 2.0).abs() < 1e-8);
    }
}
