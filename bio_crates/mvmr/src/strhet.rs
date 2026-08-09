//! Conditional F-statistic via Q-minimisation (IRLS) — faithful port of
//! `R/strhet_mvmr.R`.
//!
//! Unlike [`strength_mvmr`](crate::strength), which fixes the δ coefficients
//! by unweighted OLS and computes the Q statistic once, `strhet_mvmr`
//! iteratively re-estimates δ by weighted least squares where the per-row
//! variance `σ²_{l} = vᵀ Σ_l v` itself depends on δ. This is the
//! iteratively-reweighted-least-squares scheme described in the R source.

use crate::error::{MvmrError, Result};
use crate::format::MvmrInput;
use crate::linalg::{ols_origin, wls_origin};

/// Output of [`strhet_mvmr`].
#[derive(Debug, Clone)]
pub struct StrhetResult {
    /// Per-exposure conditional F-statistic.
    pub f_statistic: Vec<f64>,
    /// Per-exposure minimised Q statistic (`F × n`).
    pub q_statistic: Vec<f64>,
}

/// Per-SNP covariance matrices: either diagonal (scalar `gencov = 0` path) or
/// full per-SNP matrices (the `is.list(gencov)` path).
fn build_covlist(
    input: &MvmrInput,
    gencov_list: Option<&[Vec<Vec<f64>>]>,
) -> Result<Vec<Vec<Vec<f64>>>> {
    let n = input.n_snps();
    let p = input.n_exposures();
    let sebetas = input.sebeta_xg_matrix();
    match gencov_list {
        Some(list) => {
            if list.len() != n {
                return Err(MvmrError::LengthMismatch(format!(
                    "gencov list len {} != n {n}",
                    list.len()
                )));
            }
            Ok(list.to_vec())
        }
        None => {
            // diag(sebetas[row, ]^2): the default two-sample case.
            let mut out = vec![vec![vec![0.0; p]; p]; n];
            for row in 0..n {
                for i in 0..p {
                    out[row][i][i] = sebetas[row][i].powi(2);
                }
            }
            Ok(out)
        }
    }
}

/// Compute the conditional F-statistic by minimising per-exposure Q statistics
/// via iteratively-reweighted least squares.
///
/// * `gencov_list` — `None` for the default `gencov = 0` (diagonal covariance
///   from the squared standard errors); or a per-SNP `p × p` covariance matrix
///   for each of the `n` instruments.
pub fn strhet_mvmr(
    input: &MvmrInput,
    gencov_list: Option<&[Vec<Vec<f64>>]>,
) -> Result<StrhetResult> {
    input.validate()?;
    let n = input.n_snps();
    let p = input.n_exposures();

    let betas = input.beta_xg_matrix(); // n × p
    let covlist = build_covlist(input, gencov_list)?;

    let mut qmin = vec![0.0; p];
    for m in 0..p {
        // y = betas[:, m]; X = betas without column m.
        let y: Vec<f64> = (0..n).map(|row| betas[row][m]).collect();
        let x: Vec<Vec<f64>> = (0..n)
            .map(|row| (0..p).filter(|&c| c != m).map(|c| betas[row][c]).collect())
            .collect();

        // IRLS: alternate updating δ (WLS) and the per-row σ² = vᵀ Σ v.
        let mut d = vec![0.0; p - 1];
        for _iter in 0..100 {
            // Build contrast vector v (-1 at position m, d elsewhere).
            let mut v = vec![0.0; p];
            v[m] = -1.0;
            for (k, c) in (0..p).filter(|&c| c != m).enumerate() {
                v[c] = d[k];
            }
            // σ²_row = vᵀ Σ_row v.
            let sig: Vec<f64> = (0..n)
                .map(|row| {
                    let cov = &covlist[row];
                    let mut s = 0.0;
                    for a in 0..p {
                        for b in 0..p {
                            s += v[a] * cov[a][b] * v[b];
                        }
                    }
                    s
                })
                .collect();
            // WLS with weights 1/σ².
            let w: Vec<f64> = sig.iter().map(|s| 1.0 / *s).collect();
            let d_new = match wls_origin(&x, &y, &w) {
                Ok(s) => s.coef,
                Err(_) => {
                    // Singular design inside IRLS — fall back to unweighted OLS.
                    ols_origin(&x, &y)?
                }
            };
            let max_delta = d
                .iter()
                .zip(d_new.iter())
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f64, f64::max);
            d = d_new;
            if max_delta < 1e-10 {
                break;
            }
        }

        // Final Q_min for exposure m.
        let mut v = vec![0.0; p];
        v[m] = -1.0;
        for (k, c) in (0..p).filter(|&c| c != m).enumerate() {
            v[c] = d[k];
        }
        let mut q = 0.0;
        for row in 0..n {
            let cov = &covlist[row];
            let mut s = 0.0;
            for a in 0..p {
                for b in 0..p {
                    s += v[a] * cov[a][b] * v[b];
                }
            }
            // Residual: y_row − X_row · d.
            let mut pred = 0.0;
            for (k, c) in (0..p).filter(|&c| c != m).enumerate() {
                pred += d[k] * betas[row][c];
            }
            let resid = betas[row][m] - pred;
            q += resid.powi(2) / s;
        }
        qmin[m] = q / n as f64;
    }

    let f_statistic = qmin.clone();
    let q_statistic: Vec<f64> = qmin.iter().map(|&f| f * n as f64).collect();
    Ok(StrhetResult {
        f_statistic,
        q_statistic,
    })
}
