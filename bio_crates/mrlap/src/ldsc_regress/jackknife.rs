//! Block jackknife over an ordinary least-squares regression — a faithful port
//! of LDSC's `LstsqJackknifeFast` (`ldscore/jackknife.py`).
//!
//! The data passed in (`x`, `y`) is already weighted — IRWLS bakes the weights
//! into the rows before calling here — so the jackknife is plain OLS. The "fast"
//! variant accumulates the per-block sufficient statistics `XᵀX` (p×p) and `Xᵀy`
//! (p) rather than the raw rows, which makes each leave-one-block-out solve a
//! cheap p×p system.

use faer::Mat;

use super::{LdscError, Result};

use super::linalg::{build_mat_col_major, solve_spd};

/// Jackknife output. Matches LDSC's `LstsqJackknifeFast` attributes.
pub struct JackknifeResult {
    /// Whole-data estimate (`jknife.est` = `block_values_to_est`). LDSC reports
    /// this as the point estimate. Length `p`.
    pub est: Vec<f64>,
    /// Pseudovalue-mean estimate (`jknife_est`). Length `p`. Equal to `est` only
    /// for exactly-linear statistics.
    pub jknife_est: Vec<f64>,
    /// Leave-one-block-out estimates (`delete_values`), `n_blocks × p`.
    pub delete_values: Vec<Vec<f64>>,
    /// Coefficient covariance `cov(pseudovalues, ddof=1) / n_blocks` (p×p).
    pub cov: Mat<f64>,
    /// `sqrt(diag(cov))` (length `p`).
    pub se: Vec<f64>,
}

/// Block separators: `floor(linspace(0, n, n_blocks + 1))` as integer cut points,
/// matching LDSC's `Jackknife.get_separators`. Returns `n_blocks + 1` strictly
/// non-decreasing indices in `[0, n]` with `sep[0] == 0` and `sep[last] == n`.
pub fn separators(n: usize, n_blocks: usize) -> Vec<usize> {
    (0..=n_blocks)
        .map(|i| {
            // floor(i * n / n_blocks) computed in f64 then truncated, identical
            // to numpy's floor(linspace).
            ((i as f64) * (n as f64) / (n_blocks as f64)).floor() as usize
        })
        .collect()
}

/// Run the fast block jackknife on the (already weighted) design `x` (n×p) and
/// response `y` (n). Rows are assumed to be in genomic order — the block
/// structure has meaning only if correlated SNPs fall in the same block.
pub fn jackknife_fast(x: &Mat<f64>, y: &[f64], n_blocks: usize) -> Result<JackknifeResult> {
    let n = y.len();
    if x.nrows() != n {
        return Err(LdscError::DimensionMismatch(
            "jackknife: x.nrows() != y.len()".into(),
        ));
    }
    if n_blocks == 0 || n_blocks > n {
        return Err(LdscError::InvalidInput(format!(
            "jackknife: need 1 <= n_blocks <= n; got n_blocks={n_blocks}, n={n}"
        )));
    }

    let sep = separators(n, n_blocks);
    jackknife_fast_with_separators(x, y, &sep)
}

/// Like [`jackknife_fast`] but with explicit block separators (length
/// `n_blocks + 1`, `sep[0] == 0`, `sep[last] == n`). Used by the two-step
/// estimator, which maps masked-space separators back to the full space.
pub fn jackknife_fast_with_separators(
    x: &Mat<f64>,
    y: &[f64],
    sep: &[usize],
) -> Result<JackknifeResult> {
    let n = y.len();
    let p = x.ncols();
    if x.nrows() != n {
        return Err(LdscError::DimensionMismatch(
            "jackknife: x.nrows() != y.len()".into(),
        ));
    }
    // Merge any empty blocks caused by floor (e.g. when n < n_blocks).
    let blocks: Vec<(usize, usize)> = sep
        .windows(2)
        .map(|w| (w[0], w[1]))
        .filter(|(a, b)| a < b)
        .collect();
    let nb = blocks.len();
    if nb == 0 {
        return Err(LdscError::InvalidInput(
            "jackknife: no non-empty blocks".into(),
        ));
    }

    // Per-block XᵀX (p×p, row-major flat) and Xᵀy (p).
    let mut xtx_blocks: Vec<Vec<f64>> = Vec::with_capacity(nb);
    let mut xty_blocks: Vec<Vec<f64>> = Vec::with_capacity(nb);
    for &(lo, hi) in &blocks {
        let mut xtx = vec![0.0; p * p];
        let mut xty = vec![0.0; p];
        for i in lo..hi {
            let yi = y[i];
            for a in 0..p {
                let xa = x[(i, a)];
                xty[a] += xa * yi;
                for b in a..p {
                    xtx[a * p + b] += xa * x[(i, b)];
                }
            }
        }
        // mirror lower triangle.
        for a in 0..p {
            for b in (a + 1)..p {
                xtx[b * p + a] = xtx[a * p + b];
            }
        }
        xtx_blocks.push(xtx);
        xty_blocks.push(xty);
    }

    // Totals.
    let mut xtx_tot = vec![0.0; p * p];
    let mut xty_tot = vec![0.0; p];
    for k in 0..nb {
        for t in 0..p * p {
            xtx_tot[t] += xtx_blocks[k][t];
        }
        for a in 0..p {
            xty_tot[a] += xty_blocks[k][a];
        }
    }

    // Whole-data estimate.
    let est = solve_spd(build_mat_col_major(p, p, &xtx_tot).as_ref(), &xty_tot)?;

    // Leave-one-block-out estimates and pseudovalues.
    let mut delete_values: Vec<Vec<f64>> = Vec::with_capacity(nb); // nb × p
    let mut pseudo: Vec<Vec<f64>> = Vec::with_capacity(nb); // nb × p
    for j in 0..nb {
        let mut del_xtx = xtx_tot.clone();
        let mut del_xty = xty_tot.clone();
        for t in 0..p * p {
            del_xtx[t] -= xtx_blocks[j][t];
        }
        for a in 0..p {
            del_xty[a] -= xty_blocks[j][a];
        }
        let del = solve_spd(build_mat_col_major(p, p, &del_xtx).as_ref(), &del_xty)?;
        delete_values.push(del.clone());
        let pseudo_j: Vec<f64> = (0..p)
            .map(|a| (nb as f64) * est[a] - ((nb - 1) as f64) * del[a])
            .collect();
        pseudo.push(pseudo_j);
    }

    // Pseudovalue-mean estimate.
    let mut jknife_est = vec![0.0; p];
    for j in 0..nb {
        for a in 0..p {
            jknife_est[a] += pseudo[j][a];
        }
    }
    for a in 0..p {
        jknife_est[a] /= nb as f64;
    }

    // Covariance: cov(pseudovalues, ddof=1) / nb.
    let mut cov = vec![0.0; p * p];
    if nb > 1 {
        for a in 0..p {
            for b in a..p {
                let s: f64 = (0..nb)
                    .map(|j| (pseudo[j][a] - jknife_est[a]) * (pseudo[j][b] - jknife_est[b]))
                    .sum();
                let v = s / ((nb - 1) as f64 * nb as f64);
                cov[a * p + b] = v;
                cov[b * p + a] = v;
            }
        }
    }
    let cov_mat = build_mat_col_major(p, p, &cov);
    let se: Vec<f64> = (0..p).map(|a| cov[a * p + a].max(0.0).sqrt()).collect();

    Ok(JackknifeResult {
        est,
        jknife_est,
        delete_values,
        cov: cov_mat,
        se,
    })
}

/// Output of [`ratio_jackknife`] — port of LDSC's `RatioJackknife`.
pub struct RatioJackknifeResult {
    /// Whole-data ratio estimate (`est` input), length `p`.
    pub est: Vec<f64>,
    /// Pseudovalue-mean estimate (`jknife_est`), length `p`.
    pub jknife_est: Vec<f64>,
    /// Covariance (p×p, row-major flat, length p²).
    pub cov: Vec<f64>,
    /// Standard errors (length `p`).
    pub se: Vec<f64>,
}

/// `RatioJackknife(est, numer_delete, denom_delete)` — block-jackknife of a
/// ratio. `est` length `p`; `numer_delete`/`denom_delete` `n_blocks × p`.
/// Pseudovalue `j = n*est - (n-1)*numer[j]/denom[j]`.
pub fn ratio_jackknife(
    est: &[f64],
    numer_delete: &[Vec<f64>],
    denom_delete: &[Vec<f64>],
) -> Result<RatioJackknifeResult> {
    let nb = numer_delete.len();
    if nb != denom_delete.len() {
        return Err(LdscError::DimensionMismatch(
            "ratio_jackknife: numer/denom length mismatch".into(),
        ));
    }
    let p = est.len();
    if nb == 0 {
        return Err(LdscError::InvalidInput("ratio_jackknife: no blocks".into()));
    }
    let mut pseudo = vec![vec![0.0; p]; nb];
    for j in 0..nb {
        for a in 0..p {
            let d = denom_delete[j][a];
            if d == 0.0 {
                return Err(LdscError::Numerical(
                    "ratio_jackknife: zero denominator delete value".into(),
                ));
            }
            pseudo[j][a] = (nb as f64) * est[a] - ((nb - 1) as f64) * numer_delete[j][a] / d;
        }
    }
    let mut jknife_est = vec![0.0; p];
    for j in 0..nb {
        for a in 0..p {
            jknife_est[a] += pseudo[j][a];
        }
    }
    for a in 0..p {
        jknife_est[a] /= nb as f64;
    }
    let mut cov = vec![0.0; p * p];
    if nb > 1 {
        for a in 0..p {
            for b in 0..p {
                let s: f64 = (0..nb)
                    .map(|j| (pseudo[j][a] - jknife_est[a]) * (pseudo[j][b] - jknife_est[b]))
                    .sum();
                cov[a * p + b] = s / ((nb - 1) as f64 * nb as f64);
            }
        }
    }
    let se: Vec<f64> = (0..p).map(|a| cov[a * p + a].max(0.0).sqrt()).collect();
    Ok(RatioJackknifeResult {
        est: est.to_vec(),
        jknife_est,
        cov,
        se,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldsc_regress::linalg::build_mat_row_major;

    fn approx(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * b.abs().max(1.0)
    }

    #[test]
    fn separators_match_numpy_linspace() {
        let s = separators(10, 4);
        assert_eq!(s, vec![0, 2, 5, 7, 10]);
    }

    #[test]
    fn jackknife_recovers_exact_linear_coefficients() {
        // y = 2 + 3·x, no noise → delete-values all equal the exact coef,
        // pseudovalues equal coef, SE = 0.
        let n = 20;
        let rows: Vec<Vec<f64>> = (0..n).map(|i| vec![1.0, i as f64]).collect();
        let x = build_mat_row_major(&rows);
        let y: Vec<f64> = (0..n).map(|i| 2.0 + 3.0 * (i as f64)).collect();
        let res = jackknife_fast(&x, &y, 5).unwrap();
        assert!(approx(res.est[0], 2.0, 1e-9), "intercept {}", res.est[0]);
        assert!(approx(res.est[1], 3.0, 1e-9), "slope {}", res.est[1]);
        assert!(res.se[0] < 1e-9, "se[0] {}", res.se[0]);
        assert!(res.se[1] < 1e-9, "se[1] {}", res.se[1]);
    }

    #[test]
    fn jackknife_se_positive_with_noise_structure() {
        // With blocks that differ, pseudovalues vary → positive SE.
        // Construct heteroscedastic-ish blocks so the leave-one-out coefs vary.
        let rows: Vec<Vec<f64>> = (0..16).map(|i| vec![1.0, (i as f64) + 1.0]).collect();
        // y mostly 3x but block 0 offset, block 2 offset.
        let y: Vec<f64> = (0..16)
            .map(|i| {
                let base = 3.0 * ((i as f64) + 1.0);
                base + if i < 4 { 5.0 } else { 0.0 }
            })
            .collect();
        let x = build_mat_row_major(&rows);
        let res = jackknife_fast(&x, &y, 4).unwrap();
        assert!(res.se.iter().all(|s| *s > 0.0));
        assert_eq!(res.cov.nrows(), 2);
        assert_eq!(res.cov.ncols(), 2);
    }
}
