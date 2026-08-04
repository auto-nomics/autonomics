//! Covariance-matrix builders — faithful ports of `R/snpcov_mvmr.R` and
//! `R/phenocov_mvmr.R`.
//!
//! These produce the per-SNP `p × p` covariance matrices for the
//! instrument-exposure effects that the `is.list(gencov)` branches of
//! [`strength`](crate::strength), [`pleiotropy`](crate::pleiotropy) and
//! [`strhet`](crate::strhet) consume.

/// Build per-SNP covariance matrices from a phenotypic correlation matrix and
/// the exposure standard errors, faithful to `phenocov_mvmr(pcor, seBXGs)`.
///
/// Each returned matrix is `pcor ∘ (se_row ⊗ se_row)` (the Hadamard product of
/// the correlation matrix with the outer product of the per-row SEs).
pub fn phenocov_mvmr(pcor: &[Vec<f64>], sebetaxgs: &[Vec<f64>]) -> Vec<Vec<Vec<f64>>> {
    let n = sebetaxgs.len();
    let p = sebetaxgs.first().map(|r| r.len()).unwrap_or(0);
    let mut out = vec![vec![vec![0.0; p]; p]; n];
    for l in 0..n {
        for a in 0..p {
            for b in 0..p {
                out[l][a][b] = pcor[a][b] * sebetaxgs[l][a] * sebetaxgs[l][b];
            }
        }
    }
    out
}

/// Build per-SNP covariance matrices from individual-level genotype matrix
/// `Gs` (n_obs × n_snp) and exposure matrix `Xs` (n_obs × p), faithful to
/// `snpcov_mvmr(Gs, Xs)`.
///
/// For each SNP `i` and each pair of exposures `(j, k)`, the matrix element is
/// ```text
///   Σ_i = ((Gᵢᵀ G_i)^{-1} / n_obs) · Σ_obs resid_{i,j} · resid_{i,k}
/// ```
/// where `resid_{i,j}` are the residuals from regressing `X_j` on `G_i`.
pub fn snpcov_mvmr(gs: &[Vec<f64>], xs: &[Vec<f64>]) -> Vec<Vec<Vec<f64>>> {
    let n_obs = gs.len();
    let n_snp = gs.first().map(|r| r.len()).unwrap_or(0);
    let p = xs.first().map(|r| r.len()).unwrap_or(0);

    // Residuals of each exposure on each SNP (univariate OLS through intercept).
    // resmat layout (matching R): column index = i + (k-1)*nG for exposure k,
    // SNP i (1-indexed in R). We store as [exposure_k][snp_i][obs].
    let mut resid: Vec<Vec<Vec<f64>>> = vec![vec![vec![0.0; n_obs]; n_snp]; p];
    for j in 0..p {
        for i in 0..n_snp {
            let g: Vec<f64> = (0..n_obs).map(|row| gs[row][i]).collect();
            let xj: Vec<f64> = (0..n_obs).map(|row| xs[row][j]).collect();
            let (intercept, slope) = ols_with_intercept(&g, &xj);
            for row in 0..n_obs {
                resid[j][i][row] = xj[row] - (intercept + slope * g[row]);
            }
        }
    }

    let mut sigmalist = vec![vec![vec![0.0; p]; p]; n_snp];
    for i in 0..n_snp {
        // (Gᵀ G)^{-1} / n_obs — a scalar per SNP.
        let mut gtg = 0.0;
        for row in 0..n_obs {
            gtg += gs[row][i].powi(2);
        }
        let base = (1.0 / gtg) / n_obs as f64;

        for j in 0..p {
            for k in 0..p {
                let mut dot = 0.0;
                for row in 0..n_obs {
                    dot += resid[j][i][row] * resid[k][i][row];
                }
                sigmalist[i][j][k] = base * dot;
            }
        }
    }
    sigmalist
}

/// Simple OLS with intercept: returns `(intercept, slope)` minimising
/// `Σ (y_i − a − b·x_i)²`.
fn ols_with_intercept(x: &[f64], y: &[f64]) -> (f64, f64) {
    let n = x.len() as f64;
    let mean_x: f64 = x.iter().sum::<f64>() / n;
    let mean_y: f64 = y.iter().sum::<f64>() / n;
    let mut cov = 0.0;
    let mut var = 0.0;
    for i in 0..x.len() {
        cov += (x[i] - mean_x) * (y[i] - mean_y);
        var += (x[i] - mean_x).powi(2);
    }
    let slope = if var.abs() > 1e-300 { cov / var } else { 0.0 };
    let intercept = mean_y - slope * mean_x;
    (intercept, slope)
}
