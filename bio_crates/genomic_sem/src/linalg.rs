//! Linear-algebra helpers built on [`faer`].
//!
//! All functions operate on `faer::Mat<f64>` (column-major). Conventions
//! mirror R's matrix operations as closely as possible.

use faer::{Mat, MatRef, Side};
use faer::linalg::solvers::{Llt, Solve};

use crate::error::{GenomicSemError, Result};

/// Solve the linear system `A·x = b` where `A` is a general square matrix.
/// Returns `x`.
pub fn solve(a: &Mat<f64>, b: &Mat<f64>) -> Result<Mat<f64>> {
    let n = a.nrows();
    if a.ncols() != n {
        return Err(GenomicSemError::NotSquare {
            rows: a.nrows(),
            cols: a.ncols(),
        });
    }
    let lu = a.as_ref().partial_piv_lu();
    Ok(lu.solve(b))
}

/// Solve using SPD decomposition (Cholesky-based). Falls back to LU
/// if not SPD.
pub fn solve_spd(a: &Mat<f64>, b: &Mat<f64>) -> Mat<f64> {
    match Llt::new(a.as_ref(), Side::Lower) {
        Ok(llt) => llt.solve(b),
        Err(_) => {
            let lu = a.as_ref().partial_piv_lu();
            lu.solve(b)
        }
    }
}

/// Symmetric eigenvalue decomposition.
/// Returns (eigenvalues in **descending** order (matching R's `eigen`),
/// eigenvectors as columns).
pub fn eigen_sym(mat: &Mat<f64>) -> (Vec<f64>, Mat<f64>) {
    let n = mat.nrows();
    debug_assert_eq!(n, mat.ncols());

    let e = mat
        .as_ref()
        .self_adjoint_eigen(Side::Lower)
        .expect("eigen failed");
    let s = e.S();
    let u = e.U();
    let sv = s.column_vector();
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by(|&i, &j| sv[j].partial_cmp(&sv[i]).unwrap());
    let lambda: Vec<f64> = idx.iter().map(|&i| sv[i]).collect();
    let q = Mat::from_fn(n, n, |row, col| u[(row, idx[col])]);
    (lambda, q)
}

/// Extract the lower triangle (including diagonal) as a vector, in
/// column-major (vech) order — matching `lav_matrix_vech`.
pub fn vech(mat: &Mat<f64>) -> Vec<f64> {
    let n = mat.nrows();
    let mut out = Vec::with_capacity(n * (n + 1) / 2);
    for j in 0..n {
        for i in j..n {
            out.push(mat[(i, j)]);
        }
    }
    out
}

/// Expand a half-vector (vech) back to a full symmetric matrix.
pub fn vech_inv(vec: &[f64]) -> Mat<f64> {
    let m = vec.len();
    let n = ((-1.0 + (1.0 + 8.0 * m as f64).sqrt()) / 2.0).round() as usize;
    let mut mat = Mat::zeros(n, n);
    let mut idx = 0;
    for j in 0..n {
        for i in j..n {
            mat[(i, j)] = vec[idx];
            mat[(j, i)] = vec[idx];
            idx += 1;
        }
    }
    mat
}

/// Convert a covariance matrix to a correlation matrix (R's `cov2cor`).
pub fn cov2cor(cov: &Mat<f64>) -> Mat<f64> {
    let n = cov.nrows();
    let mut cor = Mat::zeros(n, n);
    let mut sd = vec![0.0f64; n];
    for i in 0..n {
        sd[i] = cov[(i, i)].max(0.0).sqrt();
    }
    for i in 0..n {
        for j in 0..n {
            let denom = sd[i] * sd[j];
            cor[(i, j)] = if denom > 0.0 {
                cov[(i, j)] / denom
            } else {
                0.0
            };
        }
    }
    for i in 0..n {
        cor[(i, i)] = 1.0;
    }
    cor
}

/// Compute `diag(sqrt(diag(S)))` — the diagonal scaling matrix.
pub fn diag_sqrt(mat: &Mat<f64>) -> Mat<f64> {
    let n = mat.nrows();
    let mut d = Mat::zeros(n, n);
    for i in 0..n {
        let v = mat[(i, i)].max(0.0).sqrt();
        d[(i, i)] = if v > 0.0 { v } else { 1e-10 };
    }
    d
}

/// Matrix inverse via partial-pivot LU.
pub fn inverse(mat: &Mat<f64>) -> Mat<f64> {
    let n = mat.nrows();
    let lu = mat.as_ref().partial_piv_lu();
    let identity = Mat::identity(n, n);
    lu.solve(&identity)
}

/// Cross-product: `A' · B`
pub fn crossprod(a: &Mat<f64>, b: &Mat<f64>) -> Mat<f64> {
    a.transpose() * b
}

/// Transpose-cross-product: `A · B'`
pub fn tcrossprod(a: &Mat<f64>, b: &Mat<f64>) -> Mat<f64> {
    a * b.transpose()
}

/// Compute the covariance of rows of a matrix (matching R's `cov()` on
/// a matrix where each row is an observation).
pub fn row_cov(mat: &Mat<f64>) -> Mat<f64> {
    let n = mat.nrows();
    let p = mat.ncols();
    let mut col_means = vec![0.0f64; p];
    for j in 0..p {
        let mut s = 0.0f64;
        for i in 0..n {
            s += mat[(i, j)];
        }
        col_means[j] = s / n as f64;
    }
    let mut cov = Mat::zeros(p, p);
    for i in 0..n {
        for j in 0..p {
            for k in 0..p {
                cov[(j, k)] += (mat[(i, j)] - col_means[j]) * (mat[(i, k)] - col_means[k]);
            }
        }
    }
    let denom = (n - 1) as f64;
    for j in 0..p {
        for k in 0..p {
            cov[(j, k)] /= denom;
        }
    }
    cov
}

/// Create a diagonal matrix from a slice of diagonal values.
pub fn diag_from_vec(vals: &[f64]) -> Mat<f64> {
    let n = vals.len();
    let mut m = Mat::zeros(n, n);
    for i in 0..n {
        m[(i, i)] = vals[i];
    }
    m
}

/// Extract the diagonal of a matrix as a vector.
pub fn diag_to_vec(mat: &Mat<f64>) -> Vec<f64> {
    let n = mat.nrows().min(mat.ncols());
    (0..n).map(|i| mat[(i, i)]).collect()
}

/// Compute sqrt of diagonal elements.
pub fn sqrt_diag(mat: &Mat<f64>) -> Vec<f64> {
    diag_to_vec(mat)
        .into_iter()
        .map(|v| v.max(0.0).sqrt())
        .collect()
}

/// Is the matrix symmetric?
pub fn is_symmetric(mat: &Mat<f64>, tol: f64) -> bool {
    let n = mat.nrows();
    if n != mat.ncols() {
        return false;
    }
    for i in 0..n {
        for j in (i + 1)..n {
            if (mat[(i, j)] - mat[(j, i)]).abs() > tol {
                return false;
            }
        }
    }
    true
}
