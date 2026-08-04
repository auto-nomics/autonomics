//! Cochran's Q for instrument validity (pleiotropy) — faithful port of
//! `R/pleiotropy_mvmr.R`.
//!
//! After fitting the IVW estimate `A`, computes
//! ```text
//!   σ²_A,j = se(β_YG,j)² + Σ_k A_k² se(β_Xk,j)²           (gencov scalar)
//!   Q_valid = Σ_j (1/σ²_A,j) (β_YG,j − Σ_k A_k β_Xk,j)²
//!   p = pchisq(Q_valid, df = n − p − 1, lower.tail = FALSE)
//! ```
//! Observed heterogeneity is evidence of a violation of the exclusion
//! restriction (horizontal pleiotropy).

use crate::error::{MvmrError, Result};
use crate::format::MvmrInput;
use crate::ivw::ivw_mvmr;
use crate::linalg::pchisq_upper;

/// Output of [`pleiotropy_mvmr`].
#[derive(Debug, Clone)]
pub struct PleiotropyResult {
    /// Cochran's Q statistic for instrument validity.
    pub q_stat: f64,
    /// Two-sided p-value from the upper chi-square tail.
    pub q_pval: f64,
    /// Residual degrees of freedom `n − p − 1`.
    pub df: f64,
}

/// Compute the validity Q statistic with scalar `gencov` (default 0).
pub fn pleiotropy_mvmr(input: &MvmrInput, _gencov: f64) -> Result<PleiotropyResult> {
    input.validate()?;
    let n = input.n_snps();
    let p = input.n_exposures();

    // IVW estimate.
    let ivw = ivw_mvmr(input)?;
    let a = &ivw.estimate;

    let betas = input.beta_xg_matrix(); // n × p
    let sebetas = input.sebeta_xg_matrix(); // n × p

    // σ²_A = sebetaYG² + Σ_k A_k² sebetaXk²  (scalar-gencov branch; the R
    // package emits a warning that validity statistics ignore non-zero gencov,
    // and we follow that convention).
    let mut q_valid = 0.0;
    for row in 0..n {
        let mut sigma2a = input.sebeta_yg[row].powi(2);
        for k in 0..p {
            sigma2a += a[k].powi(2) * sebetas[row][k].powi(2);
        }
        let mut temp_sub2 = 0.0;
        for k in 0..p {
            temp_sub2 += betas[row][k] * a[k];
        }
        let resid = input.beta_yg[row] - temp_sub2;
        q_valid += resid.powi(2) / sigma2a;
    }

    let df = (n as f64) - (p as f64) - 1.0;
    let q_pval = pchisq_upper(q_valid, df);

    Ok(PleiotropyResult {
        q_stat: q_valid,
        q_pval,
        df,
    })
}

/// Variant using per-SNP exposure-effect covariance matrices
/// (`is.list(gencov)` branch in R): each `covlist[j]` is a `p × p` covariance
/// matrix, and `σ²_A,j = sebetaYG,j² + Aᵀ Σ_j A`.
pub fn pleiotropy_mvmr_cov(
    input: &MvmrInput,
    covlist: &[Vec<Vec<f64>>],
) -> Result<PleiotropyResult> {
    input.validate()?;
    let n = input.n_snps();
    let p = input.n_exposures();
    if covlist.len() != n {
        return Err(MvmrError::LengthMismatch(format!(
            "covlist len {} != n {n}",
            covlist.len()
        )));
    }
    let ivw = ivw_mvmr(input)?;
    let a = &ivw.estimate;
    let betas = input.beta_xg_matrix();

    let mut q_valid = 0.0;
    for row in 0..n {
        let cov = &covlist[row];
        let mut sigma2a = input.sebeta_yg[row].powi(2);
        // Aᵀ Σ A.
        for k in 0..p {
            for l in 0..p {
                sigma2a += a[k] * cov[k][l] * a[l];
            }
        }
        let mut temp_sub2 = 0.0;
        for k in 0..p {
            temp_sub2 += betas[row][k] * a[k];
        }
        let resid = input.beta_yg[row] - temp_sub2;
        q_valid += resid.powi(2) / sigma2a;
    }
    let df = (n as f64) - (p as f64) - 1.0;
    let q_pval = pchisq_upper(q_valid, df);
    Ok(PleiotropyResult {
        q_stat: q_valid,
        q_pval,
        df,
    })
}
