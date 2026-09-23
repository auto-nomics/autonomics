//! Variance tests — port of `stats::var.test` (F-test for ratio of two
//! variances), `bartlett.test`, `car::leveneTest`, `fligner.test`.
//!
//! Tests `H₀: σ₁²/σ₂² = ratio₀` using the F statistic
//! `F = (s₁²/s₂²) / ratio₀ ~ F(n₁−1, n₂−1)`.

use serde_json::json;

use super::HypothesisTest;
use crate::{
    Alternative, HypoError, KEY_KIND, Result,
    dist::{chisq_sf, f_sf},
    extras,
};

/// F-test comparing two variances: `H₀: σ₁²/σ₂² = ratio₀`.
///
/// Mirrors `var.test(x, y, ratio = ratio₀, alternative = alt)`.
pub fn var_test(x: &[f64], y: &[f64], ratio: f64, alt: Alternative) -> Result<HypothesisTest> {
    let (n1, n2) = (x.len(), y.len());
    if n1 < 2 || n2 < 2 {
        return Err(HypoError::InvalidInput(
            "variance test requires ≥ 2 observations per group".into(),
        ));
    }
    if ratio <= 0.0 {
        return Err(HypoError::InvalidInput(format!(
            "ratio must be positive, got {ratio}"
        )));
    }
    let v1 = crate::extras_var(x);
    let v2 = crate::extras_var(y);
    let df1 = (n1 - 1) as f64;
    let df2 = (n2 - 1) as f64;
    let f = (v1 / v2) / ratio;

    let p_value = match alt {
        Alternative::TwoSided => {
            // 2 * min(P(F ≥ f), P(F ≤ f))
            let upper = f_sf(f, df1, df2);
            let lower = 1.0 - f_sf(f, df1, df2); // CDF(f) = 1 - sf(f) = P(F ≤ f)
            2.0 * upper.min(lower).min(1.0)
        }
        Alternative::Greater => f_sf(f, df1, df2),
        Alternative::Less => 1.0 - f_sf(f, df1, df2),
    };

    Ok(HypothesisTest::new(
        f,
        p_value,
        df1, // primary df; df2 stored in extras
        alt,
        "F test to compare two variances",
        extras([
            (KEY_KIND, json!("var_test")),
            ("estimate", json!(v1 / v2)),
            ("null_value", json!(ratio)),
            ("df1", json!(df1)),
            ("df2", json!(df2)),
            ("n", json!((n1 + n2) as u64)),
            ("n1", json!(n1 as u64)),
            ("n2", json!(n2 as u64)),
        ]),
    ))
}

// ─── Bartlett's test (parametric k-sample variance equality) ────────────────

/// Bartlett's test for homogeneity of variances across `k` groups.
///
/// Tests `H₀: σ₁² = σ₂² = … = σₖ²`. Sensitive to non-normality.
///
/// Mirrors `bartlett.test`.
pub fn bartlett_test(groups: &[&[f64]]) -> Result<HypothesisTest> {
    let k = groups.len();
    if k < 2 {
        return Err(HypoError::InvalidInput("Bartlett: need ≥ 2 groups".into()));
    }
    let mut n_total = 0_usize;
    let mut sp2_num = 0.0_f64; // numerator of pooled variance
    let mut log_terms = 0.0_f64; // Σ (n_i - 1) * ln(s_i²)
    let mut group_stats: Vec<(usize, f64)> = Vec::with_capacity(k); // (n_i, s_i²)

    for g in groups {
        let ni = g.len();
        if ni < 2 {
            return Err(HypoError::InvalidInput(
                "Bartlett: each group needs ≥ 2 observations".into(),
            ));
        }
        let vi = crate::extras_var(g);
        n_total += ni;
        sp2_num += (ni - 1) as f64 * vi;
        group_stats.push((ni, vi));
    }

    let n = n_total as f64;
    let df_pool = n - k as f64;
    let sp2 = sp2_num / df_pool;

    for &(ni, vi) in &group_stats {
        log_terms += (ni - 1) as f64 * vi.ln();
    }

    let chi2_raw = df_pool * sp2.ln() - log_terms;
    // Bartlett's correction factor C.
    let c = 1.0
        + (1.0 / (3.0 * (k - 1) as f64))
            * (group_stats
                .iter()
                .map(|(ni, _)| 1.0 / (ni - 1) as f64)
                .sum::<f64>()
                - 1.0 / df_pool);
    let chi2 = chi2_raw / c;
    let df = (k - 1) as f64;
    let p_value = chisq_sf(chi2, df);

    Ok(HypothesisTest::new(
        chi2,
        p_value,
        df,
        Alternative::TwoSided,
        "Bartlett test of homogeneity of variances",
        extras([
            (KEY_KIND, json!("bartlett_test")),
            ("k", json!(k as u64)),
            ("n", json!(n_total as u64)),
            ("n_total", json!(n_total as u64)),
            ("pooled_var", json!(sp2)),
        ]),
    ))
}

// ─── Levene / Brown–Forsythe ────────────────────────────────────────────────

/// Levene's test for homogeneity of variances.
///
/// `center = Center::Mean` gives the classic Levene (1960);
/// `center = Center::Median` gives Brown–Forsythe (1974), which is more
/// robust to non-normality.
///
/// Mirrors `car::leveneTest`.
pub fn levene_test(groups: &[&[f64]], center: Center) -> Result<HypothesisTest> {
    let k = groups.len();
    if k < 2 {
        return Err(HypoError::InvalidInput("Levene: need ≥ 2 groups".into()));
    }
    // Compute group centers and absolute deviations.
    let mut all_devs: Vec<(f64, usize)> = Vec::new();
    let mut group_means: Vec<f64> = Vec::with_capacity(k);
    let mut group_n: Vec<usize> = Vec::with_capacity(k);

    for (gi, g) in groups.iter().enumerate() {
        let ni = g.len();
        if ni < 2 {
            return Err(HypoError::InvalidInput(
                "Levene: each group needs ≥ 2 observations".into(),
            ));
        }
        let center_val = match center {
            Center::Mean => crate::extras_mean(g),
            Center::Median => median(g),
        };
        let devs: Vec<f64> = g.iter().map(|&x| (x - center_val).abs()).collect();
        let dev_mean = crate::extras_mean(&devs);
        for &d in &devs {
            all_devs.push((d, gi));
        }
        group_means.push(dev_mean);
        group_n.push(ni);
    }

    let n_total = all_devs.len();
    let nf = n_total as f64;
    let kf = k as f64;
    let grand_mean: f64 = all_devs.iter().map(|(d, _)| *d).sum::<f64>() / nf;

    // Between-group sum of squares.
    let ss_between: f64 = group_means
        .iter()
        .zip(&group_n)
        .map(|(gm, &gn)| gn as f64 * (gm - grand_mean).powi(2))
        .sum();

    // Within-group sum of squares.
    let ss_within: f64 = all_devs
        .iter()
        .map(|(d, gi)| (d - group_means[*gi]).powi(2))
        .sum();

    let df1 = kf - 1.0;
    let df2 = nf - kf;
    if df2 <= 0.0 || ss_within <= 0.0 {
        return Err(HypoError::InvalidInput(
            "Levene: insufficient degrees of freedom".into(),
        ));
    }
    let f = (ss_between / df1) / (ss_within / df2);
    let p_value = crate::dist::f_sf(f, df1, df2);

    let method = match center {
        Center::Mean => "Levene's Test for Homogeneity of Variance",
        Center::Median => "Brown-Forsythe Test for Homogeneity of Variance",
    };

    Ok(HypothesisTest::new(
        f,
        p_value,
        df1,
        Alternative::TwoSided,
        method,
        extras([
            (KEY_KIND, json!("levene_test")),
            ("center", json!(format!("{center:?}").to_lowercase())),
            ("k", json!(k as u64)),
            ("n", json!(n_total as u64)),
            ("n_total", json!(n_total as u64)),
            ("df1", json!(df1)),
            ("df2", json!(df2)),
        ]),
    ))
}

/// Center type for Levene's test.
#[derive(
    Clone, Copy, PartialEq, Eq, Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Center {
    Mean,
    Median,
}

/// Sample median.
fn median(v: &[f64]) -> f64 {
    let mut sorted = v.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = sorted.len();
    if n % 2 == 0 {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    } else {
        sorted[n / 2]
    }
}

// ─── Fligner–Killeen test ───────────────────────────────────────────────────

/// Fligner–Killeen test for homogeneity of variances (rank-based, robust).
///
/// Centers each group by its median, ranks the absolute centered values,
/// and computes a χ² statistic. Default in R for non-normal data.
///
/// Mirrors `fligner.test`.
pub fn fligner_test(groups: &[&[f64]]) -> Result<HypothesisTest> {
    let k = groups.len();
    if k < 2 {
        return Err(HypoError::InvalidInput("Fligner: need ≥ 2 groups".into()));
    }
    // Center each group by its overall median, then rank |centered| values.
    let all_medians: Vec<f64> = groups.iter().map(|g| median(g)).collect();

    let mut centered: Vec<(f64, usize)> = Vec::new();
    for (gi, g) in groups.iter().enumerate() {
        for &x in *g {
            centered.push(((x - all_medians[gi]).abs(), gi));
        }
    }
    centered.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let n = centered.len();
    let nf = n as f64;

    // Rank with average ties.
    let vals: Vec<f64> = centered.iter().map(|(v, _)| *v).collect();
    let ranks = crate::extras_rank_average(&vals);

    // Normal scores: a_ij = (rank - (n+1)/2) / sqrt((n²-1)/12)
    let a_const = ((nf.powi(2) - 1.0) / 12.0).sqrt();
    let scores: Vec<f64> = ranks
        .iter()
        .map(|&r| (r - (nf + 1.0) / 2.0) / a_const)
        .collect();

    // Sum of scores per group, and overall sum of squares.
    let mut group_a_bar: Vec<f64> = vec![0.0; k];
    let mut group_ns: Vec<usize> = vec![0; k];
    for (i, (_, gi)) in centered.iter().enumerate() {
        group_a_bar[*gi] += scores[i];
        group_ns[*gi] += 1;
    }
    for gi in 0..k {
        group_a_bar[gi] /= group_ns[gi] as f64;
    }

    let sum_a2: f64 = scores.iter().map(|a| a * a).sum();
    let sum_ni_a2: f64 = group_a_bar
        .iter()
        .zip(&group_ns)
        .map(|(ab, &ni)| ni as f64 * ab * ab)
        .sum();

    let stat = (nf - 1.0) * sum_ni_a2 / sum_a2;
    let df = (k - 1) as f64;
    let p_value = chisq_sf(stat, df);

    Ok(HypothesisTest::new(
        stat,
        p_value,
        df,
        Alternative::TwoSided,
        "Fligner-Killeen test of homogeneity of variances",
        extras([
            (KEY_KIND, json!("fligner_test")),
            ("k", json!(k as u64)),
            ("n", json!(n as u64)),
            ("n_total", json!(n as u64)),
            ("group_sizes", json!(group_ns)),
        ]),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_variances() {
        // Two samples from the same distribution → F ≈ 1, p ≈ 1
        let x: Vec<f64> = (1..=10).map(|i| i as f64).collect();
        let y: Vec<f64> = (1..=10).map(|i| (i as f64) + 5.0).collect();
        // Shifting doesn't change variance → F=1
        let t = var_test(&x, &y, 1.0, Alternative::TwoSided).unwrap();
        assert!((t.stat - 1.0).abs() < 1e-12);
        assert!((t.dof - 9.0).abs() < 1e-12);
    }

    #[test]
    fn known_f_statistic() {
        // x = 1..6 (var=2.5), y = 1..11 (var=9.1667)
        let x: Vec<f64> = (1..=5).map(|i| i as f64).collect();
        let y: Vec<f64> = (1..=10).map(|i| i as f64).collect();
        let t = var_test(&x, &y, 1.0, Alternative::TwoSided).unwrap();
        let v1 = crate::extras_var(&x);
        let v2 = crate::extras_var(&y);
        assert!((t.stat - v1 / v2).abs() < 1e-9);
        assert!((t.extra_f64("df1").unwrap() - 4.0).abs() < 1e-9);
        assert!((t.extra_f64("df2").unwrap() - 9.0).abs() < 1e-9);
    }

    #[test]
    fn rejects_bad_inputs() {
        assert!(var_test(&[1.0], &[1.0, 2.0], 1.0, Alternative::TwoSided).is_err());
        assert!(var_test(&[1.0, 2.0], &[1.0], 1.0, Alternative::TwoSided).is_err());
        assert!(var_test(&[1.0, 2.0], &[1.0, 2.0], 0.0, Alternative::TwoSided).is_err());
        assert!(var_test(&[1.0, 2.0], &[1.0, 2.0], -1.0, Alternative::TwoSided).is_err());
    }
}
