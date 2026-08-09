//! Conditional F-statistic for instrument strength — faithful port of
//! `R/strength_mvmr.R`.
//!
//! For each exposure `i`, regress that exposure's instrument-effect estimates
//! on the other exposures' estimates (unweighted OLS through the origin) to
//! obtain contrast coefficients δ, then build the conditional instrument
//! strength statistic
//! ```text
//!   F_i = (1/n) · Σ_j ( β_Xi,j − Σ_{k≠i} δ_{k,i} β_Xk,j )² / σ²_{xi,j}
//! ```
//! where, in the default `gencov = 0` case, `σ²_{xi,j} = se(β_Xi,j)² +
//! Σ_{k≠i} δ_{k,i}² se(β_Xk,j)²`. A conventional F ≥ 10 threshold indicates
//! sufficient conditional strength.

use crate::error::{MvmrError, Result};
use crate::format::MvmrInput;
use crate::linalg::ols_origin;

/// Output of [`strength_mvmr`].
#[derive(Debug, Clone)]
pub struct StrengthResult {
    /// Per-exposure conditional F-statistic.
    pub f_statistic: Vec<f64>,
    /// Per-exposure conditional Q-statistic (F × n).
    pub q_statistic: Vec<f64>,
}

/// Compute the conditional F-statistic (`strength_mvmr`) with scalar `gencov`.
///
/// **Note:** unlike the legacy [`mvmr`](crate::mvmr) function, R's
/// `strength_mvmr` does **not** subtract a scalar `gencov` from `σ²_{xi}` in
/// its non-list code path (see `R/strength_mvmr.R` line 88). The `gencov`
/// argument is retained only for the `is.list(gencov)` branch, where per-SNP
/// covariance matrices are used directly. We faithfully reproduce this: the
/// scalar `gencov` is silently ignored here, matching R.
pub fn strength_mvmr(input: &MvmrInput, _gencov: f64) -> Result<StrengthResult> {
    input.validate()?;
    let n = input.n_snps();
    let p = input.n_exposures();

    let betas = input.beta_xg_matrix(); // n × p
    let sebetas = input.sebeta_xg_matrix(); // n × p

    // delta_mat is (p-1) × p: column i holds the OLS coefficients of exposure i
    // regressed on the remaining exposures (through origin, unweighted).
    let mut delta_mat = vec![vec![0.0; p]; p - 1];
    for i in 0..p {
        // Regressand: betas[:, i]. Regressors: betas without column i.
        let y: Vec<f64> = (0..n).map(|row| betas[row][i]).collect();
        let x: Vec<Vec<f64>> = (0..n)
            .map(|row| (0..p).filter(|&c| c != i).map(|c| betas[row][c]).collect())
            .collect();
        let coef = ols_origin(&x, &y)?;
        for k in 0..(p - 1) {
            delta_mat[k][i] = coef[k];
        }
    }

    // σ²_{xi,j} (n × p). Default gencov = 0 path.
    let mut sigma2xj = vec![vec![0.0; p]; n];
    for i in 0..p {
        for row in 0..n {
            let mut acc = 0.0;
            // Σ_{k≠i} δ_{k,i}² se(β_Xk)²
            let mut k_index = 0;
            for c in 0..p {
                if c == i {
                    continue;
                }
                acc += sebetas[row][c].powi(2) * delta_mat[k_index][i].powi(2);
                k_index += 1;
            }
            acc += sebetas[row][i].powi(2);
            sigma2xj[row][i] = acc;
        }
    }

    // Q_strength[i] = Σ (1/σ²_{xi}) (β_Xi − Σ δ·β_X)²
    let mut q = vec![0.0; p];
    for i in 0..p {
        let mut acc = 0.0;
        for row in 0..n {
            let mut temp_sub = 0.0;
            let mut k_index = 0;
            for c in 0..p {
                if c == i {
                    continue;
                }
                temp_sub += delta_mat[k_index][i] * betas[row][c];
                k_index += 1;
            }
            let resid = betas[row][i] - temp_sub;
            acc += resid.powi(2) / sigma2xj[row][i];
        }
        q[i] = acc;
    }

    // strength_mvmr divides Q by n to report the F-statistic.
    let f_statistic: Vec<f64> = q.iter().map(|&v| v / n as f64).collect();

    Ok(StrengthResult {
        f_statistic,
        q_statistic: q,
    })
}

/// Variant of [`strength_mvmr`] that takes per-SNP covariance matrices for the
/// exposure effects (the `is.list(gencov)` branch in R). Each `covlist[j]` is a
/// `p × p` covariance matrix for instrument `j`.
pub fn strength_mvmr_cov(input: &MvmrInput, covlist: &[Vec<Vec<f64>>]) -> Result<StrengthResult> {
    input.validate()?;
    let n = input.n_snps();
    let p = input.n_exposures();
    if covlist.len() != n {
        return Err(MvmrError::LengthMismatch(format!(
            "covlist len {} != n {n}",
            covlist.len()
        )));
    }

    let betas = input.beta_xg_matrix();

    // δ coefficients (same OLS-through-origin as the scalar branch).
    let mut delta_mat = vec![vec![0.0; p]; p - 1];
    for i in 0..p {
        let y: Vec<f64> = (0..n).map(|row| betas[row][i]).collect();
        let x: Vec<Vec<f64>> = (0..n)
            .map(|row| (0..p).filter(|&c| c != i).map(|c| betas[row][c]).collect())
            .collect();
        let coef = ols_origin(&x, &y)?;
        for k in 0..(p - 1) {
            delta_mat[k][i] = coef[k];
        }
    }

    // Build the per-exposure contrast vector v with -1 at position i and the
    // δ coefficients elsewhere, then σ²_{xi,j} = vᵀ Σ_j v.
    let mut sigma2xj = vec![vec![0.0; p]; n];
    for i in 0..p {
        let mut v = vec![0.0; p];
        v[i] = -1.0;
        let mut k_index = 0;
        for c in 0..p {
            if c == i {
                continue;
            }
            v[c] = delta_mat[k_index][i];
            k_index += 1;
        }
        for row in 0..n {
            let cov = &covlist[row]; // p × p
            let mut s = 0.0;
            for a in 0..p {
                for b in 0..p {
                    s += v[a] * cov[a][b] * v[b];
                }
            }
            sigma2xj[row][i] = s;
        }
    }

    let mut q = vec![0.0; p];
    for i in 0..p {
        let mut acc = 0.0;
        for row in 0..n {
            let mut temp_sub = 0.0;
            let mut k_index = 0;
            for c in 0..p {
                if c == i {
                    continue;
                }
                temp_sub += delta_mat[k_index][i] * betas[row][c];
                k_index += 1;
            }
            let resid = betas[row][i] - temp_sub;
            acc += resid.powi(2) / sigma2xj[row][i];
        }
        q[i] = acc;
    }

    let f_statistic: Vec<f64> = q.iter().map(|&v| v / n as f64).collect();
    Ok(StrengthResult {
        f_statistic,
        q_statistic: q,
    })
}
