//! Legacy combined IVW + strength + validity driver — faithful port of
//! `R/mvmr.R`.
//!
//! The legacy `mvmr()` function fits the IVW multivariable model and returns
//! the coefficient table together with two diagnostic Q statistics:
//!
//! * `Q_strength` — per-exposure modified Cochran's Q for instrument strength
//!   (with a non-zero `gencov` subtracted from `σ²_{xi}` in this code path; the
//!   F-statistic analogue lives in [`strength`](crate::strength)).
//! * `Q_valid` — instrument-validity Cochran's Q, plus its chi-square p-value.
//!
//! **Note:** as in the R source, the strength statistics here are *not* divided
//! by `n` — they are raw Q values, intended to be compared against the F ≥ 10
//! heuristic only after the caller applies that normalisation.

use crate::error::Result;
use crate::format::MvmrInput;
use crate::ivw::IvwResult;
use crate::linalg::pchisq_upper;

/// Output of [`mvmr`] — the legacy combined result object.
#[derive(Debug, Clone)]
pub struct MvmrLegacyResult {
    /// IVW coefficient table (shared with [`ivw_mvmr`](crate::ivw_mvmr)).
    pub ivw: IvwResult,
    /// Per-exposure Q-statistic for instrument strength (not divided by n).
    pub q_strength: Vec<f64>,
    /// Cochran's Q statistic for instrument validity.
    pub q_valid: f64,
    /// Upper-tail chi-square p-value for `q_valid` on `n − p − 1` df.
    pub p_valid: f64,
}

/// Fit the legacy combined IVW + strength + validity MVMR model.
///
/// `gencov` defaults to `0.0`; a non-zero scalar is subtracted from each
/// strength `σ²_{xi}` exactly as in the R reference (see `R/mvmr.R` lines
/// 144–151).
pub fn mvmr(input: &MvmrInput, gencov: f64) -> Result<MvmrLegacyResult> {
    input.validate()?;
    let n = input.n_snps();
    let p = input.n_exposures();

    let ivw = crate::ivw::ivw_mvmr(input)?;
    let a = &ivw.estimate;

    let betas = input.beta_xg_matrix();
    let sebetas = input.sebeta_xg_matrix();

    // ── Q_strength: δ from unweighted OLS of each exposure on the others ──
    let mut delta_mat = vec![vec![0.0; p]; p - 1];
    for i in 0..p {
        let y: Vec<f64> = (0..n).map(|row| betas[row][i]).collect();
        let x: Vec<Vec<f64>> = (0..n)
            .map(|row| (0..p).filter(|&c| c != i).map(|c| betas[row][c]).collect())
            .collect();
        let coef = crate::linalg::ols_origin(&x, &y)?;
        for k in 0..(p - 1) {
            delta_mat[k][i] = coef[k];
        }
    }

    // σ²_{xi,j} = Σ_{k≠i} δ²·se(β_Xk)² + se(β_Xi)² − gencov.
    let mut sigma2xj = vec![vec![0.0; p]; n];
    for i in 0..p {
        for row in 0..n {
            let mut acc = 0.0;
            let mut k_index = 0;
            for c in 0..p {
                if c == i {
                    continue;
                }
                acc += sebetas[row][c].powi(2) * delta_mat[k_index][i].powi(2);
                k_index += 1;
            }
            acc += sebetas[row][i].powi(2) - gencov;
            sigma2xj[row][i] = acc;
        }
    }

    let mut q_strength = vec![0.0; p];
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
        q_strength[i] = acc;
    }

    // ── Q_valid (validity), same formula as pleiotropy_mvmr scalar branch ──
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
    let p_valid = pchisq_upper(q_valid, df);

    Ok(MvmrLegacyResult {
        ivw,
        q_strength,
        q_valid,
        p_valid,
    })
}
