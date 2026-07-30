//! The core MTAG analysis — computing adjusted effect sizes and standard
//! errors from the estimated Omega and Sigma matrices.
//!
//! Port of `mtag_analysis` in `mtag.py` (Turley et al. 2018, Supplementary
//! Note §1.2.3).
//!
//! For each target trait *k*, the MTAG estimator is a conditional-expectation
//! formula derived from the multivariate normal model of Z-scores:
//!
//! ```text
//! β̂_MTAG,k = (γ_k^T Ω^{-1} γ_k + (Σ_N + Ω_{-k})^{-1}-weighted) ...
//! ```
//!
//! In practice the code computes, per SNP *m* and trait *k*:
//!
//! 1. `Σ_N = W_N^{-1} Σ_LD W_N^{-1}`  (N-scaled residual covariance)
//! 2. `Ω_min_γ = Ω − γ_k γ_k^T / τ_k²`  (Schur complement of Ω w.r.t. trait *k*)
//! 3. `xx = Ω_min_γ + Σ_N`  then  `xx^{-1}`
//! 4. `yy = γ_k / τ_k²`
//! 5. `denom = yy^T xx^{-1} yy`  (per SNP, scalar)
//! 6. `β̂ = (yy^T xx^{-1} W_N^{-1} Z) / denom`
//! 7. `SE = 1/√denom`

use faer::Mat;

use crate::linalg::invert;

/// Result of [`mtag_analysis`]: adjusted betas, SEs, and weight factors.
pub struct MtagResult {
    /// M×P matrix of MTAG-adjusted betas (standardised scale).
    pub mtag_betas: Mat<f64>,
    /// M×P matrix of MTAG standard errors.
    pub mtag_se: Mat<f64>,
    /// M×P matrix of MTAG weight factors.
    pub mtag_factor: Mat<f64>,
}

/// Perform the MTAG analysis.
///
/// Port of `mtag_analysis` in `mtag.py`.
///
/// - `zs`: M×P matrix of Z-scores.
/// - `ns`: M×P matrix of sample sizes.
/// - `omega_hat`: P×P genetic covariance matrix (Ω).
/// - `sigma_ld`: P×P residual covariance matrix (Σ_LD).
pub fn mtag_analysis(
    zs: &Mat<f64>,
    ns: &Mat<f64>,
    omega_hat: &Mat<f64>,
    sigma_ld: &Mat<f64>,
) -> MtagResult {
    let m = zs.nrows();
    let p = zs.ncols();

    // W_N_inv[m] = diag(1/sqrt(N_m,p))
    // Sigma_N[m] = W_N_inv[m] · sigma_LD · W_N_inv[m]
    //   = diag(1/N) · sigma_LD
    //
    // In numpy: W_N_inv = inv(diag(sqrt(N)))
    //           Sigma_N = einsum('mpq,mqr->mpr', einsum('mpq,qr->mpr', W_N_inv, sigma_LD), W_N_inv)
    //
    // Since W_N_inv = diag(1/sqrt(N_p)):
    //   Sigma_N[i,j] = sigma_LD[i,j] / sqrt(N_m,i * N_m,j)

    let mut mtag_betas = Mat::zeros(m, p);
    let mut mtag_se = Mat::zeros(m, p);
    let mut mtag_factor = Mat::zeros(m, p);

    for k in 0..p {
        // gamma_k = omega_hat[:, k]
        let gamma_k: Vec<f64> = (0..p).map(|i| omega_hat[(i, k)]).collect();
        let tau_k2 = omega_hat[(k, k)];

        // om_min_gam = omega_hat - outer(gamma_k, gamma_k) / tau_k2
        let mut om_min_gam = Mat::zeros(p, p);
        for i in 0..p {
            for j in 0..p {
                om_min_gam[(i, j)] = omega_hat[(i, j)] - gamma_k[i] * gamma_k[j] / tau_k2;
            }
        }

        // yy = gamma_k / tau_k2
        let yy: Vec<f64> = gamma_k.iter().map(|&v| v / tau_k2).collect();

        for idx in 0..m {
            // Sigma_N for this SNP
            let mut sigma_n = Mat::zeros(p, p);
            for i in 0..p {
                for j in 0..p {
                    sigma_n[(i, j)] = sigma_ld[(i, j)] / (ns[(idx, i)] * ns[(idx, j)]).sqrt();
                }
            }

            // xx = om_min_gam + Sigma_N
            let mut xx = Mat::zeros(p, p);
            for i in 0..p {
                for j in 0..p {
                    xx[(i, j)] = om_min_gam[(i, j)] + sigma_n[(i, j)];
                }
            }

            // inv_xx = inv(xx)
            let inv_xx = invert(&xx);

            // W_inv_Z = W_N_inv[idx] · zs[idx,:]
            // = zs[idx,p] / sqrt(N[idx,p])
            let w_inv_z: Vec<f64> = (0..p)
                .map(|j| zs[(idx, j)] / ns[(idx, j)].sqrt())
                .collect();

            // beta_denom = yy^T · inv_xx · yy  (scalar, per SNP)
            // = sum_p ( sum_q ( yy_q * inv_xx[q,p] ) ) * yy_p
            let mut tmp = vec![0.0f64; p];
            for i in 0..p {
                for j in 0..p {
                    tmp[i] += yy[j] * inv_xx[(j, i)];
                }
            }
            let beta_denom: f64 = tmp.iter().zip(&yy).map(|(a, b)| a * b).sum();

            if beta_denom.abs() < 1e-300 {
                mtag_betas[(idx, k)] = 0.0;
                mtag_se[(idx, k)] = f64::INFINITY;
                mtag_factor[(idx, k)] = 0.0;
                continue;
            }

            // mtag_var = 1 / beta_denom
            let mtag_var = 1.0 / beta_denom;
            mtag_se[(idx, k)] = mtag_var.sqrt();

            // mtag_factor = (yy^T · inv_xx) / beta_denom
            // tmp already holds yy^T · inv_xx (as a P-vector indexed by column)
            mtag_factor[(idx, k)] = tmp[k] / beta_denom;

            // mtag_beta = (yy^T · inv_xx · W_inv_Z) / beta_denom
            let mut inner = 0.0;
            for i in 0..p {
                inner += tmp[i] * w_inv_z[i];
            }
            mtag_betas[(idx, k)] = inner / beta_denom;
        }
    }

    MtagResult {
        mtag_betas,
        mtag_se,
        mtag_factor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linalg::invert as linalg_invert;

    #[test]
    fn test_invert_identity() {
        let n = 3;
        let id = Mat::from_fn(n, n, |i, j| if i == j { 1.0 } else { 0.0 });
        let inv_id = linalg_invert(&id);
        for i in 0..n {
            for j in 0..n {
                let expected = if i == j { 1.0 } else { 0.0 };
                assert!((inv_id[(i, j)] - expected).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn test_mtag_single_trait_identity() {
        // With P=1, Omega=1, Sigma=1, MTAG should return Z/sqrt(N).
        let m = 3;
        let mut zs = Mat::zeros(m, 1);
        let mut ns = Mat::zeros(m, 1);
        for i in 0..m {
            zs[(i, 0)] = (i as f64) + 1.0;
            ns[(i, 0)] = 100.0;
        }
        let omega = Mat::from_fn(1, 1, |_, _| 1.0);
        let sigma = Mat::from_fn(1, 1, |_, _| 1.0);

        let result = mtag_analysis(&zs, &ns, &omega, &sigma);

        // With single trait: xx = 0 + sigma/N = 1/100
        // inv_xx = 100
        // yy = 1/1 = 1
        // denom = 1 * 100 * 1 = 100
        // beta = (1 * 100 * Z/sqrt(100)) / 100 = Z/10
        // se = 1/sqrt(100) = 0.1
        for i in 0..m {
            let expected_beta = zs[(i, 0)] / 10.0;
            assert!(
                (result.mtag_betas[(i, 0)] - expected_beta).abs() < 1e-10,
                "beta[{i}] = {}, expected {}",
                result.mtag_betas[(i, 0)],
                expected_beta
            );
            assert!((result.mtag_se[(i, 0)] - 0.1).abs() < 1e-10);
        }
    }
}
