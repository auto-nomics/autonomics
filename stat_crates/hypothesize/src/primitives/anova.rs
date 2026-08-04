//! One-way ANOVA — port of `stats::aov` and `stats::oneway.test`.
//!
//! Classical (equal-variance) F-test and Welch's heteroscedastic F-test for
//! k-sample mean equality.

use serde_json::json;

use super::HypothesisTest;
use crate::{Alternative, HypoError, KEY_KIND, Result, dist::f_sf, extras};

/// One-way ANOVA.
///
/// `var_equal = true` → classical ANOVA F-test (`aov`).
/// `var_equal = false` → Welch's heteroscedastic F-test (`oneway.test`).
pub fn oneway_anova(groups: &[&[f64]], var_equal: bool) -> Result<HypothesisTest> {
    let k = groups.len();
    if k < 2 {
        return Err(HypoError::InvalidInput(
            "one-way ANOVA: need ≥ 2 groups".into(),
        ));
    }
    let group_n: Vec<usize> = groups.iter().map(|g| g.len()).collect();
    for &ni in &group_n {
        if ni < 2 {
            return Err(HypoError::InvalidInput(
                "one-way ANOVA: each group needs ≥ 2 observations".into(),
            ));
        }
    }
    let n_total: usize = group_n.iter().sum();
    let group_means: Vec<f64> = groups.iter().map(|g| crate::extras_mean(g)).collect();
    let grand_mean: f64 = {
        let total_sum: f64 = groups
            .iter()
            .zip(&group_n)
            .map(|(g, _)| g.iter().sum::<f64>())
            .sum();
        total_sum / n_total as f64
    };

    if var_equal {
        classical_anova(&group_means, &group_n, groups, grand_mean, n_total, k)
    } else {
        welch_anova(&group_means, &group_n, groups, grand_mean, k)
    }
}

fn classical_anova(
    group_means: &[f64],
    group_n: &[usize],
    groups: &[&[f64]],
    grand_mean: f64,
    n_total: usize,
    k: usize,
) -> Result<HypothesisTest> {
    let (nf, kf) = (n_total as f64, k as f64);

    // Between-group sum of squares.
    let ss_between: f64 = group_means
        .iter()
        .zip(group_n)
        .map(|(gm, &gn)| gn as f64 * (gm - grand_mean).powi(2))
        .sum();

    // Within-group sum of squares.
    let ss_within: f64 = groups
        .iter()
        .zip(group_means)
        .map(|(g, gm)| g.iter().map(|&x| (x - gm).powi(2)).sum::<f64>())
        .sum();

    let df1 = kf - 1.0;
    let df2 = nf - kf;
    let msb = ss_between / df1;
    let msw = ss_within / df2;
    let f = msb / msw;
    let p_value = f_sf(f, df1, df2);

    Ok(HypothesisTest::new(
        f,
        p_value,
        df1,
        Alternative::TwoSided,
        "One-way ANOVA",
        extras([
            (KEY_KIND, json!("oneway_anova")),
            ("var_equal", json!(true)),
            ("k", json!(k as u64)),
            ("n_total", json!(n_total as u64)),
            ("df1", json!(df1)),
            ("df2", json!(df2)),
            ("ss_between", json!(ss_between)),
            ("ss_within", json!(ss_within)),
            ("ms_between", json!(msb)),
            ("ms_within", json!(msw)),
            ("group_means", json!(group_means)),
        ]),
    ))
}

fn welch_anova(
    group_means: &[f64],
    group_n: &[usize],
    groups: &[&[f64]],
    _grand_mean: f64,
    k: usize,
) -> Result<HypothesisTest> {
    let kf = k as f64;

    // Per-group variances and weights.
    let group_vars: Vec<f64> = groups.iter().map(|g| crate::extras_var(g)).collect();
    let weights: Vec<f64> = group_n
        .iter()
        .zip(&group_vars)
        .map(|(&ni, vi)| ni as f64 / vi)
        .collect();
    let w_sum: f64 = weights.iter().sum();
    let weighted_mean: f64 = group_means
        .iter()
        .zip(&weights)
        .map(|(gm, w)| gm * w)
        .sum::<f64>()
        / w_sum;

    // Welch F statistic: F = [Σ wᵢ(ȳᵢ−ȳ_w)²/(k−1)] / [1 + 2(k−2)/(k²−1)·C]
    let numerator: f64 = group_means
        .iter()
        .zip(&weights)
        .map(|(gm, w)| w * (gm - weighted_mean).powi(2))
        .sum::<f64>()
        / (kf - 1.0);

    // Correction term C = Σ (1 − wᵢ/W)² / (nᵢ − 1).
    let c: f64 = (0..k)
        .map(|i| {
            let ni = group_n[i] as f64;
            (1.0 - weights[i] / w_sum).powi(2) / (ni - 1.0)
        })
        .sum();
    let denominator = 1.0 + 2.0 * (kf - 2.0) / (kf * kf - 1.0) * c;

    let f = numerator / denominator;

    // Welch–Satterthwaite df2.
    let df1 = kf - 1.0;
    let df2 = (kf * kf - 1.0) / (3.0 * c);

    let p_value = f_sf(f, df1, df2);

    Ok(HypothesisTest::new(
        f,
        p_value,
        df1,
        Alternative::TwoSided,
        "Welch one-way ANOVA",
        extras([
            (KEY_KIND, json!("oneway_anova")),
            ("var_equal", json!(false)),
            ("k", json!(k as u64)),
            ("df1", json!(df1)),
            ("df2", json!(df2)),
            ("group_means", json!(group_means)),
            ("weighted_grand_mean", json!(weighted_mean)),
        ]),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classical_anova_equal_means() {
        let g1 = [1.0_f64, 2.0, 3.0, 4.0, 5.0];
        let g2 = [1.0_f64, 2.0, 3.0, 4.0, 5.0];
        let g3 = [1.0_f64, 2.0, 3.0, 4.0, 5.0];
        let t = oneway_anova(&[&g1, &g2, &g3], true).unwrap();
        // All means equal → F ≈ 0
        assert!(t.stat < 0.5, "F = {}", t.stat);
        assert!((t.dof - 2.0).abs() < 1e-12);
        assert!(t.extra_f64("df2").unwrap() - 12.0 < 1e-9);
    }

    #[test]
    fn classical_anova_different_means() {
        let g1 = [1.0_f64, 2.0, 3.0];
        let g2 = [4.0_f64, 5.0, 6.0];
        let g3 = [7.0_f64, 8.0, 9.0];
        let t = oneway_anova(&[&g1, &g2, &g3], true).unwrap();
        // Clear between-group differences → F large
        assert!(t.stat > 10.0, "F = {}", t.stat);
        assert!(t.p_value < 0.01);
    }

    #[test]
    fn welch_anova_basic() {
        let g1 = [1.0_f64, 2.0, 3.0, 4.0, 5.0];
        let g2 = [3.0_f64, 4.0, 5.0, 6.0, 7.0];
        let t = oneway_anova(&[&g1, &g2], false).unwrap();
        assert!((t.dof - 1.0).abs() < 1e-9);
        assert!(t.extra_f64("df2").unwrap() > 0.0);
    }

    #[test]
    fn rejects_bad_inputs() {
        assert!(oneway_anova(&[&[1.0_f64][..]], true).is_err());
        assert!(oneway_anova(&[&[1.0_f64][..]], false).is_err());
    }
}
