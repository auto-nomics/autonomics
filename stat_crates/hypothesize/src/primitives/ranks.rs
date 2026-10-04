//! Rank-based nonparametric tests — Wilcoxon signed-rank, Mann–Whitney U,
//! Kruskal–Wallis, Friedman.
//!
//! All asymptotic branches use the same tie correction and continuity
//! correction as R `wilcox.test`, `kruskal.test`, `friedman.test`.

use serde_json::json;

use super::HypothesisTest;
use crate::{
    Alternative, HypoError, KEY_KIND, Result,
    dist::{chisq_sf, normal_cdf, normal_two_sided_p},
    extras,
};

/// Zero-value handling method for Wilcoxon signed-rank (R `zero.method`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ZeroMethod {
    /// Drop zeros entirely (R default `wilcox`).
    Wilcox,
    /// Treat zeros as positive (`pratt` — keeps them, halves rank).
    Pratt,
    /// Assign zeros a rank of 0 (`zsplit`).
    Zsplit,
}

/// Tie-correction factor for rank statistics: `C = 1 - Σ(t³ᵢ - tᵢ) / (N³ - N)`.
fn tie_correction(tie_groups: &[usize]) -> f64 {
    let n: f64 = tie_groups.iter().map(|&t| t as f64).sum();
    if n <= 1.0 {
        return 1.0;
    }
    let sum_t3_t: f64 = tie_groups
        .iter()
        .map(|&t| {
            let tf = t as f64;
            tf.powi(3) - tf
        })
        .sum();
    1.0 - sum_t3_t / (n.powi(3) - n)
}

// ─── Wilcoxon signed-rank test ──────────────────────────────────────────────

/// One-sample Wilcoxon signed-rank test: `H₀: median(x) = mu0`.
///
/// Uses asymptotic normal approximation with continuity correction.
/// For `correct = true` (R default), applies a 0.5 continuity correction.
pub fn wilcoxon_signed_rank(
    x: &[f64],
    mu0: f64,
    alt: Alternative,
    zero_method: ZeroMethod,
    correct: bool,
) -> Result<HypothesisTest> {
    let d: Vec<f64> = x.iter().map(|&xi| xi - mu0).collect();
    let nonzero: Vec<f64> = match zero_method {
        ZeroMethod::Wilcox => d.into_iter().filter(|&di| di != 0.0).collect(),
        ZeroMethod::Pratt | ZeroMethod::Zsplit => d,
    };
    let n = nonzero.len();
    if n < 1 {
        return Err(HypoError::InvalidInput(
            "Wilcoxon signed-rank: insufficient non-zero observations".into(),
        ));
    }

    // Rank by absolute value (average ties).
    let abs_vals: Vec<f64> = nonzero.iter().map(|d| d.abs()).collect();
    let ranks = rank_average(&abs_vals);

    // W+ = sum of ranks for positive differences.
    let w_plus: f64 = nonzero
        .iter()
        .zip(&ranks)
        .filter(|(d, _)| **d > 0.0)
        .map(|(_, r)| *r)
        .sum();

    // Tie groups among |d_i|.
    let tie_groups = compute_tie_groups(&abs_vals);

    let nf = n as f64;
    let mean_w = nf * (nf + 1.0) / 4.0;
    let tie_c = tie_correction(&tie_groups);
    let var_w = nf * (nf + 1.0) * (2.0 * nf + 1.0) / 24.0 * tie_c;

    let z = if var_w > 0.0 {
        let c = if correct { 0.5 } else { 0.0 };
        let numerator = match alt {
            Alternative::TwoSided => (w_plus - mean_w).abs().max(0.0) - c,
            Alternative::Greater => (w_plus - mean_w) - c,
            Alternative::Less => (mean_w - w_plus) - c,
        };
        numerator / var_w.sqrt()
    } else {
        0.0
    };

    let p_value = match alt {
        Alternative::TwoSided => normal_two_sided_p(z),
        Alternative::Greater => 1.0 - normal_cdf(z),
        Alternative::Less => normal_cdf(z),
    };

    Ok(HypothesisTest::new(
        w_plus,
        p_value,
        f64::INFINITY,
        alt,
        "Wilcoxon signed rank test",
        extras([
            (KEY_KIND, json!("wilcoxon_signed_rank")),
            ("estimate", json!(w_plus)),
            ("n", json!(n as u64)),
            ("mu", json!(mu0)),
            ("corrected", json!(correct)),
            ("z", json!(z)),
        ]),
    ))
}

// ─── Mann–Whitney U test ────────────────────────────────────────────────────

/// Two-sample Mann–Whitney U / Wilcoxon rank-sum test.
///
/// Tests whether one sample is stochastically larger than the other.
pub fn mann_whitney(
    x: &[f64],
    y: &[f64],
    alt: Alternative,
    correct: bool,
) -> Result<HypothesisTest> {
    let (n1, n2) = (x.len(), y.len());
    if n1 == 0 || n2 == 0 {
        return Err(HypoError::InvalidInput(
            "Mann-Whitney: both samples must be non-empty".into(),
        ));
    }

    // Pool and rank.
    let mut pooled: Vec<(f64, bool)> = x
        .iter()
        .map(|&v| (v, true))
        .chain(y.iter().map(|&v| (v, false)))
        .collect();
    pooled.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let vals: Vec<f64> = pooled.iter().map(|(v, _)| *v).collect();
    let ranks = rank_average(&vals);

    // Sum of ranks for group x.
    let r1: f64 = pooled
        .iter()
        .zip(&ranks)
        .filter(|((_, is_x), _)| *is_x)
        .map(|(_, r)| *r)
        .sum();

    let (n1f, n2f) = (n1 as f64, n2 as f64);
    let u1 = r1 - n1f * (n1f + 1.0) / 2.0;
    let u2 = n1f * n2f - u1;
    let u = u1.min(u2);

    // Tie correction.
    let tie_groups = compute_tie_groups(&vals);
    let tie_c = tie_correction(&tie_groups);
    let n_total = n1f + n2f;
    let mean_u = n1f * n2f / 2.0;
    let var_u = n1f * n2f * (n_total + 1.0) / 12.0 * tie_c;

    // One-sided tests are directional on U₁ (x vs y): "greater" means x is
    // stochastically larger than y. Only the two-sided test folds U₁ and U₂
    // together via min(U₁, U₂); for it |U₁ − E[U]| is numerically equivalent.
    let c = if correct { 0.5 } else { 0.0 };
    let z = if var_u > 0.0 {
        let numerator = match alt {
            Alternative::TwoSided => (u1 - mean_u).abs().max(0.0) - c,
            Alternative::Less => (u1 - mean_u) + c,
            Alternative::Greater => (u1 - mean_u) - c,
        };
        numerator / var_u.sqrt()
    } else {
        0.0
    };

    let p_value = match alt {
        Alternative::TwoSided => normal_two_sided_p(z),
        Alternative::Less => normal_cdf(z),
        Alternative::Greater => 1.0 - normal_cdf(z),
    };

    Ok(HypothesisTest::new(
        u,
        p_value,
        f64::INFINITY,
        alt,
        "Wilcoxon rank sum exact test",
        extras([
            (KEY_KIND, json!("mann_whitney")),
            ("W", json!(r1)), // R's W = rank sum for x
            ("U", json!(u)),
            ("n", json!((n1 + n2) as u64)),
            ("n1", json!(n1 as u64)),
            ("n2", json!(n2 as u64)),
            ("corrected", json!(correct)),
            ("z", json!(z)),
        ]),
    ))
}

// ─── Kruskal–Wallis test ────────────────────────────────────────────────────

/// Kruskal–Wallis rank-sum test for k independent samples.
///
/// `groups` is a slice of slices; tests whether all k samples come from the
/// same distribution.
pub fn kruskal_wallis(groups: &[&[f64]]) -> Result<HypothesisTest> {
    let k = groups.len();
    if k < 2 {
        return Err(HypoError::InvalidInput(
            "Kruskal-Wallis: need ≥ 2 groups".into(),
        ));
    }
    let n_total: usize = groups.iter().map(|g| g.len()).sum();
    if n_total < 2 {
        return Err(HypoError::InvalidInput(
            "total sample size must be ≥ 2".into(),
        ));
    }

    // Pool and rank.
    let mut pooled: Vec<(f64, usize)> = Vec::with_capacity(n_total);
    for (gi, group) in groups.iter().enumerate() {
        for &v in *group {
            pooled.push((v, gi));
        }
    }
    pooled.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let vals: Vec<f64> = pooled.iter().map(|(v, _)| *v).collect();
    let ranks = rank_average(&vals);

    // Sum of ranks per group.
    let n_f = n_total as f64;
    let mut rank_sums = vec![0.0_f64; k];
    let mut group_sizes = vec![0_usize; k];
    for (entry, &r) in pooled.iter().zip(&ranks) {
        rank_sums[entry.1] += r;
        group_sizes[entry.1] += 1;
    }

    // H statistic.
    let mut h = 0.0_f64;
    for i in 0..k {
        let ni = group_sizes[i] as f64;
        h += rank_sums[i].powi(2) / ni;
    }
    h = 12.0 / (n_f * (n_f + 1.0)) * h - 3.0 * (n_f + 1.0);

    // Tie correction.
    let tie_groups = compute_tie_groups(&vals);
    let tie_c = tie_correction(&tie_groups);
    if tie_c > 0.0 {
        h /= tie_c;
    }

    let df = (k - 1) as f64;
    let p_value = chisq_sf(h, df);

    Ok(HypothesisTest::new(
        h,
        p_value,
        df,
        Alternative::TwoSided,
        "Kruskal-Wallis rank sum test",
        extras([
            (KEY_KIND, json!("kruskal_wallis")),
            ("k", json!(k as u64)),
            ("n", json!(n_total as u64)),
            ("n_total", json!(n_total as u64)),
            ("rank_sums", json!(rank_sums)),
            ("group_sizes", json!(group_sizes)),
            ("tie_correction", json!(tie_c)),
        ]),
    ))
}

// ─── Friedman test ──────────────────────────────────────────────────────────

/// Friedman chi-squared for `b` blocks × `t` treatments.
///
/// `data[block][treatment]` is the observed value. Tests whether the `t`
/// treatments have identical effects.
pub fn friedman_test(data: &[&[f64]]) -> Result<HypothesisTest> {
    let b = data.len();
    if b < 1 {
        return Err(HypoError::InvalidInput("Friedman: need ≥ 1 block".into()));
    }
    let t = data[0].len();
    if t < 2 {
        return Err(HypoError::InvalidInput(
            "Friedman: need ≥ 2 treatments".into(),
        ));
    }
    for row in data {
        if row.len() != t {
            return Err(HypoError::LengthMismatch { a: t, b: row.len() });
        }
    }

    // Rank within each block.
    let mut rank_sums = vec![0.0_f64; t];
    for block in data {
        let ranks = rank_average(block);
        for (j, &r) in ranks.iter().enumerate() {
            rank_sums[j] += r;
        }
    }

    let (bf, tf) = (b as f64, t as f64);
    let mut chi2 = 0.0_f64;
    for j in 0..t {
        chi2 += rank_sums[j].powi(2);
    }
    chi2 = 12.0 / (bf * tf * (tf + 1.0)) * chi2 - 3.0 * bf * (tf + 1.0);

    let df = (t - 1) as f64;
    let p_value = chisq_sf(chi2, df);

    Ok(HypothesisTest::new(
        chi2,
        p_value,
        df,
        Alternative::TwoSided,
        "Friedman rank sum test",
        extras([
            (KEY_KIND, json!("friedman_test")),
            ("blocks", json!(b as u64)),
            ("treatments", json!(t as u64)),
            ("n", json!((b * t) as u64)),
            ("rank_sums", json!(rank_sums)),
        ]),
    ))
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Assign average ranks (R `rank(ties.method = "average")`).
fn rank_average(v: &[f64]) -> Vec<f64> {
    let n = v.len();
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by(|&a, &b| v[a].partial_cmp(&v[b]).unwrap_or(std::cmp::Ordering::Equal));
    let mut ranks = vec![0.0_f64; n];
    let mut i = 0;
    while i < n {
        let mut j = i + 1;
        while j < n && v[idx[j]] == v[idx[i]] {
            j += 1;
        }
        let avg = ((i + 1 + j) as f64) / 2.0;
        for k in i..j {
            ranks[idx[k]] = avg;
        }
        i = j;
    }
    ranks
}

/// Compute tie group sizes (sorted, only groups with size ≥ 1).
fn compute_tie_groups(v: &[f64]) -> Vec<usize> {
    let n = v.len();
    if n == 0 {
        return Vec::new();
    }
    let mut sorted = v.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut groups = Vec::new();
    let mut i = 0;
    while i < n {
        let mut j = i + 1;
        while j < n && sorted[j] == sorted[i] {
            j += 1;
        }
        groups.push(j - i);
        i = j;
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wilcoxon_signed_rank_symmetric() {
        // Symmetric data around 0 → W+ ≈ n(n+1)/4, z ≈ 0, p ≈ 1
        let x = vec![-2.0, -1.0, 0.5, 1.0, 2.5];
        let t =
            wilcoxon_signed_rank(&x, 0.0, Alternative::TwoSided, ZeroMethod::Wilcox, true).unwrap();
        assert!(t.stat > 0.0);
        assert!(t.p_value > 0.5, "p = {}", t.p_value);
    }

    #[test]
    fn wilcoxon_shifted() {
        // Shifted positive → mostly positive ranks → significant
        let x: Vec<f64> = (1..=20).map(|i| i as f64).collect();
        let t =
            wilcoxon_signed_rank(&x, 0.0, Alternative::Greater, ZeroMethod::Wilcox, true).unwrap();
        assert!(t.p_value < 0.001);
    }

    #[test]
    fn mann_whitney_disjoint() {
        // Completely separated groups → U = 0, significant
        let x = vec![10.0, 11.0, 12.0, 13.0, 14.0];
        let y = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let t = mann_whitney(&x, &y, Alternative::TwoSided, true).unwrap();
        assert!(t.p_value < 0.02, "p = {}", t.p_value);
        assert_eq!(t.extra_f64("n"), Some(10.0));
    }

    #[test]
    fn mann_whitney_identical() {
        // Same groups → U ≈ n1*n2/2, z ≈ 0, p ≈ 1
        let x = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let y = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let t = mann_whitney(&x, &y, Alternative::TwoSided, true).unwrap();
        assert!(t.p_value > 0.5, "p = {}", t.p_value);
    }

    #[test]
    fn mann_whitney_one_sided_direction() {
        // x is stochastically larger than y → "greater" significant, "less"
        // not. Regression: the old code derived both one-sided branches from
        // min(U₁, U₂), which is ≤ E[U] by construction, so every one-sided
        // p-value came out ≥ 0.5 regardless of direction.
        let x: Vec<f64> = (0..40).map(|i| i as f64 * 0.25 + 5.3).collect();
        let y: Vec<f64> = (0..40).map(|i| i as f64 * 0.25).collect();

        let g = mann_whitney(&x, &y, Alternative::Greater, true).unwrap();
        let l = mann_whitney(&x, &y, Alternative::Less, true).unwrap();
        let t = mann_whitney(&x, &y, Alternative::TwoSided, true).unwrap();
        assert!(g.p_value < 1e-4, "greater: {}", g.p_value);
        assert!(l.p_value > 0.999, "less: {}", l.p_value);
        assert!(t.p_value < 1e-4, "two-sided: {}", t.p_value);

        // Swapping the samples must swap which side is significant.
        let g2 = mann_whitney(&y, &x, Alternative::Greater, true).unwrap();
        let l2 = mann_whitney(&y, &x, Alternative::Less, true).unwrap();
        assert!(g2.p_value > 0.999, "greater swapped: {}", g2.p_value);
        assert!(l2.p_value < 1e-4, "less swapped: {}", l2.p_value);
    }

    #[test]
    fn kruskal_wallis_basic() {
        // Three groups with clear differences
        let g1 = [1.0_f64, 2.0, 3.0];
        let g2 = [4.0_f64, 5.0, 6.0];
        let g3 = [7.0_f64, 8.0, 9.0];
        let t = kruskal_wallis(&[&g1, &g2, &g3]).unwrap();
        assert!((t.dof - 2.0).abs() < 1e-12);
        assert_eq!(t.extra_f64("n"), Some(9.0));
        // Fully separated → H ≈ 7.2 (max), p very small
        assert!(t.p_value < 0.05, "p = {}", t.p_value);
    }

    #[test]
    fn kruskal_wallis_identical_groups() {
        let g1 = [1.0_f64, 2.0, 3.0];
        let g2 = [1.0_f64, 2.0, 3.0];
        let t = kruskal_wallis(&[&g1, &g2]).unwrap();
        assert!(t.stat < 0.5);
        assert!(t.p_value > 0.5);
    }

    #[test]
    fn friedman_basic() {
        // 4 blocks, 3 treatments with a clear treatment effect
        let b1 = [1.0_f64, 2.0, 3.0];
        let b2 = [1.0_f64, 2.0, 3.0];
        let b3 = [1.0_f64, 2.0, 3.0];
        let b4 = [1.0_f64, 2.0, 3.0];
        let t = friedman_test(&[&b1, &b2, &b3, &b4]).unwrap();
        assert_eq!(t.extra_f64("n"), Some(12.0)); // blocks × treatments
        // All blocks rank treatment 1 as best → χ² large
        assert!(t.stat > 5.0, "χ² = {}", t.stat);
        assert!((t.dof - 2.0).abs() < 1e-12);
        assert!(t.p_value < 0.1);
    }

    #[test]
    fn friedman_no_effect() {
        // Balanced design → treatment rank sums nearly equal
        let b1 = [3.0_f64, 1.0, 2.0];
        let b2 = [2.0_f64, 3.0, 1.0];
        let t = friedman_test(&[&b1, &b2]).unwrap();
        // χ² = 1.0, p = 0.607 — not significant
        assert!((t.stat - 1.0).abs() < 0.5, "χ² = {}", t.stat);
        assert!(t.p_value > 0.5, "p = {}", t.p_value);
    }

    #[test]
    fn rejects_bad_inputs() {
        assert!(
            wilcoxon_signed_rank(&[], 0.0, Alternative::TwoSided, ZeroMethod::Wilcox, true)
                .is_err()
        );
        assert!(mann_whitney(&[], &[1.0], Alternative::TwoSided, true).is_err());
        assert!(kruskal_wallis(&[&[1.0_f64][..]]).is_err());
        assert!(friedman_test(&[&[1.0_f64][..]]).is_err());
    }
}
