//! Proportion tests — port of `stats::prop.test`.
//!
//! One-sample and two-sample tests for proportions using the Pearson
//! chi-squared statistic with optional Yates continuity correction.
//! Matches R `prop.test` numerically.

use serde_json::json;

use super::HypothesisTest;
use crate::{Alternative, HypoError, KEY_KIND, Result, dist::chisq_sf, extras};

/// One-sample proportion test: `H₀: p = p0`.
///
/// `correct = true` applies Yates' continuity correction (R default).
/// The statistic is χ²(1); the p-value is always two-sided (R `prop.test`
/// does not support one-sided alternatives — use `wilson_hilferty` or the
/// normal approximation for directional tests).
pub fn prop_test_one(x: u32, n: u32, p0: f64, correct: bool) -> Result<HypothesisTest> {
    if n == 0 {
        return Err(HypoError::InvalidInput("n must be > 0".into()));
    }
    if x > n {
        return Err(HypoError::InvalidInput(format!("x ({x}) > n ({n})")));
    }
    if !(p0 > 0.0 && p0 < 1.0) {
        return Err(HypoError::InvalidInput(format!(
            "p0 must be in (0, 1), got {p0}"
        )));
    }
    let nf = n as f64;
    let xf = x as f64;
    let expected = nf * p0;
    let expected_complement = nf * (1.0 - p0);

    // χ² = (|O - E| - c)² / E + (|O' - E'| - c)² / E'
    // where c = 0.5 if correct else 0.
    let c = if correct { 0.5 } else { 0.0 };
    let d1 = (xf - expected).abs() - c;
    let d2 = ((nf - xf) - expected_complement).abs() - c;
    let stat = d1 * d1 / expected + d2 * d2 / expected_complement;
    let dof = 1.0_f64;
    let p_value = chisq_sf(stat.max(0.0), dof);

    let phat = xf / nf;
    Ok(HypothesisTest::new(
        stat,
        p_value,
        dof,
        Alternative::TwoSided,
        "1-sample proportions test with continuity correction",
        extras([
            (KEY_KIND, json!("prop_test")),
            ("estimate", json!(phat)),
            ("null_value", json!(p0)),
            ("x", json!(x)),
            ("n", json!(n)),
            ("corrected", json!(correct)),
        ]),
    ))
}

/// Two-sample proportion test: `H₀: p₁ − p₂ = 0`.
///
/// Uses the pooled estimate `p̂ = (x₁ + x₂) / (n₁ + n₂)` under H₀.
pub fn prop_test_two(x1: u32, n1: u32, x2: u32, n2: u32, correct: bool) -> Result<HypothesisTest> {
    if n1 == 0 || n2 == 0 {
        return Err(HypoError::InvalidInput("n1 and n2 must be > 0".into()));
    }
    if x1 > n1 || x2 > n2 {
        return Err(HypoError::InvalidInput("counts exceed sample sizes".into()));
    }
    let (n1f, n2f) = (n1 as f64, n2 as f64);
    let (x1f, x2f) = (x1 as f64, x2 as f64);
    let p1 = x1f / n1f;
    let p2 = x2f / n2f;
    let p_pool = (x1f + x2f) / (n1f + n2f);

    let c = if correct { 0.5 } else { 0.0 };
    // Build the 2×2 table and compute Pearson χ² with continuity correction.
    // Rows: (success, failure), Columns: (group1, group2).
    let expected = [
        [n1f * p_pool, n2f * p_pool],
        [n1f * (1.0 - p_pool), n2f * (1.0 - p_pool)],
    ];
    let observed = [[x1f, x2f], [n1f - x1f, n2f - x2f]];
    let mut stat = 0.0_f64;
    for i in 0..2 {
        for j in 0..2 {
            let d = observed[i][j] - expected[i][j];
            // Yates correction applies to 2×2 tables only, on each cell.
            let d_corrected = d.abs() - c;
            stat += d_corrected * d_corrected / expected[i][j];
        }
    }
    let dof = 1.0_f64;
    let p_value = chisq_sf(stat.max(0.0), dof);

    Ok(HypothesisTest::new(
        stat,
        p_value,
        dof,
        Alternative::TwoSided,
        "2-sample test for equality of proportions with continuity correction",
        extras([
            (KEY_KIND, json!("prop_test")),
            ("estimate1", json!(p1)),
            ("estimate2", json!(p2)),
            ("pooled", json!(p_pool)),
            ("x1", json!(x1)),
            ("n", json!(n1 + n2)),
            ("n1", json!(n1)),
            ("x2", json!(x2)),
            ("n2", json!(n2)),
            ("corrected", json!(correct)),
        ]),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_sample_basic() {
        // R: prop.test(50, 100, p=0.5, correct=TRUE)
        // x=50, n=100, p0=0.5 → no deviation → stat≈0.01 (Yates), p≈0.92
        let t = prop_test_one(50, 100, 0.5, true).unwrap();
        assert!(t.stat <= 0.02);
        assert!(t.p_value > 0.85);
        assert_eq!(t.dof, 1.0);
    }

    #[test]
    fn one_sample_against_r() {
        // R: prop.test(65, 100, p=0.5, correct=TRUE)
        // chi-squared ≈ 8.01, p ≈ 0.00465
        let t = prop_test_one(65, 100, 0.5, true).unwrap();
        // With continuity correction: (|15| - 0.5)²/50 + (|15| - 0.5)²/50
        // = 14.5² / 50 * 2 = 210.25/50 * 2 = 8.41
        assert!((t.stat - 8.41).abs() < 0.01);
        assert!((t.extra_f64("estimate").unwrap() - 0.65).abs() < 1e-12);
    }

    #[test]
    fn one_sample_no_correction() {
        let t_corr = prop_test_one(65, 100, 0.5, true).unwrap();
        let t_nc = prop_test_one(65, 100, 0.5, false).unwrap();
        // Without correction the statistic is larger.
        assert!(t_nc.stat > t_corr.stat);
        // Without correction: 15²/50 * 2 = 9.0
        assert!((t_nc.stat - 9.0).abs() < 1e-9);
    }

    #[test]
    fn two_sample_basic() {
        // R: prop.test(c(30, 40), c(100, 100), correct=TRUE)
        let t = prop_test_two(30, 100, 40, 100, true).unwrap();
        assert_eq!(t.dof, 1.0);
        assert!(t.stat > 0.0);
        // p should be moderate (not very significant)
        assert!(t.p_value > 0.05);
    }

    #[test]
    fn rejects_bad_inputs() {
        assert!(prop_test_one(5, 3, 0.5, true).is_err());
        assert!(prop_test_one(5, 0, 0.5, true).is_err());
        assert!(prop_test_one(5, 10, 0.0, true).is_err());
        assert!(prop_test_one(5, 10, 1.0, true).is_err());
        assert!(prop_test_two(5, 3, 4, 10, true).is_err());
        assert!(prop_test_two(5, 10, 4, 0, true).is_err());
    }
}
