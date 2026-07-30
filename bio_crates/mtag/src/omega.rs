//! Estimation of the genetic covariance matrix Ω (Omega).
//!
//! Port of the Omega estimation routines in `mtag.py`:
//!
//! - [`gmm_omega`] — GMM (method of moments) estimator (default).
//! - [`numerical_omega`] — MLE via Nelder-Mead optimization of the
//!   multivariate normal log-likelihood (`numerical_omega` + `_omega_neglogL`).
//! - [`estimate_omega`] — the top-level dispatcher that selects the
//!   estimation method based on config flags.
//!
//! The Cholesky-based reparameterisation (`flatten_out_omega` /
//! `rebuild_omega`) ensures that the optimised Ω is always positive
//! semi-definite.

use faer::Mat;

use crate::linalg::{compute_n_mats, compute_z_outer, is_pos_semidef, mvn_pdf, pos_def_adjustment, cholesky};
use crate::nelder::nelder_mead_generic;
use crate::error::Result;

/// GMM (method-of-moments) estimator of Omega.
///
/// Port of `gmm_omega` in `mtag.py`:
/// ```python
/// N_mats = np.sqrt(np.einsum('mp,mq->mpq', Ns, Ns))
/// Z_outer = np.einsum('mp,mq->mpq', Zs, Zs)
/// return np.mean((Z_outer - sigma_LD) / N_mats, axis=0)
/// ```
///
/// Returns a P×P Omega matrix.
pub fn gmm_omega(zs: &Mat<f64>, ns: &Mat<f64>, sigma_ld: &Mat<f64>) -> Mat<f64> {
    let m = zs.nrows();
    let p = zs.ncols();
    let n_mats = compute_n_mats(ns);
    let z_outer = compute_z_outer(zs);

    let mut omega = Mat::zeros(p, p);
    for i in 0..p {
        for j in 0..p {
            let mut sum = 0.0;
            for idx in 0..m {
                sum += (z_outer[idx][(i, j)] - sigma_ld[(i, j)]) / n_mats[idx][(i, j)];
            }
            omega[(i, j)] = sum / m as f64;
        }
    }
    omega
}

/// Flatten the lower-triangular Cholesky decomposition of Omega into a
/// 1-D parameter vector for optimisation.
///
/// Port of `flatten_out_omega` in `mtag.py`:
/// - Off-diagonal elements are divided by `sqrt(L[i,i] * L[j,j])`
///   (correlation-like transform).
/// - Diagonal elements are log-transformed.
/// - The lower-triangular elements are stacked row-wise.
pub fn flatten_out_omega(omega_est: &Mat<f64>) -> Vec<f64> {
    let p = omega_est.nrows();
    let l = cholesky(omega_est).expect("flatten_out_omega: Cholesky failed");
    let mut x_chol_trf = Mat::zeros(p, p);

    // Transform off-diagonal lower-triangular elements.
    for i in 0..p {
        for j in 0..i {
            x_chol_trf[(i, j)] = l[(i, j)] / (l[(i, i)] * l[(j, j)]).sqrt();
        }
    }
    // Log-transform diagonal.
    for i in 0..p {
        x_chol_trf[(i, i)] = l[(i, i)].ln();
    }

    // Stack lower-triangular elements row-wise: (0,0) (1,0) (1,1) (2,0) (2,1) (2,2) ...
    let mut result = Vec::with_capacity(p * (p + 1) / 2);
    for i in 0..p {
        for j in 0..=i {
            result.push(x_chol_trf[(i, j)]);
        }
    }
    result
}

/// Rebuild Omega from a flattened Cholesky parameter vector.
///
/// Port of `rebuild_omega` in `mtag.py` (the `s is None` path):
/// 1. Fill the lower triangle of `cholL` from `chol_elems` (row-wise).
/// 2. Exponentiate the diagonal so Cholesky is unique.
/// 3. Rescale off-diagonal: `cholL[i,j] *= sqrt(cholL[i,i] * cholL[j,j])`.
/// 4. `omega = cholL · cholL^T`.
pub fn rebuild_omega(chol_elems: &[f64]) -> Mat<f64> {
    let p = ((-1.0 + (1.0 + 8.0 * chol_elems.len() as f64).sqrt()) / 2.0).round() as usize;
    let mut chol_l = Mat::zeros(p, p);

    // Fill lower triangle row-wise.
    let mut idx = 0;
    for i in 0..p {
        for j in 0..=i {
            chol_l[(i, j)] = chol_elems[idx];
            idx += 1;
        }
    }

    // Exponentiate diagonal.
    for i in 0..p {
        chol_l[(i, i)] = chol_l[(i, i)].exp();
    }

    // Rescale off-diagonal.
    for i in 0..p {
        for j in 0..i {
            chol_l[(i, j)] *= (chol_l[(i, i)] * chol_l[(j, j)]).sqrt();
        }
    }

    // omega = cholL · cholL^T
    let mut omega = Mat::zeros(p, p);
    for i in 0..p {
        for j in 0..p {
            let mut sum = 0.0;
            for k in 0..p {
                sum += chol_l[(i, k)] * chol_l[(j, k)];
            }
            omega[(i, j)] = sum;
        }
    }
    omega
}

/// Negative log-likelihood of Omega given Z-scores (for numerical optimisation).
///
/// Port of `_omega_neglogL` in `mtag.py` (the non-perfect-gencov path).
fn omega_neglog_l(x: &[f64], zs: &Mat<f64>, n_mats: &[Mat<f64>], sigma_ld: &Mat<f64>) -> f64 {
    let omega_it = rebuild_omega(x);
    let joint_prob = mvn_pdf(zs, &omega_it, sigma_ld, n_mats);
    -joint_prob.iter().filter(|p| **p > 0.0).map(|p| p.ln()).sum::<f64>()
}

/// Configuration for Omega estimation.
#[derive(Clone, Debug)]
pub struct OmegaConfig {
    /// Use Nelder-Mead MLE instead of GMM.
    pub numerical: bool,
    /// Assume perfect genetic covariance (off-diagonals = sqrt(h²_i · h²_j)).
    pub perfect_gencov: bool,
    /// Assume equal heritability across traits (requires perfect_gencov).
    pub equal_h2: bool,
    /// Tolerance for Nelder-Mead optimisation.
    pub tol: f64,
    /// Whether to assume no sample overlap.
    pub no_overlap: bool,
}

impl Default for OmegaConfig {
    fn default() -> Self {
        Self {
            numerical: false,
            perfect_gencov: false,
            equal_h2: false,
            tol: 1e-6,
            no_overlap: false,
        }
    }
}

/// Estimate the genetic covariance matrix Omega.
///
/// Port of `estimate_omega` in `mtag.py`:
/// - If `perfect_gencov && equal_h2`: return all-ones matrix.
/// - If `numerical`: GMM diagonal as starting point → Nelder-Mead MLE.
/// - If `perfect_gencov`: GMM → posdef adjustment →
///   `sqrt(diag ⊗ diag)`.
/// - Otherwise (default): GMM → posdef adjustment.
pub fn estimate_omega(
    zs: &Mat<f64>,
    ns: &Mat<f64>,
    sigma_ld: &Mat<f64>,
    cfg: &OmegaConfig,
) -> Result<Mat<f64>> {
    let p = zs.ncols();

    if cfg.perfect_gencov && cfg.equal_h2 {
        return Ok(Mat::from_fn(p, p, |_, _| 1.0));
    }

    if cfg.numerical {
        // Starting point: GMM diagonal only.
        let gmm = gmm_omega(zs, ns, sigma_ld);
        let mut omega_in = Mat::zeros(p, p);
        for i in 0..p {
            omega_in[(i, i)] = gmm[(i, i)];
        }

        let x_start = flatten_out_omega(&omega_in);
        let n_mats = compute_n_mats(ns);
        let max_iter = if cfg.perfect_gencov { p * 250 } else { p * (p + 1) * 500 };

        let opt_x = nelder_mead_generic(
            |x| omega_neglog_l(x, zs, &n_mats, sigma_ld),
            &x_start,
            0.5, // initial step
            cfg.tol, // xatol
            1e-8,   // fatol
            max_iter,
        );

        if cfg.perfect_gencov {
            // sqrt(exp(x) ⊗ exp(x))
            let diag: Vec<f64> = (0..p).map(|i| opt_x[i * (i + 1) / 2 + i].exp().sqrt()).collect();
            let mut omega = Mat::zeros(p, p);
            for i in 0..p {
                for j in 0..p {
                    omega[(i, j)] = diag[i] * diag[j];
                }
            }
            Ok(omega)
        } else {
            Ok(rebuild_omega(&opt_x))
        }
    } else if cfg.perfect_gencov {
        let gmm = gmm_omega(zs, ns, sigma_ld);
        let adjusted = pos_def_adjustment(gmm, 0.99, 1000)?;
        let diag: Vec<f64> = (0..p).map(|i| adjusted[(i, i)].sqrt()).collect();
        let mut omega = Mat::zeros(p, p);
        for i in 0..p {
            for j in 0..p {
                omega[(i, j)] = diag[i] * diag[j];
            }
        }
        Ok(omega)
    } else {
        // Default: GMM with posdef adjustment.
        let gmm = gmm_omega(zs, ns, sigma_ld);
        pos_def_adjustment(gmm, 0.99, 1000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flatten_rebuild_roundtrip() {
        // Create a simple 3×3 positive-definite Omega.
        let mut omega = Mat::zeros(3, 3);
        omega[(0, 0)] = 0.5;
        omega[(1, 1)] = 0.3;
        omega[(2, 2)] = 0.4;
        omega[(0, 1)] = 0.1;
        omega[(1, 0)] = 0.1;
        omega[(0, 2)] = 0.05;
        omega[(2, 0)] = 0.05;
        omega[(1, 2)] = 0.08;
        omega[(2, 1)] = 0.08;

        let flat = flatten_out_omega(&omega);
        assert_eq!(flat.len(), 6); // 3*(3+1)/2

        let rebuilt = rebuild_omega(&flat);
        for i in 0..3 {
            for j in 0..3 {
                assert!(
                    (omega[(i, j)] - rebuilt[(i, j)]).abs() < 1e-10,
                    "Mismatch at [{i},{j}]: {} vs {}",
                    omega[(i, j)],
                    rebuilt[(i, j)]
                );
            }
        }
    }

    #[test]
    fn test_gmm_omega_identity_case() {
        // When Z_outer == sigma_LD on average, omega should be ~0.
        let mut zs = Mat::zeros(2, 2);
        zs[(0, 0)] = 1.0;
        zs[(0, 1)] = 0.5;
        zs[(1, 0)] = 2.0;
        zs[(1, 1)] = 1.0;

        let mut ns = Mat::zeros(2, 2);
        ns[(0, 0)] = 100.0;
        ns[(0, 1)] = 100.0;
        ns[(1, 0)] = 100.0;
        ns[(1, 1)] = 100.0;

        let mut sigma = Mat::zeros(2, 2);
        sigma[(0, 0)] = 1.0;
        sigma[(1, 1)] = 1.0;
        sigma[(0, 1)] = 0.5;
        sigma[(1, 0)] = 0.5;

        let omega = gmm_omega(&zs, &ns, &sigma);
        // mean of (Z*Z - sigma) / N for each element
        // For [0,0]: ((1*1 - 1)/100 + (2*2 - 1)/100) / 2 = (0 + 3)/200 = 0.015
        assert!((omega[(0, 0)] - 0.015).abs() < 1e-10);
    }
}
