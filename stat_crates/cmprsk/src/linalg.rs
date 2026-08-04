//! Small linear-algebra helpers over [`faer`].
//!
//! `crr` needs exactly two operations, both on the (generally non-symmetric in
//! floating point, though mathematically symmetric) information matrix:
//! a linear solve for the Newton step and a full inverse for the sandwich.
//!
//! R's `solve()` dispatches to LAPACK `dgesv` / `dgesv`-based inversion, i.e.
//! LU with partial pivoting — so [`faer::linalg::solvers::PartialPivLu`] is the
//! matching factorisation, not a Cholesky. (`crr`'s own comment notes the
//! information matrix "should be pd except in rare circumstances", which is
//! precisely why the reference does *not* rely on positive-definiteness.)

use faer::linalg::solvers::{DenseSolveCore, Solve};
use faer::{Mat, prelude::*};

use crate::error::{CmprskError, Result};

/// Build an owned `Mat<f64>` from row-major rows.
pub fn to_mat(rows: &[Vec<f64>]) -> Mat<f64> {
    let nr = rows.len();
    let nc = rows.first().map_or(0, |r| r.len());
    Mat::from_fn(nr, nc, |i, j| rows[i][j])
}

/// Convert a `Mat<f64>` back to row-major `Vec<Vec<f64>>`.
pub fn from_mat(m: &Mat<f64>) -> Vec<Vec<f64>> {
    (0..m.nrows())
        .map(|i| (0..m.ncols()).map(|j| m[(i, j)]).collect())
        .collect()
}

/// Solve `A x = b` (general square `A`) via LU with partial pivoting.
pub fn solve(a: &[Vec<f64>], b: &[f64], context: &'static str) -> Result<Vec<f64>> {
    let p = b.len();
    if a.len() != p || a.iter().any(|r| r.len() != p) {
        return Err(CmprskError::Singular { context });
    }
    let am = to_mat(a);
    if a.iter().flatten().any(|v| !v.is_finite()) {
        return Err(CmprskError::Singular { context });
    }
    let lu = am.as_ref().partial_piv_lu();
    let rhs = Mat::from_fn(p, 1, |i, _| b[i]);
    let x = lu.solve(&rhs);
    let out: Vec<f64> = (0..p).map(|i| x[(i, 0)]).collect();
    if out.iter().any(|v| !v.is_finite()) {
        return Err(CmprskError::Singular { context });
    }
    Ok(out)
}

/// Inverse of a general square matrix via LU with partial pivoting.
pub fn inverse(a: &[Vec<f64>], context: &'static str) -> Result<Vec<Vec<f64>>> {
    let p = a.len();
    if a.iter().any(|r| r.len() != p) {
        return Err(CmprskError::Singular { context });
    }
    let am = to_mat(a);
    if a.iter().flatten().any(|v| !v.is_finite()) {
        return Err(CmprskError::Singular { context });
    }
    let inv = am.as_ref().partial_piv_lu().inverse();
    let out = from_mat(&inv);
    if out.iter().flatten().any(|v| !v.is_finite()) {
        return Err(CmprskError::Singular { context });
    }
    Ok(out)
}

/// `a %*% b %*% t(a)` for square `a`, `b`.
pub fn sandwich(a: &[Vec<f64>], b: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let p = a.len();
    // tmp = a %*% b
    let mut tmp = vec![vec![0.0_f64; p]; p];
    for i in 0..p {
        for k in 0..p {
            let aik = a[i][k];
            if aik == 0.0 {
                continue;
            }
            for j in 0..p {
                tmp[i][j] += aik * b[k][j];
            }
        }
    }
    // out = tmp %*% t(a)
    let mut out = vec![vec![0.0_f64; p]; p];
    for i in 0..p {
        for j in 0..p {
            let mut s = 0.0;
            for k in 0..p {
                s += tmp[i][k] * a[j][k];
            }
            out[i][j] = s;
        }
    }
    out
}
