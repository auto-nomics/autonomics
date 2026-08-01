//! Pearson χ² test for contingency tables, Bonferroni correction, and
//! pairwise comparisons.
//!
//! The paper uses χ² to compare group compositions across categorical
//! variables (Table 1, Table 3) and for pairwise incidence comparisons
//! with Bonferroni adjustment (Table 4).

use statrs::distribution::{ChiSquared, ContinuousCDF};

use crate::error::{EpiError, Result};

/// Result of a Pearson χ² test.
#[derive(Debug, Clone)]
pub struct ChiSqResult {
    /// The χ² test statistic `Σ (O−E)²/E`.
    pub chi_squared: f64,
    /// Degrees of freedom `(rows−1)(cols−1)`.
    pub df: usize,
    /// p-value from the χ² distribution.
    pub p_value: f64,
    /// Number of cells with expected count < 5 (a warning indicator).
    pub small_expected_count: usize,
}

/// Pearson χ² independence test on an `r×c` contingency table.
///
/// `counts[row][col]` is the observed cell frequency. The test computes
/// expected counts `E_ij = (row_total_i × col_total_j) / grand_total`,
/// the statistic `χ² = Σ (O−E)²/E`, and the p-value from
/// `χ²_{(r−1)(c−1)}`.
pub fn chi_squared_test(counts: &[Vec<u64>]) -> Result<ChiSqResult> {
    let r = counts.len();
    if r < 2 {
        return Err(EpiError::InvalidTable {
            rows: r,
            cols: counts.first().map(|c| c.len()).unwrap_or(0),
        });
    }
    let c = counts[0].len();
    if c < 2 {
        return Err(EpiError::InvalidTable { rows: r, cols: c });
    }
    for row in counts {
        if row.len() != c {
            return Err(EpiError::DimensionMismatch { a: c, b: row.len() });
        }
    }

    // Marginals.
    let row_totals: Vec<u64> = counts.iter().map(|row| row.iter().sum()).collect();
    let col_totals: Vec<u64> = (0..c)
        .map(|j| counts.iter().map(|row| row[j]).sum())
        .collect();
    let grand: u64 = row_totals.iter().sum();
    if grand == 0 {
        return Err(EpiError::EmptyInput);
    }

    // χ² statistic + count of small expected cells.
    let mut chi2 = 0.0_f64;
    let mut small = 0_usize;
    for i in 0..r {
        for j in 0..c {
            let expected = row_totals[i] as f64 * col_totals[j] as f64 / grand as f64;
            if expected < 5.0 {
                small += 1;
            }
            let diff = counts[i][j] as f64 - expected;
            chi2 += diff * diff / expected;
        }
    }

    let df = (r - 1) * (c - 1);
    let dist =
        ChiSquared::new(df as f64).map_err(|e| EpiError::Numerical(format!("ChiSquared: {e}")))?;
    let p_value = 1.0 - dist.cdf(chi2); // sf

    Ok(ChiSqResult {
        chi_squared: chi2,
        df,
        p_value,
        small_expected_count: small,
    })
}

/// Apply Bonferroni correction: multiply each p-value by the number of
/// comparisons, capping at 1.0.
pub fn bonferroni(p_values: &[f64]) -> Vec<f64> {
    let n = p_values.len() as f64;
    p_values.iter().map(|&p| (p * n).min(1.0)).collect()
}

/// Pairwise χ² comparisons between groups defined by `group_labels`, with
/// Bonferroni-adjusted p-values.
///
/// `group_labels[i]` identifies the group of observation `i`; `outcome[i]` is
/// 0 or 1. For each pair of groups, a 2×2 table is constructed and tested.
/// Returns one [`ChiSqResult`] per pair plus the corrected p-value.
pub fn pairwise_chi_squared(
    group_labels: &[usize],
    outcome: &[u64],
) -> Result<Vec<(usize, usize, ChiSqResult, f64)>> {
    let n_groups = *group_labels.iter().max().unwrap_or(&0) + 1;
    let mut results = Vec::new();

    // Build 2×2 tables for each pair.
    for a in 0..n_groups {
        for b in (a + 1)..n_groups {
            let mut table = vec![vec![0u64; 2]; 2]; // [group][outcome]
            for (i, &g) in group_labels.iter().enumerate() {
                if g == a || g == b {
                    let row = if g == a { 0 } else { 1 };
                    let col = outcome[i] as usize;
                    if col < 2 {
                        table[row][col] += 1;
                    }
                }
            }
            let res = chi_squared_test(&table)?;
            results.push((a, b, res.clone(), res.p_value));
        }
    }

    // Bonferroni correction.
    let raw_ps: Vec<f64> = results.iter().map(|(_, _, _, p)| *p).collect();
    let corrected = bonferroni(&raw_ps);
    for (r, adj) in results.iter_mut().zip(corrected) {
        r.3 = adj;
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * b.abs().max(1.0)
    }

    #[test]
    fn independence_2x2() {
        // Classic 2×2: Pearson χ² without Yates correction.
        // Table: [[8,2],[3,7]] → χ² ≈ 5.05, df=1, p ≈ 0.0246
        let table = vec![vec![8, 2], vec![3, 7]];
        let res = chi_squared_test(&table).unwrap();
        assert_eq!(res.df, 1);
        assert!(approx_eq(res.chi_squared, 5.051, 0.01));
        assert!(approx_eq(res.p_value, 0.0246, 0.001));
    }

    #[test]
    fn perfect_independence() {
        // Row proportions identical → χ² = 0, p = 1.
        let table = vec![vec![10, 20], vec![30, 60]];
        let res = chi_squared_test(&table).unwrap();
        assert!(res.chi_squared < 1e-9);
        assert!(approx_eq(res.p_value, 1.0, 1e-9));
    }

    #[test]
    fn strong_association_3x2() {
        // Clear association.
        let table = vec![vec![100, 0], vec![50, 50], vec![0, 100]];
        let res = chi_squared_test(&table).unwrap();
        assert_eq!(res.df, 2);
        assert!(res.p_value < 1e-10);
    }

    #[test]
    fn rejects_invalid_table() {
        assert!(chi_squared_test(&[vec![1, 2]]).is_err()); // 1 row
        assert!(chi_squared_test(&[vec![1], vec![2]]).is_err()); // 1 col
    }

    #[test]
    fn bonferroni_basic() {
        let ps = vec![0.01, 0.04, 0.03];
        let adj = bonferroni(&ps);
        assert!(approx_eq(adj[0], 0.03, 1e-9));
        assert!(approx_eq(adj[1], 0.12, 1e-9));
        assert!(approx_eq(adj[2], 0.09, 1e-9));
    }

    #[test]
    fn bonferroni_caps_at_one() {
        let ps = vec![0.5, 0.6];
        let adj = bonferroni(&ps);
        assert_eq!(adj, vec![1.0, 1.0]);
    }

    #[test]
    fn pairwise_bonferroni() {
        // 3 groups, outcome 0/1.
        let groups = vec![0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2];
        let outcome = vec![0, 0, 1, 1, 0, 0, 1, 1, 1, 1, 1, 1];
        let results = pairwise_chi_squared(&groups, &outcome).unwrap();
        assert_eq!(results.len(), 3); // 3 pairs
        for (_, _, res, adj) in &results {
            assert!(*adj >= 0.0 && *adj <= 1.0);
            assert_eq!(res.df, 1);
        }
    }
}
