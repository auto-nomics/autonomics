//! Cross-validation of all `hypothesize` sample-data primitives against
//! R-equivalent formulas (manual derivation from `stats::` documentation).
//!
//! Where R fixtures are available (`p.adjust` → `xval_padjust.rs`), this
//! crate's `xval_padjust.rs` covers them with golden vectors. The tests
//! here cover every other primitive by reconstructing the expected values
//! from the *formulas* documented in R — these are the same closed-form
//! expressions R uses internally for `t.test`, `prop.test`, `var.test`,
//! `cor.test`, `chisq.test`, `ks.test`, `fisher.test`, `wilcox.test`,
//! `kruskal.test`, `friedman.test`, `bartlett.test`, `car::leveneTest`,
//! `fligner.test`, `aov`, `oneway.test`, `nortest::ad.test`.
//!
//! Tolerance is `1e-9` for closed-form tests, `1e-4` for asymptotic p-values
//! (KS, Shapiro) and `1e-6` for chi-squared/F approximations.

use hypothesize as h;
use hypothesize::dist::*;

/// Two-sided t p-value shortcut.
fn t_two_sided(t: f64, df: f64) -> f64 {
    2.0 * t_sf(t.abs(), df)
}

// ─── helpers ────────────────────────────────────────────────────────────────

fn assert_close(a: f64, b: f64, tol: f64, ctx: &str) {
    assert!(
        (a - b).abs() <= tol,
        "{ctx}: got {a:.10}, expected {b:.10} (diff = {:.3e})",
        (a - b).abs()
    );
}

// ════ t_test ════════════════════════════════════════════════════════════════

#[test]
fn t_test_one_matches_r_pt() {
    // R: t.test(c(2.1,2.5,1.8,3.0,2.7,1.9,2.4,2.2), mu=2.0, alternative="two.sided")
    //   t = (x̄ - μ) / s/√n = 2.325 - 2.0) / (0.4232/√8) ≈ 2.171
    //   df = 7, p = 2·pt(-|t|, 7)
    let x = vec![2.1, 2.5, 1.8, 3.0, 2.7, 1.9, 2.4, 2.2];
    let t = h::t_test_one(&x, 2.0, h::Alternative::TwoSided, 0.95).unwrap();
    let mean: f64 = x.iter().sum::<f64>() / 8.0;
    let sd = (x.iter().map(|&xi| (xi - mean).powi(2)).sum::<f64>() / 7.0).sqrt();
    let t_expected = (mean - 2.0) / (sd / 8.0_f64.sqrt());
    assert_close(t.stat, t_expected, 1e-12, "t");
    assert_close(t.p_value, t_two_sided(t_expected.abs(), 7.0), 1e-12, "p");
}

#[test]
fn t_test_paired_matches_r_pt() {
    // R: t.test(c(2.5,3.5,3.2,4.0,4.8,5.5,5.4,6.2), c(2.1,2.9,3.3,4.1,4.7,5.4,5.2,6.0), paired=T)
    let a = vec![2.5, 3.5, 3.2, 4.0, 4.8, 5.5, 5.4, 6.2];
    let b = vec![2.1, 2.9, 3.3, 4.1, 4.7, 5.4, 5.2, 6.0];
    let t = h::t_test_paired(&a, &b, h::Alternative::TwoSided, 0.95).unwrap();
    let d: Vec<f64> = a.iter().zip(&b).map(|(p, q)| p - q).collect();
    let dm: f64 = d.iter().sum::<f64>() / d.len() as f64;
    let dvar: f64 = d.iter().map(|&di| (di - dm).powi(2)).sum::<f64>() / 7.0;
    let t_expected = dm / (dvar / 8.0).sqrt();
    assert_close(t.stat, t_expected, 1e-12, "t paired");
    assert_close(
        t.p_value,
        t_two_sided(t_expected.abs(), 7.0),
        1e-12,
        "p paired",
    );
}

#[test]
fn t_test_two_welch_matches_r_pt() {
    // R: t.test(c(2.1,2.5,1.8,3.0,2.7,1.9,2.4,2.2), c(3.0,3.5,3.2,4.0,4.8,5.5,5.4,6.2), var.equal=F)
    let x = vec![2.1, 2.5, 1.8, 3.0, 2.7, 1.9, 2.4, 2.2];
    let y = vec![3.0, 3.5, 3.2, 4.0, 4.8, 5.5, 5.4, 6.2];
    let t = h::t_test_two(&x, &y, false, h::Alternative::TwoSided, 0.95).unwrap();
    let (n1f, n2f) = (8.0_f64, 8.0_f64);
    let m1: f64 = x.iter().sum::<f64>() / n1f;
    let m2: f64 = y.iter().sum::<f64>() / n2f;
    let v1: f64 = x.iter().map(|&xi| (xi - m1).powi(2)).sum::<f64>() / (n1f - 1.0);
    let v2: f64 = y.iter().map(|&yi| (yi - m2).powi(2)).sum::<f64>() / (n2f - 1.0);
    let se = (v1 / n1f + v2 / n2f).sqrt();
    let t_expected = (m1 - m2) / se;
    let df_expected = (v1 / n1f + v2 / n2f).powi(2)
        / ((v1 / n1f).powi(2) / (n1f - 1.0) + (v2 / n2f).powi(2) / (n2f - 1.0));
    assert_close(t.stat, t_expected, 1e-12, "t welch");
    assert_close(t.dof, df_expected, 1e-12, "df welch");
    assert_close(
        t.p_value,
        t_two_sided(t_expected.abs(), df_expected),
        1e-12,
        "p welch",
    );
}

#[test]
fn t_test_two_pooled_matches_r_pt() {
    // R: t.test(x, y, var.equal=TRUE) — df = n1+n2-2
    let x = vec![2.1, 2.5, 1.8, 3.0, 2.7, 1.9, 2.4, 2.2];
    let y = vec![3.0, 3.5, 3.2, 4.0, 4.8, 5.5, 5.4, 6.2];
    let t = h::t_test_two(&x, &y, true, h::Alternative::TwoSided, 0.95).unwrap();
    assert!((t.dof - 14.0).abs() < 1e-12, "df should be n1+n2-2");
    let (m1, m2) = (x.iter().sum::<f64>() / 8.0, y.iter().sum::<f64>() / 8.0);
    let sp2: f64 = (x.iter().map(|&xi| (xi - m1).powi(2)).sum::<f64>()
        + y.iter().map(|&yi| (yi - m2).powi(2)).sum::<f64>())
        / 14.0;
    let t_expected = (m1 - m2) / (sp2 * (1.0 / 8.0 + 1.0 / 8.0)).sqrt();
    assert_close(t.stat, t_expected, 1e-12, "t pooled");
}

#[test]
fn t_test_directions_match_r() {
    // R: pt(t, df) for less; pt(-t, df) for greater; 2*pt(-|t|, df) for two-sided
    let x = vec![2.1, 2.5, 1.8, 3.0, 2.7, 1.9, 2.4, 2.2];
    let t = h::t_test_one(&x, 0.0, h::Alternative::TwoSided, 0.95).unwrap();
    let p_less = h::t_test_one(&x, 0.0, h::Alternative::Less, 0.95).unwrap();
    let p_greater = h::t_test_one(&x, 0.0, h::Alternative::Greater, 0.95).unwrap();
    assert_close(
        p_less.p_value + p_greater.p_value,
        1.0,
        1e-12,
        "less+greater=1",
    );
    assert_close(p_less.p_value, 1.0 - 0.5 * t.p_value, 1e-12, "two-sided/2");
}

// ════ prop_test ════════════════════════════════════════════════════════════

#[test]
fn prop_test_one_matches_r_chisq() {
    // R: prop.test(65, 100, p=0.5, correct=TRUE)
    // With Yates correction: χ² = (|15| − 0.5)²/50 + (|15| − 0.5)²/50 = 2·14.5²/50 = 8.41
    let t = h::prop_test_one(65, 100, 0.5, true).unwrap();
    assert_close(t.stat, 8.41, 1e-9, "χ²");
    assert_close(t.p_value, chisq_sf(8.41, 1.0), 1e-12, "p");
}

#[test]
fn prop_test_two_matches_r_chisq() {
    // R: prop.test(c(20, 30), c(100, 100), correct=TRUE) — symmetric table
    let t = h::prop_test_two(20, 100, 30, 100, true).unwrap();
    // Manual pooled χ²:
    let (x1, n1, x2, n2): (f64, f64, f64, f64) = (20.0, 100.0, 30.0, 100.0);
    let p_pool = (x1 + x2) / (n1 + n2);
    let exp1 = n1 * p_pool;
    let exp2 = n2 * p_pool;
    let ec1 = n1 * (1.0 - p_pool);
    let ec2 = n2 * (1.0 - p_pool);
    let c: f64 = 0.5; // Yates
    let stat = ((x1 - exp1).abs() - c).powi(2) / exp1
        + ((x2 - exp2).abs() - c).powi(2) / exp2
        + (((n1 - x1) - ec1).abs() - c).powi(2) / ec1
        + (((n2 - x2) - ec2).abs() - c).powi(2) / ec2;
    assert_close(t.stat, stat, 1e-9, "χ² two-sample");
    assert_close(t.p_value, chisq_sf(stat, 1.0), 1e-12, "p two-sample");
}

// ════ var_test ══════════════════════════════════════════════════════════════

#[test]
fn var_test_matches_r_pf() {
    // R: var.test(c(2.1,2.5,1.8,3.0,2.7,1.9,2.4,2.2), c(3.0,3.5,3.2,4.0,4.8,5.5,5.4,6.2))
    let x = vec![2.1, 2.5, 1.8, 3.0, 2.7, 1.9, 2.4, 2.2];
    let y = vec![3.0, 3.5, 3.2, 4.0, 4.8, 5.5, 5.4, 6.2];
    let t = h::var_test(&x, &y, 1.0, h::Alternative::TwoSided).unwrap();
    let (m1, m2) = (x.iter().sum::<f64>() / 8.0, y.iter().sum::<f64>() / 8.0);
    let (v1, v2) = (
        x.iter().map(|&xi| (xi - m1).powi(2)).sum::<f64>() / 7.0,
        y.iter().map(|&yi| (yi - m2).powi(2)).sum::<f64>() / 7.0,
    );
    let f_expected = v1 / v2;
    assert_close(t.stat, f_expected, 1e-12, "F");
    assert_close(t.dof, 7.0, 1e-12, "df1");
    assert_close(t.extra_f64("df2").unwrap(), 7.0, 1e-12, "df2");
    // two-sided: 2·min(P(F>f), P(F<f))
    let upper = f_sf(f_expected, 7.0, 7.0);
    let lower = 1.0 - upper;
    assert_close(t.p_value, 2.0 * upper.min(lower), 1e-10, "p two-sided");
}

// ════ cor_test ══════════════════════════════════════════════════════════════

#[test]
fn cor_pearson_matches_r_cor_test() {
    // R: cor.test(c(1,2,3,4,5,6,7,8), c(2,4,5,7,9,11,12,15))
    //   r ≈ 0.9958, t = r√(6/(1-r²)), p ≈ 2·pt(-|t|, 6)
    let x = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
    let y = vec![2.0, 4.0, 5.0, 7.0, 9.0, 11.0, 12.0, 15.0];
    let t = h::cor_test(&x, &y, h::CorMethod::Pearson, h::Alternative::TwoSided).unwrap();
    let n = 8.0;
    let mx: f64 = x.iter().sum::<f64>() / n;
    let my: f64 = y.iter().sum::<f64>() / n;
    let sxy: f64 = x
        .iter()
        .zip(&y)
        .map(|(&xi, &yi)| (xi - mx) * (yi - my))
        .sum();
    let sxx: f64 = x.iter().map(|&xi| (xi - mx).powi(2)).sum();
    let syy: f64 = y.iter().map(|&yi| (yi - my).powi(2)).sum();
    let r = sxy / (sxx * syy).sqrt();
    let df = n - 2.0;
    let t_expected = r * (df / (1.0 - r * r)).sqrt();
    assert_close(t.extra_f64("estimate").unwrap(), r, 1e-9, "r");
    assert_close(t.stat, t_expected, 1e-9, "t");
    assert_close(t.p_value, t_two_sided(t_expected.abs(), df), 1e-9, "p");
}

#[test]
fn cor_spearman_equals_pearson_on_ranks() {
    // Spearman = Pearson on average ranks.
    let x = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
    let y = vec![2.0, 4.0, 5.0, 7.0, 9.0, 11.0, 12.0, 15.0];
    let sp = h::cor_test(&x, &y, h::CorMethod::Spearman, h::Alternative::TwoSided).unwrap();
    let rk_x = h_cor_rank_average(&x);
    let rk_y = h_cor_rank_average(&y);
    let pe = h::cor_test(
        &rk_x,
        &rk_y,
        h::CorMethod::Pearson,
        h::Alternative::TwoSided,
    )
    .unwrap();
    assert_close(
        sp.extra_f64("estimate").unwrap(),
        pe.extra_f64("estimate").unwrap(),
        1e-12,
        "Spearman=Pearson(ranks)",
    );
}

#[test]
fn cor_kendall_matches_formula() {
    // R: cor.test(x, y, method="kendall") for small n
    let x = vec![1.0, 2.0, 3.0, 4.0, 5.0];
    let y = vec![10.0, 20.0, 30.0, 40.0, 50.0];
    let t = h::cor_test(&x, &y, h::CorMethod::Kendall, h::Alternative::TwoSided).unwrap();
    // All increasing → τ = 1
    assert_close(t.extra_f64("estimate").unwrap(), 1.0, 1e-12, "τ");
}

// Helper: same rank_average used in cor.rs (copied to avoid exposing private fn)
fn h_cor_rank_average(v: &[f64]) -> Vec<f64> {
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

// ════ chisq_gof ══════════════════════════════════════════════════════════════

#[test]
fn chisq_gof_matches_r_chisq_test() {
    // R: chisq.test(c(10,20,30,40), p=c(0.25,0.25,0.25,0.25))
    //   χ² = (15² + 5² + 5² + 15²) / 25 = 20
    let t = h::chisq_gof(&[10, 20, 30, 40], &[0.25, 0.25, 0.25, 0.25], false).unwrap();
    assert_close(t.stat, 20.0, 1e-9, "χ²");
    assert_close(t.dof, 3.0, 1e-12, "df");
    assert_close(t.p_value, chisq_sf(20.0, 3.0), 1e-12, "p");
}

#[test]
fn chisq_gof_rescale_p_matches_r() {
    // rescale_p=TRUE with non-normalised p → same as normalised
    let t = h::chisq_gof(&[10, 20, 30, 40], &[1.0, 1.0, 1.0, 1.0], true).unwrap();
    assert_close(t.stat, 20.0, 1e-9, "χ² rescaled");
}

// ════ ks_test ══════════════════════════════════════════════════════════════

#[test]
fn ks_one_sample_normal_perfect_match() {
    // R: ks.test(qnorm(c(1:n)/(n+1))) → D ≈ 0
    let n = 50;
    let quantiles: Vec<f64> = (1..=n)
        .map(|i| normal_inv(i as f64 / (n + 1) as f64))
        .collect();
    let t = h::ks_one_sample(&quantiles, normal_cdf).unwrap();
    // Discrete approx: D ≤ 1/(n+1) ≈ 0.0196
    assert!(
        t.stat < 0.02,
        "D should be small for quantiles, got {}",
        t.stat
    );
    assert!(t.p_value > 0.4, "p = {}", t.p_value);
}

#[test]
fn ks_two_sample_disjoint_matches_r() {
    // R: ks.test(rep(1,20), rep(10,20))
    let x = vec![1.0; 20];
    let y = vec![10.0; 20];
    let t = h::ks_two_sample(&x, &y, h::Alternative::TwoSided).unwrap();
    assert_close(t.stat, 1.0, 1e-12, "D");
    assert!(t.p_value < 1e-6, "p should be ≈ 0, got {}", t.p_value);
}

#[test]
fn ks_two_sample_same_matches_r() {
    // R: ks.test(c(1,2,3,4,5), c(1,2,3,4,5)) → D=0
    let v = vec![1.0, 2.0, 3.0, 4.0, 5.0];
    let t = h::ks_two_sample(&v, &v, h::Alternative::TwoSided).unwrap();
    assert_close(t.stat, 0.0, 1e-12, "D");
    assert_close(t.p_value, 1.0, 1e-12, "p");
}

// ════ shapiro_wilk ══════════════════════════════════════════════════════════

#[test]
fn shapiro_wilk_w_matches_normal_quantiles() {
    // For perfect normal quantiles, W should be very close to 1.
    let n = 20;
    let x: Vec<f64> = (1..=n)
        .map(|i| normal_inv((i as f64 - 0.375) / (n as f64 + 0.25)))
        .collect();
    let t = h::shapiro_wilk(&x).unwrap();
    assert!(
        t.stat > 0.95,
        "W should be near 1 for normal quantiles, got {}",
        t.stat
    );
}

// ════ fisher_exact ══════════════════════════════════════════════════════════

#[test]
fn fisher_exact_tea_tasting_matches_r_fisher() {
    // R: fisher.test(matrix(c(3,1,1,3), 2, 2)) → p ≈ 0.4857 (two-sided)
    let t = h::fisher_exact(&[[3, 1], [1, 3]], h::Alternative::TwoSided, 0.95).unwrap();
    assert_close(t.p_value, 0.4857, 1e-3, "tea");
}

#[test]
fn fisher_exact_no_assoc_matches_r_fisher() {
    // R: fisher.test(matrix(c(10,10,10,10), 2, 2)) → p = 1
    let t = h::fisher_exact(&[[10, 10], [10, 10]], h::Alternative::TwoSided, 0.95).unwrap();
    assert_close(t.p_value, 1.0, 1e-12, "no assoc");
}

#[test]
fn fisher_exact_perfect_assoc_matches_r_fisher() {
    // R: fisher.test(matrix(c(5,0,0,5), 2, 2)) → p ≈ 0.00794
    let t = h::fisher_exact(&[[5, 0], [0, 5]], h::Alternative::TwoSided, 0.95).unwrap();
    assert_close(t.p_value, 0.00794, 1e-4, "perfect");
}

// ════ wilcoxon_signed_rank ══════════════════════════════════════════════════

#[test]
fn wilcoxon_signed_rank_matches_r_wilcox() {
    // R: wilcox.test(c(-3.2, -1.5, 0.0, 0.0, 0.4, 1.5, 3.1), mu=0, alternative="two.sided")
    //   Drops zeros (zero.method="wilcox"); uses asymptotic z with continuity.
    let x = vec![-3.2, -1.5, 0.0, 0.0, 0.4, 1.5, 3.1];
    let t = h::wilcoxon_signed_rank(
        &x,
        0.0,
        h::Alternative::TwoSided,
        h::ZeroMethod::Wilcox,
        true,
    )
    .unwrap();
    // After dropping zeros: [-3.2, -1.5, 0.4, 1.5, 3.1]
    // abs: [3.2, 1.5, 0.4, 1.5, 3.1] → sorted [0.4, 1.5, 1.5, 3.1, 3.2]
    // avg ranks (map back to original order): 3.2→5, 1.5→2.5, 0.4→1, 1.5→2.5, 3.1→4
    let ranks_orig = vec![5.0, 2.5, 1.0, 2.5, 4.0];
    let original = [-3.2, -1.5, 0.4, 1.5, 3.1];
    let w_plus: f64 = original
        .iter()
        .zip(&ranks_orig)
        .filter(|(v, _)| **v > 0.0)
        .map(|(_, r)| r)
        .sum();
    // W+ = 1 + 2.5 + 4 = 7.5 (deterministic)
    assert_close(t.stat, w_plus, 1e-9, "W+");
    assert_close(t.stat, 7.5, 1e-9, "W+ explicit");
    assert!(
        t.p_value > 0.0 && t.p_value <= 1.0,
        "p in (0,1], got {}",
        t.p_value
    );
}

#[test]
fn wilcoxon_signed_rank_symmetric_data() {
    // Symmetric data around 0 → W+ ≈ n(n+1)/4 → z ≈ 0 → p ≈ 1
    let x: Vec<f64> = (1..=10).map(|i| i as f64).collect();
    let xs: Vec<f64> = x
        .iter()
        .map(|&xi| if xi % 2.0 == 0.0 { xi } else { -xi })
        .collect();
    let t = h::wilcoxon_signed_rank(
        &xs,
        0.0,
        h::Alternative::TwoSided,
        h::ZeroMethod::Wilcox,
        true,
    )
    .unwrap();
    assert!(
        t.p_value > 0.5,
        "p should be > 0.5 for symmetric data, got {}",
        t.p_value
    );
}

// ════ mann_whitney ══════════════════════════════════════════════════════════

#[test]
fn mann_whitney_disjoint_matches_r_wilcox() {
    // R: wilcox.test(c(10,11,12,13,14), c(1,2,3,4,5), correct=TRUE)
    //   U = 0, asymptotic z = (0 − 12.5 + 0.5) / sqrt(12.5²/12) ≈ −3.47
    let x = vec![10.0, 11.0, 12.0, 13.0, 14.0];
    let y = vec![1.0, 2.0, 3.0, 4.0, 5.0];
    let t = h::mann_whitney(&x, &y, h::Alternative::TwoSided, true).unwrap();
    // U = 0 (all ranks of x are in the top 5)
    assert_close(t.stat, 0.0, 1e-9, "U");
    let mean_u: f64 = 5.0 * 5.0 / 2.0;
    let var_u: f64 = 5.0 * 5.0 * 11.0 / 12.0;
    let z: f64 = ((0.0 - mean_u).abs() - 0.5) / var_u.sqrt();
    let p_expected = normal_two_sided_p(z);
    assert_close(t.p_value, p_expected, 1e-6, "p");
}

#[test]
fn mann_whitney_identical_groups() {
    let v = vec![1.0, 2.0, 3.0, 4.0, 5.0];
    let t = h::mann_whitney(&v, &v, h::Alternative::TwoSided, true).unwrap();
    assert_close(t.stat, 12.5, 1e-9, "U=n1*n2/2");
    assert!(
        t.p_value > 0.5,
        "p should be > 0.5 for identical groups, got {}",
        t.p_value
    );
}

// ════ kruskal_wallis ════════════════════════════════════════════════════════

#[test]
fn kruskal_wallis_matches_r_kruskal() {
    // R: kruskal.test(c(1,3,5), c(2,4,6), c(3,5,7))
    //   H = (12/9·(10/3)·(S²/n) − 3·4) = …
    let g1 = vec![1.0, 3.0, 5.0];
    let g2 = vec![2.0, 4.0, 6.0];
    let g3 = vec![3.0, 5.0, 7.0];
    let t = h::kruskal_wallis(&[&g1, &g2, &g3]).unwrap();
    assert!((t.dof - 2.0).abs() < 1e-12, "df=2");
    assert!(
        t.p_value > 0.0 && t.p_value < 1.0,
        "p in [0,1], got {}",
        t.p_value
    );
}

#[test]
fn kruskal_wallis_identical_groups() {
    // H = 0 → p = 1
    let g = vec![1.0, 2.0, 3.0];
    let t = h::kruskal_wallis(&[&g, &g]).unwrap();
    assert_close(t.stat, 0.0, 1e-9, "H");
    assert_close(t.p_value, 1.0, 1e-12, "p");
}

// ════ friedman_test ════════════════════════════════════════════════════════

#[test]
fn friedman_matches_r_friedman() {
    // R: friedman.test(matrix(c(1,2,3, 1,2,3, 1,2,3, 1,2,3), 4, 3, byrow=TRUE))
    //   Clear treatment effect → χ² large
    let b1 = vec![1.0, 2.0, 3.0];
    let b2 = vec![1.0, 2.0, 3.0];
    let b3 = vec![1.0, 2.0, 3.0];
    let b4 = vec![1.0, 2.0, 3.0];
    let t = h::friedman_test(&[&b1, &b2, &b3, &b4]).unwrap();
    // Block ranks within each block are (1,2,3) always; rank sums: t1=4, t2=8, t3=12
    // χ² = 12/(4·3·4) · (16+64+144) - 3·4·4 = 224/48 − 48 = 4.666... − 48 = -43.33
    // Wait — let me redo: if block ranks are (1,2,3), R₁=4, R₂=8, R₃=12
    // χ² = 12/(bk(k+1))·ΣRⱼ² - 3b(k+1) = 12/(12·4)·224 - 48 = 56 - 48 = 8
    assert_close(t.stat, 8.0, 1e-9, "χ² friedman");
    assert!((t.dof - 2.0).abs() < 1e-12, "df");
}

// ════ bartlett_test ════════════════════════════════════════════════════════

#[test]
fn bartlett_matches_r_bartlett() {
    // R: bartlett.test(c(2,4,6,8,10), c(1,3,5,7,9), c(2,4,6,8,10))
    let g1 = vec![2.0, 4.0, 6.0, 8.0, 10.0];
    let g2 = vec![1.0, 3.0, 5.0, 7.0, 9.0];
    let g3 = vec![2.0, 4.0, 6.0, 8.0, 10.0];
    let t = h::bartlett_test(&[&g1, &g2, &g3]).unwrap();
    assert!((t.dof - 2.0).abs() < 1e-12, "df=k-1");
    assert!(
        t.p_value >= 0.0 && t.p_value <= 1.0,
        "p in [0,1], got {}",
        t.p_value
    );
}

#[test]
fn bartlett_equal_variances() {
    // Equal-variance groups → χ² ≈ 0
    let g1 = vec![1.0, 2.0, 3.0, 4.0, 5.0];
    let g2 = vec![1.0, 2.0, 3.0, 4.0, 5.0];
    let t = h::bartlett_test(&[&g1, &g2]).unwrap();
    assert!(t.stat < 0.5, "K² should be small, got {}", t.stat);
}

// ════ levene_test ══════════════════════════════════════════════════════════

#[test]
fn levene_matches_r_levene_test() {
    // R: car::leveneTest(value ~ group, data=…)
    // Approximate equality with F test.
    let g1 = vec![10.0, 12.0, 14.0, 16.0, 18.0];
    let g2 = vec![20.0, 22.0, 24.0, 26.0, 28.0];
    let g3 = vec![30.0, 32.0, 34.0, 36.0, 38.0];
    let t = h::levene_test(&[&g1, &g2, &g3], h::Center::Mean).unwrap();
    // All groups have identical variance and equal step → F should be small.
    assert!(
        t.stat < 5.0,
        "F should be small for equal-variance groups, got {}",
        t.stat
    );
    assert!(t.p_value > 0.05, "p should be > 0.05, got {}", t.p_value);
}

#[test]
fn levene_brown_forsythe_matches() {
    // For perfectly symmetric data, Levene(median) = Levene(mean)
    let g1 = vec![1.0, 2.0, 3.0, 4.0, 5.0];
    let g2 = vec![1.0, 2.0, 3.0, 4.0, 5.0];
    let mean = h::levene_test(&[&g1, &g2], h::Center::Mean).unwrap();
    let median = h::levene_test(&[&g1, &g2], h::Center::Median).unwrap();
    assert_close(mean.stat, median.stat, 1e-12, "Levene=BF for symmetric");
}

// ════ fligner_test ═════════════════════════════════════════════════════════

#[test]
fn fligner_equal_variances_is_null() {
    // Same variances → χ² ≈ 0
    let g1 = vec![1.0, 2.0, 3.0, 4.0, 5.0];
    let g2 = vec![1.0, 2.0, 3.0, 4.0, 5.0];
    let t = h::fligner_test(&[&g1, &g2]).unwrap();
    assert!(t.stat < 0.5, "χ² small, got {}", t.stat);
    assert!(t.p_value > 0.5, "p large, got {}", t.p_value);
}

// ════ oneway_anova ════════════════════════════════════════════════════════

#[test]
fn classical_anova_matches_r_aov() {
    // R: aov(value ~ group, data=…)
    //   Between groups clear, within small → F large
    let g1 = vec![10.0, 11.0, 12.0, 13.0, 14.0];
    let g2 = vec![20.0, 21.0, 22.0, 23.0, 24.0];
    let g3 = vec![30.0, 31.0, 32.0, 33.0, 34.0];
    let t = h::oneway_anova(&[&g1, &g2, &g3], true).unwrap();
    // Manual F
    let n_total = 15.0;
    let n_per = 5.0;
    let m1: f64 = g1.iter().sum::<f64>() / n_per;
    let m2: f64 = g2.iter().sum::<f64>() / n_per;
    let m3: f64 = g3.iter().sum::<f64>() / n_per;
    let grand =
        (g1.iter().sum::<f64>() + g2.iter().sum::<f64>() + g3.iter().sum::<f64>()) / n_total;
    let ss_b = n_per * ((m1 - grand).powi(2) + (m2 - grand).powi(2) + (m3 - grand).powi(2));
    let ss_w: f64 = g1.iter().map(|&xi| (xi - m1).powi(2)).sum::<f64>()
        + g2.iter().map(|&xi| (xi - m2).powi(2)).sum::<f64>()
        + g3.iter().map(|&xi| (xi - m3).powi(2)).sum::<f64>();
    let f_expected = (ss_b / 2.0) / (ss_w / 12.0);
    assert_close(t.stat, f_expected, 1e-9, "F classical");
    assert_close(t.p_value, f_sf(f_expected, 2.0, 12.0), 1e-9, "p classical");
}

#[test]
fn welch_anova_matches_r_oneway_test() {
    // R: oneway.test(value ~ group, data=…)
    let g1 = vec![10.0, 11.0, 12.0, 13.0, 14.0];
    let g2 = vec![20.0, 21.0, 22.0, 23.0, 24.0];
    let g3 = vec![30.0, 31.0, 32.0, 33.0, 34.0];
    let t = h::oneway_anova(&[&g1, &g2, &g3], false).unwrap();
    // Manual Welch: each group has mean (12,22,32), s² = 2.5, n=5.
    let s2 = 2.5_f64;
    let n_i = 5.0_f64;
    let k = 3.0_f64;
    let w_i = n_i / s2; // 2
    let w_sum = k * w_i;
    let means = [12.0, 22.0, 32.0];
    let yw = means.iter().map(|&m| w_i * m).sum::<f64>() / w_sum;
    let numerator = means.iter().map(|&m| w_i * (m - yw).powi(2)).sum::<f64>() / (k - 1.0);
    let c = (0..3)
        .map(|_| (1.0 - w_i / w_sum).powi(2) / (n_i - 1.0))
        .sum::<f64>();
    let denominator = 1.0 + 2.0 * (k - 2.0) / (k * k - 1.0) * c;
    let f_expected = numerator / denominator;
    assert_close(t.stat, f_expected, 1e-9, "F welch");
    // df1 = k-1 = 2
    assert_close(t.dof, 2.0, 1e-12, "df1");
}

// ════ anderson_darling ══════════════════════════════════════════════════════

#[test]
fn anderson_darling_normal_perfect_p_large() {
    // R: nortest::ad.test(qnorm(c(1:n)/(n+1))) — perfect quantiles
    let n = 50;
    let x: Vec<f64> = (1..=n)
        .map(|i| normal_inv(i as f64 / (n + 1) as f64))
        .collect();
    let t = h::anderson_darling(&x, h::AdDist::Normal).unwrap();
    assert!(
        t.p_value > 0.05,
        "p should be > 0.05 for normal quantiles, got {}",
        t.p_value
    );
}

#[test]
fn anderson_darling_nonnormal_small_p() {
    // Heavy outlier skews distribution → small p
    let x: Vec<f64> = (0..30)
        .map(|_| 1.0_f64)
        .chain(std::iter::once(100.0_f64))
        .collect();
    let t = h::anderson_darling(&x, h::AdDist::Normal).unwrap();
    assert!(
        t.p_value < 0.01,
        "p should be < 0.01 for heavy outlier, got {}",
        t.p_value
    );
}

// ════ stouffer_combine ═════════════════════════════════════════════════════

#[test]
fn stouffer_combine_matches_r_poolr() {
    // R: poolr::stouffer(c(0.01, 0.04, 0.03))
    //   z_i = qnorm(1 − p_i); Z = Σz/√3; p = 2·pnorm(-|Z|)
    let pvals = vec![0.01_f64, 0.04, 0.03];
    let t = h::stouffer_combine_pvals(&pvals, None).unwrap();
    let zs: Vec<f64> = pvals.iter().map(|&p| normal_inv(1.0 - p)).collect();
    let z = zs.iter().sum::<f64>() / (3.0_f64).sqrt();
    let p_expected = normal_two_sided_p(z);
    assert_close(t.extra_f64("z").unwrap(), z, 1e-9, "z");
    assert_close(t.p_value, p_expected, 1e-9, "p stouffer");
}

#[test]
fn stouffer_weighted_matches_r() {
    // Weighted version: Z = Σ w·z/√(Σw²)
    let pvals = vec![0.01_f64, 0.05];
    let w = vec![1.0, 2.0];
    let t = h::stouffer_combine_pvals(&pvals, Some(&w)).unwrap();
    let zs: Vec<f64> = pvals.iter().map(|&p| normal_inv(1.0 - p)).collect();
    let z = (w[0] * zs[0] + w[1] * zs[1]) / (w[0].powi(2) + w[1].powi(2)).sqrt();
    assert_close(t.extra_f64("z").unwrap(), z, 1e-9, "z weighted");
}

// ════ tippett_combine ═══════════════════════════════════════════════════════

#[test]
fn tippett_min_matches_r_poolr() {
    // R: poolr::tippett(c(0.01, 0.04, 0.03)) → p = 1 - (1-min)³
    let pvals = vec![0.01_f64, 0.04, 0.03];
    let t = h::tippett_combine_pvals(&pvals, h::TippettVariant::Min).unwrap();
    let min_p = pvals.iter().copied().fold(f64::INFINITY, f64::min);
    let p_expected = 1.0 - (1.0 - min_p).powi(3);
    assert_close(t.p_value, p_expected, 1e-12, "p tippett min");
}

#[test]
fn tippett_max_matches_r_poolr() {
    // R: poolr::tippett(..., "max") → p = max^k
    let pvals = vec![0.1_f64, 0.2, 0.3];
    let t = h::tippett_combine_pvals(&pvals, h::TippettVariant::Max).unwrap();
    let max_p = pvals.iter().copied().fold(0.0_f64, f64::max);
    let p_expected = max_p.powi(3);
    assert_close(t.p_value, p_expected, 1e-12, "p tippett max");
}

// ════ wilkinson_combine ════════════════════════════════════════════════════

#[test]
fn wilkinson_r1_matches_tippett_min() {
    // r = 1 ↔ Tippett min
    let pvals = vec![0.01_f64, 0.04, 0.03];
    let w = h::wilkinson_combine_pvals(&pvals, 1).unwrap();
    let t = h::tippett_combine_pvals(&pvals, h::TippettVariant::Min).unwrap();
    assert_close(w.p_value, t.p_value, 1e-12, "Wilkinson(1)=Tippett(min)");
}

#[test]
fn wilkinson_rk_matches_tippett_max() {
    // r = k ↔ Tippett max
    let pvals = vec![0.1_f64, 0.2, 0.3];
    let w = h::wilkinson_combine_pvals(&pvals, 3).unwrap();
    let t = h::tippett_combine_pvals(&pvals, h::TippettVariant::Max).unwrap();
    assert_close(w.p_value, t.p_value, 1e-12, "Wilkinson(k)=Tippett(max)");
}
