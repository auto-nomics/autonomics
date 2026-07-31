//! Input processing: correlation-matrix estimation (Eq. 6 of the paper) and
//! GWAS summary-statistics harmonisation.
//!
//! ## Correlation matrix R (Eq. 6)
//! The correlation between test statistics for two traits is estimated from
//! the summary statistics across all common SNPs via Pearson correlation:
//!
//! ```text
//!   corr(T1, T2) = Σ_i (T_i1 - m1)(T_i2 - m2)
//!                  ────────────────────────────
//!                  √[Σ_i (T_i1-m1)² · Σ_i (T_i2-m2)²]
//! ```
//!
//! This is exactly R's `cor(X)` when X has no missing values.

use faer::Mat;

/// Pearson correlation matrix from an M×K matrix of summary statistics.
///
/// Port of R's `cor(X)` (column-wise Pearson correlation, pairwise complete
/// obs when `na.rm = TRUE`). Missing values (NaN) are dropped pairwise.
///
/// - `x` — M×K matrix (M SNPs × K traits).
/// Returns a K×K correlation matrix.
pub fn corr_matrix(x: &Mat<f64>) -> Mat<f64> {
    let m = x.nrows();
    let k = x.ncols();
    let mut r = Mat::identity(k, k);

    for a in 0..k {
        for b in (a + 1)..k {
            // Pairwise complete observations
            let pairs: Vec<(f64, f64)> = (0..m)
                .filter_map(|i| {
                    let va = x[(i, a)];
                    let vb = x[(i, b)];
                    if va.is_nan() || vb.is_nan() {
                        None
                    } else {
                        Some((va, vb))
                    }
                })
                .collect();
            let c = if pairs.len() < 2 {
                f64::NAN
            } else {
                let n = pairs.len() as f64;
                let ma: f64 = pairs.iter().map(|(v, _)| v).sum::<f64>() / n;
                let mb: f64 = pairs.iter().map(|(_, v)| v).sum::<f64>() / n;
                let mut sxy = 0.0;
                let mut sxx = 0.0;
                let mut syy = 0.0;
                for (va, vb) in &pairs {
                    let da = va - ma;
                    let db = vb - mb;
                    sxy += da * db;
                    sxx += da * da;
                    syy += db * db;
                }
                if sxx == 0.0 || syy == 0.0 {
                    f64::NAN
                } else {
                    sxy / (sxx * syy).sqrt()
                }
            };
            r[(a, b)] = c;
            r[(b, a)] = c;
        }
    }
    r
}

/// A single SNP's summary statistics across K traits/cohort-trait combinations.
#[derive(Clone, Debug)]
pub struct SnpStats {
    /// SNP identifier (e.g. rsid).
    pub id: String,
    /// Z-scores / test statistics for K traits (length K).
    pub stats: Vec<f64>,
}

/// Result of a CPASSOC analysis for a single SNP.
#[derive(Clone, Debug)]
pub struct CpassocResult {
    pub id: String,
    /// SHom statistic.
    pub shom: f64,
    /// SHet statistic.
    pub shet: f64,
    /// SHom p-value (χ²₁).
    pub shom_p: f64,
    /// SHet p-value (shifted gamma).
    pub shet_p: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corr_identity() {
        let x = Mat::from_fn(5, 2, |i, j| (i + j) as f64);
        let r = corr_matrix(&x);
        // Perfect positive correlation between column 0 (0,1,2,3,4)
        // and column 1 (1,2,3,4,5)
        assert!((r[(0, 1)] - 1.0).abs() < 1e-10);
    }

    #[test]
    fn corr_anticorrelated() {
        let x = Mat::from_fn(5, 2, |i, j| if j == 0 { i as f64 } else { -(i as f64) });
        let r = corr_matrix(&x);
        assert!((r[(0, 1)] - (-1.0)).abs() < 1e-10);
    }
}
