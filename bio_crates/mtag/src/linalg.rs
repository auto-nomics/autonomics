//! Linear-algebra helpers for MTAG.
//!
//! All dense linear algebra runs on [`faer`]. The functions here mirror
//! specific numpy/scipy calls in the Python MTAG reference:
//!
//! - [`is_pos_semidef`] — `np.all(np.linalg.eigvals(m) >= 0)` (with the
//!   special-cased 2×2 shortcut from the Python `is_pos_semidef`).
//! - [`pos_def_adjustment`] — the adjustment procedure in
//!   `_posDef_adjustment` (Supplementary Note §1.2.2).
//! - [`cov2corr`] — convert a covariance matrix to a correlation matrix.
//! - [`mvn_pdf`] — multivariate normal PDF, matching the einsum-based
//!   `jointEffect_probability`.
//! - [`cholesky`] / [`solve_lower`] — thin wrappers around faer's SPD
//!   factorisation used by the MVN PDF.

use faer::Mat;
use crate::error::{MtagError, Result};

/// Check whether a symmetric matrix is positive semi-definite.
///
/// Mirrors the Python `is_pos_semidef` in `mtag.py`:
/// for 2×2 matrices uses the determinant shortcut
/// `sqrt(d₀₀·d₁₁) ≥ |d₀₁|`; otherwise checks all eigenvalues ≥ 0.
///
/// **Note**: numpy's `eigvals` returns complex values when the matrix is not
/// exactly symmetric; we use faer's symmetric eigenvalue decomposition
/// (`SelfAdjoint`), which returns real eigenvalues. The comparison `>= 0`
/// matches the Python behaviour exactly for symmetric input.
pub fn is_pos_semidef(m: &Mat<f64>) -> bool {
    let n = m.nrows();
    debug_assert_eq!(n, m.ncols());
    if n == 2 {
        return (m[(0, 0)] * m[(1, 1)]).sqrt() >= m[(0, 1)].abs();
    }
    // Symmetric eigenvalue decomposition: all eigenvalues of a symmetric
    // matrix are real. We use faer's selfadjoint eigendecomposition.
    let eigvals = symmetric_eigvals(m);
    eigvals.iter().all(|&v| v >= 0.0)
}

/// Compute eigenvalues of a symmetric (self-adjoint) matrix using the
/// classical Jacobi eigenvalue algorithm. For MTAG, P is small (typically
/// 2–20), so this O(n³) per-sweep method is both fast and dependency-free.
///
/// This replaces `np.linalg.eigvals` for symmetric matrices — eigenvalues
/// are returned sorted in ascending order to match numpy.
fn symmetric_eigvals(m: &Mat<f64>) -> Vec<f64> {
    let n = m.nrows();
    let mut a: Vec<f64> = (0..n).flat_map(|i| (0..n).map(move |j| m[(i, j)])).collect();
    let mut eigenvalues = vec![0.0f64; n];

    // Jacobi rotations: iterate until off-diagonal is negligible.
    const MAX_SWEEPS: usize = 100;
    for _sweep in 0..MAX_SWEEPS {
        // Sum of squares of off-diagonal elements.
        let mut off: f64 = 0.0;
        for p in 0..n {
            for q in (p + 1)..n {
                off += a[p * n + q] * a[p * n + q];
            }
        }
        if off < 1e-30 {
            break;
        }
        for p in 0..n {
            for q in (p + 1)..n {
                let apq: f64 = a[p * n + q];
                if apq.abs() < 1e-300 {
                    continue;
                }
                let app: f64 = a[p * n + p];
                let aqq: f64 = a[q * n + q];
                let theta: f64 = (aqq - app) / (2.0 * apq);
                let t: f64 = theta.signum()
                    / (theta.abs() + (1.0_f64 + theta * theta).sqrt());
                let c: f64 = 1.0_f64 / (1.0_f64 + t * t).sqrt();
                let s: f64 = t * c;
                // Apply rotation
                a[p * n + p] = app - t * apq;
                a[q * n + q] = aqq + t * apq;
                a[p * n + q] = 0.0;
                a[q * n + p] = 0.0;
                for i in 0..n {
                    if i != p && i != q {
                        let aip: f64 = a[i * n + p];
                        let aiq: f64 = a[i * n + q];
                        a[i * n + p] = c * aip - s * aiq;
                        a[p * n + i] = a[i * n + p];
                        a[i * n + q] = s * aip + c * aiq;
                        a[q * n + i] = a[i * n + q];
                    }
                }
            }
        }
    }
    for i in 0..n {
        eigenvalues[i] = a[i * n + i];
    }
    eigenvalues.sort_by(|a, b| a.partial_cmp(b).unwrap());
    eigenvalues
}

/// Positive (semi)definiteness adjustment.
///
/// Port of `_posDef_adjustment` in `mtag.py`:
/// 1. If already PSD, return as-is.
/// 2. Cap each off-diagonal `|m[i,j]|` at `scaling_factor * sqrt(m[i,i]*m[j,j])`.
/// 3. Iteratively scale off-diagonals by `scaling_factor` (keeping the
///    diagonal fixed) until PSD or `max_it` reached.
pub fn pos_def_adjustment(mut mat: Mat<f64>, scaling_factor: f64, max_it: usize) -> Result<Mat<f64>> {
    let n = mat.nrows();
    if n != mat.ncols() {
        return Err(MtagError::DimensionMismatch(format!(
            "pos_def_adjustment expects square matrix, got {n}×{}",
            mat.ncols()
        )));
    }

    if is_pos_semidef(&mat) {
        return Ok(mat);
    }

    // Step 1: cap off-diagonals at scaling_factor * sign * sqrt(di*dj).
    for i in 0..n {
        for j in i..n {
            let threshold = (mat[(i, i)] * mat[(j, j)]).sqrt();
            if mat[(i, j)].abs() > threshold {
                let sign = if mat[(i, j)] >= 0.0 { 1.0 } else { -1.0 };
                mat[(i, j)] = scaling_factor * sign * threshold;
                mat[(j, i)] = mat[(i, j)];
            }
        }
    }

    // Step 2: iterative off-diagonal scaling.
    let mut n_iter = 0;
    while !is_pos_semidef(&mat) && n_iter < max_it {
        let diag: Vec<f64> = (0..n).map(|i| mat[(i, i)]).collect();
        // Scale entire matrix by scaling_factor, then restore diagonal.
        for i in 0..n {
            for j in 0..n {
                mat[(i, j)] *= scaling_factor;
            }
        }
        for i in 0..n {
            mat[(i, i)] = diag[i];
        }
        n_iter += 1;
    }

    Ok(mat)
}

/// Convert a covariance matrix to a correlation matrix.
///
/// Port of `cov2corr` in `mtag.py`.
pub fn cov2corr(cov: &Mat<f64>) -> Mat<f64> {
    let n = cov.nrows();
    let std_: Vec<f64> = (0..n).map(|i| cov[(i, i)].sqrt()).collect();
    let mut corr = Mat::zeros(n, n);
    for i in 0..n {
        for j in 0..n {
            corr[(i, j)] = cov[(i, j)] / (std_[i] * std_[j]);
        }
    }
    corr
}

/// Cholesky factorisation `L` such that `L L^T = mat`, for a symmetric
/// positive-definite matrix. Returns the lower-triangular `L`.
pub fn cholesky(mat: &Mat<f64>) -> Result<Mat<f64>> {
    let n = mat.nrows();
    let mut l = Mat::zeros(n, n);
    for i in 0..n {
        for j in 0..=i {
            let mut sum = mat[(i, j)];
            for k in 0..j {
                sum -= l[(i, k)] * l[(j, k)];
            }
            if i == j {
                if sum <= 0.0 {
                    return Err(MtagError::Linalg(format!(
                        "Cholesky: matrix not positive definite at diagonal [{i},{j}] (value {sum})"
                    )));
                }
                l[(i, j)] = sum.sqrt();
            } else {
                l[(i, j)] = sum / l[(j, j)];
            }
        }
    }
    Ok(l)
}

/// Solve `L x = b` for a lower-triangular matrix `L` (forward substitution).
pub fn solve_lower(l: &Mat<f64>, b: &[f64]) -> Vec<f64> {
    let n = l.nrows();
    let mut x = vec![0.0f64; n];
    for i in 0..n {
        let mut sum = b[i];
        for k in 0..i {
            sum -= l[(i, k)] * x[k];
        }
        x[i] = sum / l[(i, i)];
    }
    x
}

/// Evaluate the multivariate normal PDF for M observation vectors.
///
/// Port of `jointEffect_probability` in `mtag.py` (the `n_S == 1` path,
/// i.e. a single causal state).
///
/// - `zs`: M×P matrix of Z-scores (one row per SNP).
/// - `omega`: P×P genetic covariance matrix.
/// - `sigma_ld`: P×P residual covariance (Σ_LD).
/// - `n_mats`: M×P×P array of sqrt(N_p · N_q) per SNP.
///
/// Returns a vector of M joint probabilities.
pub fn mvn_pdf(
    zs: &Mat<f64>,
    omega: &Mat<f64>,
    sigma_ld: &Mat<f64>,
    n_mats: &[Mat<f64>], // length M, each P×P
) -> Vec<f64> {
    let m = zs.nrows();
    let p = zs.ncols();

    // cov_s for each SNP: N_mats ⊙ omega + sigma_ld
    let mut joint_probs = vec![0.0f64; m];

    let log_2pi_half = (p as f64) * (std::f64::consts::LN_2 + std::f64::consts::PI.ln()) * 0.5;

    for idx in 0..m {
        // Build cov_s = N_mats[idx] ⊙ omega + sigma_ld
        let mut cov_s = Mat::zeros(p, p);
        for i in 0..p {
            for j in 0..p {
                cov_s[(i, j)] = n_mats[idx][(i, j)] * omega[(i, j)] + sigma_ld[(i, j)];
            }
        }

        // Cholesky of cov_s
        let l = match cholesky(&cov_s) {
            Ok(l) => l,
            Err(_) => {
                joint_probs[idx] = 0.0;
                continue;
            }
        };

        // xRinv = solve(L, zs[idx,:])
        let z_row: Vec<f64> = (0..p).map(|j| zs[(idx, j)]).collect();
        let x_rinv = solve_lower(&l, &z_row);

        // logSqrtDetSigma = sum(log(diag(R))) = sum(log(diag(L^T))) = sum(log(diag(L)))
        let log_sqrt_det: f64 = (0..p).map(|i| l[(i, i)].ln()).sum();

        // quadform = sum(xRinv^2)
        let quadform: f64 = x_rinv.iter().map(|v| v * v).sum();

        joint_probs[idx] = (-0.5 * quadform - log_sqrt_det - log_2pi_half).exp();
    }

    joint_probs
}

/// Invert a small P×P matrix via Gauss-Jordan elimination with partial pivoting.
///
/// Used for the per-SNP P×P solves in `mtag_analysis` (P is the number of
/// traits, typically 2–20).
pub fn invert(mat: &Mat<f64>) -> Mat<f64> {
    let n = mat.nrows();
    debug_assert_eq!(n, mat.ncols());

    // Augmented matrix [mat | I]
    let mut aug = vec![vec![0.0f64; 2 * n]; n];
    for i in 0..n {
        for j in 0..n {
            aug[i][j] = mat[(i, j)];
        }
        aug[i][n + i] = 1.0;
    }

    // Forward elimination with partial pivoting.
    for col in 0..n {
        let mut pivot = col;
        for row in (col + 1)..n {
            if aug[row][col].abs() > aug[pivot][col].abs() {
                pivot = row;
            }
        }
        aug.swap(col, pivot);

        let pv = aug[col][col];
        if pv.abs() < 1e-300 {
            continue;
        }
        for j in 0..(2 * n) {
            aug[col][j] /= pv;
        }
        for row in 0..n {
            if row != col {
                let factor = aug[row][col];
                for j in 0..(2 * n) {
                    aug[row][j] -= factor * aug[col][j];
                }
            }
        }
    }

    let mut inv = Mat::zeros(n, n);
    for i in 0..n {
        for j in 0..n {
            inv[(i, j)] = aug[i][n + j];
        }
    }
    inv
}

/// Compute `N_mats[m,p,q] = sqrt(Ns[m,p] * Ns[m,q])` for all SNPs.
///
/// Port of `np.sqrt(np.einsum('mp,mq->mpq', Ns, Ns))`.
pub fn compute_n_mats(ns: &Mat<f64>) -> Vec<Mat<f64>> {
    let m = ns.nrows();
    let p = ns.ncols();
    let mut n_mats = Vec::with_capacity(m);
    for idx in 0..m {
        let mut nm = Mat::zeros(p, p);
        for i in 0..p {
            for j in 0..p {
                nm[(i, j)] = (ns[(idx, i)] * ns[(idx, j)]).sqrt();
            }
        }
        n_mats.push(nm);
    }
    n_mats
}

/// Compute `Z_outer[m,p,q] = Zs[m,p] * Zs[m,q]` for all SNPs.
///
/// Port of `np.einsum('mp,mq->mpq', Zs, Zs)`.
pub fn compute_z_outer(zs: &Mat<f64>) -> Vec<Mat<f64>> {
    let m = zs.nrows();
    let p = zs.ncols();
    let mut z_outer = Vec::with_capacity(m);
    for idx in 0..m {
        let mut zo = Mat::zeros(p, p);
        for i in 0..p {
            for j in 0..p {
                zo[(i, j)] = zs[(idx, i)] * zs[(idx, j)];
            }
        }
        z_outer.push(zo);
    }
    z_outer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_pos_semidef_2x2() {
        let mut m = Mat::zeros(2, 2);
        m[(0, 0)] = 2.0;
        m[(1, 1)] = 2.0;
        m[(0, 1)] = 1.0;
        m[(1, 0)] = 1.0;
        assert!(is_pos_semidef(&m));

        m[(0, 1)] = 2.0;
        m[(1, 0)] = 2.0;
        // sqrt(4) = 2 >= 2 → still PSD (boundary)
        assert!(is_pos_semidef(&m));

        m[(0, 1)] = 2.1;
        m[(1, 0)] = 2.1;
        assert!(!is_pos_semidef(&m));
    }

    #[test]
    fn test_cov2corr() {
        let mut cov = Mat::zeros(2, 2);
        cov[(0, 0)] = 4.0;
        cov[(1, 1)] = 9.0;
        cov[(0, 1)] = 3.0;
        cov[(1, 0)] = 3.0;
        let corr = cov2corr(&cov);
        assert!((corr[(0, 0)] - 1.0).abs() < 1e-12);
        assert!((corr[(1, 1)] - 1.0).abs() < 1e-12);
        // 3 / (2*3) = 0.5
        assert!((corr[(0, 1)] - 0.5).abs() < 1e-12);
    }
}
