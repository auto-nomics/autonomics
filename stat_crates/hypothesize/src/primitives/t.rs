//! t-tests — port of `stats::t.test`.
//!
//! One-sample, paired, and two-sample (Welch or pooled-variance) t-tests.
//! All branches match R `t.test.default` numerically, including the
//! confidence interval on the estimate (`conf.level`, one- or two-sided per
//! `alternative`).

use serde_json::json;

use super::HypothesisTest;
use crate::{Alternative, HypoError, KEY_KIND, Result, dist::t_inv, dist::t_sf, extras};

/// Reject `conf_level` outside (0, 1) — mirrors R's `conf.level` check.
fn validate_conf_level(conf_level: f64) -> Result<()> {
    if !(conf_level > 0.0 && conf_level < 1.0) {
        return Err(HypoError::InvalidInput(format!(
            "conf_level must be in (0, 1), got {conf_level}"
        )));
    }
    Ok(())
}

/// Confidence interval for `est ± z·se` following R `t.test.default`:
/// two-sided uses `qt(1 − α/2, df)`; one-sided intervals keep the R shape
/// (`less` → `(-Inf, est + qt(1−α)·se)`, `greater` →
/// `(est − qt(1−α)·se, Inf)`). Non-finite bounds serialise to JSON `null`
/// in `extras` (JSON has no ±Inf), so one-sided rows carry a null bound.
fn conf_int_t(est: f64, se: f64, df: f64, alt: Alternative, conf_level: f64) -> (f64, f64) {
    let alpha = 1.0 - conf_level;
    match alt {
        Alternative::TwoSided => {
            let z = t_inv(1.0 - alpha / 2.0, df);
            (est - z * se, est + z * se)
        }
        Alternative::Less => (f64::NEG_INFINITY, est + t_inv(1.0 - alpha, df) * se),
        Alternative::Greater => (est - t_inv(1.0 - alpha, df) * se, f64::INFINITY),
    }
}

/// Serialise a CI bound, mapping ±Inf to JSON null (see [`conf_int_t`]).
fn ci_bound_json(v: f64) -> serde_json::Value {
    if v.is_finite() {
        json!(v)
    } else {
        serde_json::Value::Null
    }
}

/// One-sample t-test of `H₀: μ = mu0`.
///
/// Mirrors `t.test(x, mu = mu0, alternative = alt, conf.level = cl)`.
pub fn t_test_one(
    x: &[f64],
    mu0: f64,
    alt: Alternative,
    conf_level: f64,
) -> Result<HypothesisTest> {
    validate_conf_level(conf_level)?;
    let n = x.len();
    if n < 2 {
        return Err(HypoError::InvalidInput(
            "one-sample t-test requires ≥ 2 observations".into(),
        ));
    }
    let n_f = n as f64;
    let mean = crate::extras_mean(x);
    let ss = x.iter().map(|&xi| (xi - mean).powi(2)).sum::<f64>();
    let var = ss / (n_f - 1.0);
    let se = (var / n_f).sqrt();
    if se == 0.0 {
        return Err(HypoError::InvalidInput(
            "all values are identical (SE = 0)".into(),
        ));
    }
    let t = (mean - mu0) / se;
    let df = n_f - 1.0;
    let p = p_value_t(t, df, alt);
    let (conf_low, conf_high) = conf_int_t(mean, se, df, alt, conf_level);
    Ok(HypothesisTest::new(
        t,
        p,
        df,
        alt,
        "One Sample t-test",
        extras([
            (KEY_KIND, json!("t_test")),
            ("estimate", json!(mean)),
            ("null_value", json!(mu0)),
            ("stderr", json!(se)),
            ("conf_low", ci_bound_json(conf_low)),
            ("conf_high", ci_bound_json(conf_high)),
            ("n", json!(n as u64)),
        ]),
    ))
}

/// Paired t-test of `H₀: E[x − y] = 0`.
///
/// Equivalent to a one-sample test on the differences `dᵢ = xᵢ − yᵢ`.
/// Complete-pair handling (R drops a pair when either side is `NA`) is the
/// caller's responsibility — this f64-layer contract is unchanged.
pub fn t_test_paired(
    x: &[f64],
    y: &[f64],
    mu0: f64,
    alt: Alternative,
    conf_level: f64,
) -> Result<HypothesisTest> {
    validate_conf_level(conf_level)?;
    if x.len() != y.len() {
        return Err(HypoError::LengthMismatch {
            a: x.len(),
            b: y.len(),
        });
    }
    let d: Vec<f64> = x.iter().zip(y).map(|(a, b)| a - b).collect();
    let n = d.len();
    if n < 2 {
        return Err(HypoError::InvalidInput(
            "paired t-test requires ≥ 2 pairs".into(),
        ));
    }
    let n_f = n as f64;
    let mean = crate::extras_mean(&d);
    let ss = d.iter().map(|&di| (di - mean).powi(2)).sum::<f64>();
    let var = ss / (n_f - 1.0);
    let se = (var / n_f).sqrt();
    if se == 0.0 {
        return Err(HypoError::InvalidInput(
            "all differences are identical (SE = 0)".into(),
        ));
    }
    // R t.test(paired=TRUE, mu=…): the null mean of the differences is mu0,
    // not 0; the CI stays on the unshifted mean difference.
    let t = (mean - mu0) / se;
    let df = n_f - 1.0;
    let p = p_value_t(t, df, alt);
    let (conf_low, conf_high) = conf_int_t(mean, se, df, alt, conf_level);
    Ok(HypothesisTest::new(
        t,
        p,
        df,
        alt,
        "Paired t-test",
        extras([
            (KEY_KIND, json!("t_test")),
            ("estimate", json!(mean)),
            ("null_value", json!(mu0)),
            ("stderr", json!(se)),
            ("conf_low", ci_bound_json(conf_low)),
            ("conf_high", ci_bound_json(conf_high)),
            ("n", json!(n as u64)),
        ]),
    ))
}

/// Two-sample t-test.
///
/// `var_equal = false` (default) → Welch (unequal-variance) with
/// Welch–Satterthwaite degrees of freedom. `var_equal = true` → pooled
/// variance with `df = n₁ + n₂ − 2`. The `estimate` extra is the mean
/// difference `m₁ − m₂` (R's `estimate` alongside `estimate1`/`estimate2`
/// in `diff` semantics); its CI follows `alternative`/`conf_level`.
pub fn t_test_two(
    x: &[f64],
    y: &[f64],
    var_equal: bool,
    mu0: f64,
    alt: Alternative,
    conf_level: f64,
) -> Result<HypothesisTest> {
    validate_conf_level(conf_level)?;
    let (n1, n2) = (x.len(), y.len());
    if n1 < 2 || n2 < 2 {
        return Err(HypoError::InvalidInput(
            "two-sample t-test requires ≥ 2 observations per group".into(),
        ));
    }
    let (n1f, n2f) = (n1 as f64, n2 as f64);
    let m1 = crate::extras_mean(x);
    let m2 = crate::extras_mean(y);
    let ss1 = x.iter().map(|&xi| (xi - m1).powi(2)).sum::<f64>();
    let ss2 = y.iter().map(|&yi| (yi - m2).powi(2)).sum::<f64>();

    let (se, df) = if var_equal {
        let sp2 = (ss1 + ss2) / (n1f + n2f - 2.0);
        let se = (sp2 * (1.0 / n1f + 1.0 / n2f)).sqrt();
        (se, n1f + n2f - 2.0)
    } else {
        let v1 = ss1 / (n1f - 1.0);
        let v2 = ss2 / (n2f - 1.0);
        let se = (v1 / n1f + v2 / n2f).sqrt();
        // Welch–Satterthwaite df
        let num = (v1 / n1f + v2 / n2f).powi(2);
        let den = (v1 / n1f).powi(2) / (n1f - 1.0) + (v2 / n2f).powi(2) / (n2f - 1.0);
        (se, num / den)
    };
    if se == 0.0 {
        return Err(HypoError::InvalidInput(
            "SE = 0 (identical values within groups)".into(),
        ));
    }
    let est = m1 - m2;
    // R t.test(x, y, mu=…): the null difference in means is mu0, not 0;
    // the CI stays on the unshifted difference.
    let t = (est - mu0) / se;
    let p = p_value_t(t, df, alt);
    let (conf_low, conf_high) = conf_int_t(est, se, df, alt, conf_level);
    let method = if var_equal {
        "Two Sample t-test (pooled)"
    } else {
        "Welch Two Sample t-test"
    };
    Ok(HypothesisTest::new(
        t,
        p,
        df,
        alt,
        method,
        extras([
            (KEY_KIND, json!("t_test")),
            ("estimate1", json!(m1)),
            ("estimate2", json!(m2)),
            ("estimate", json!(est)),
            ("null_value", json!(mu0)),
            ("stderr", json!(se)),
            ("conf_low", ci_bound_json(conf_low)),
            ("conf_high", ci_bound_json(conf_high)),
            ("n", json!((n1 + n2) as u64)),
            ("n1", json!(n1 as u64)),
            ("n2", json!(n2 as u64)),
            ("var_equal", json!(var_equal)),
        ]),
    ))
}

/// t-distribution p-value for a given alternative.
pub(crate) fn p_value_t(t: f64, df: f64, alt: Alternative) -> f64 {
    match alt {
        Alternative::TwoSided => 2.0 * t_sf(t.abs(), df),
        Alternative::Less => 1.0 - t_sf(t, df), // P(T < t) = 1 - P(T > t)... careful: t_sf gives P(T > t) for upper tail
        Alternative::Greater => t_sf(t, df),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dist::t_sf;

    #[test]
    fn one_sample_basic() {
        // R: t.test(c(1,2,3,4,5), mu=3) → t=0, df=4, p=1
        let x = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let t = t_test_one(&x, 3.0, Alternative::TwoSided, 0.95).unwrap();
        assert!((t.stat - 0.0).abs() < 1e-12);
        assert!((t.dof - 4.0).abs() < 1e-12);
        assert!((t.p_value - 1.0).abs() < 1e-12);
    }

    #[test]
    fn one_sample_against_r() {
        // R: t.test(c(2.1, 2.5, 1.8, 3.0, 2.7, 1.9, 2.4, 2.2), mu=2.0)
        // mean=2.325, sd=0.4232, se=0.1496, t=(2.325-2.0)/0.1496=2.171
        let x = vec![2.1, 2.5, 1.8, 3.0, 2.7, 1.9, 2.4, 2.2];
        let t = t_test_one(&x, 2.0, Alternative::TwoSided, 0.95).unwrap();
        let mean: f64 = x.iter().sum::<f64>() / 8.0;
        let ss: f64 = x.iter().map(|&xi| (xi - mean).powi(2)).sum();
        let var = ss / 7.0;
        let se = (var / 8.0).sqrt();
        let expected_t = (mean - 2.0) / se;
        assert!((t.stat - expected_t).abs() < 1e-9);
        assert!((t.p_value - 2.0 * t_sf(expected_t.abs(), 7.0)).abs() < 1e-9);
        assert_eq!(t.method, "One Sample t-test");
    }

    #[test]
    fn two_sample_welch() {
        let x = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let y = vec![2.0, 3.0, 4.0, 5.0, 6.0];
        let t = t_test_two(&x, &y, false, 0.0, Alternative::TwoSided, 0.95).unwrap();
        // mean diff = -1, both groups same variance → Welch = pooled here
        assert!((t.stat + 1.0_f64).abs() < 0.5); // roughly t≈-1
        assert!(t.dof < 8.5 && t.dof > 7.5); // ≈8
        assert_eq!(t.extra_f64("n"), Some(10.0)); // total n1 + n2
        assert!((t.extra_f64("estimate").unwrap() - (-1.0)).abs() < 1e-12);
    }

    #[test]
    fn two_sample_pooled() {
        let x = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let y = vec![2.0, 3.0, 4.0, 5.0, 6.0];
        let t = t_test_two(&x, &y, true, 0.0, Alternative::TwoSided, 0.95).unwrap();
        assert!((t.dof - 8.0).abs() < 1e-12);
        assert_eq!(t.extra_f64("n"), Some(10.0));
    }

    #[test]
    fn paired() {
        let x = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let y = vec![2.0, 3.0, 4.0, 5.0, 6.0];
        // differences all = -1, sd=0 → error
        assert!(t_test_paired(&x, &y, 0.0, Alternative::TwoSided, 0.95).is_err());

        // Non-degenerate paired
        let a = vec![1.0, 3.0, 2.0, 4.0, 5.0];
        let b = vec![2.0, 1.0, 4.0, 3.0, 5.0];
        let t = t_test_paired(&a, &b, 0.0, Alternative::TwoSided, 0.95).unwrap();
        let d: Vec<f64> = a.iter().zip(&b).map(|(p, q)| p - q).collect();
        let dmean = d.iter().sum::<f64>() / 5.0;
        let dss = d.iter().map(|&di| (di - dmean).powi(2)).sum::<f64>();
        let dvar = dss / 4.0;
        let dse = (dvar / 5.0).sqrt();
        let expected_t = dmean / dse;
        assert!((t.stat - expected_t).abs() < 1e-9);
        assert!((t.dof - 4.0).abs() < 1e-12);
    }

    #[test]
    fn rejects_bad_inputs() {
        assert!(t_test_one(&[1.0], 0.0, Alternative::TwoSided, 0.95).is_err());
        assert!(t_test_one(&[], 0.0, Alternative::TwoSided, 0.95).is_err());
        assert!(t_test_two(&[1.0], &[2.0], false, 0.0, Alternative::TwoSided, 0.95).is_err());
        assert!(t_test_paired(&[1.0, 2.0], &[1.0], 0.0, Alternative::TwoSided, 0.95).is_err());
        for bad in [0.0, 1.0, -0.1, 1.1] {
            assert!(
                t_test_one(&[1.0, 2.0], 0.0, Alternative::TwoSided, bad).is_err(),
                "conf_level {bad} should be rejected"
            );
        }
    }

    // ── R 4.6.1 goldens (epsilon 1e-14), generated with Rscript ──────────────

    /// Audit counter-example: x = c(1,NA,5,10), y = c(0,3,NA,2) → complete
    /// pairs (1,0),(10,2) → mean diff 4.5 (not 3.6667 from independent
    /// NA-dropping). R: t.test(x, y, paired=TRUE).
    #[test]
    fn paired_audit_example_matches_r() {
        let xs = vec![1.0, 10.0];
        let ys = vec![0.0, 2.0];
        let t = t_test_paired(&xs, &ys, 0.0, Alternative::TwoSided, 0.95).unwrap();
        assert!((t.extra_f64("estimate").unwrap() - 4.5).abs() < 1e-14);
        assert!((t.extra_f64("stderr").unwrap() - 3.5).abs() < 1e-14);
        assert!((t.stat - 1.2857142857142858).abs() < 1e-14);
        assert!((t.dof - 1.0).abs() < 1e-14);
        assert!((t.p_value - 0.42083315167886859).abs() < 1e-14);
        assert!((t.extra_f64("conf_low").unwrap() - (-39.971716576611428)).abs() < 1e-12);
        assert!((t.extra_f64("conf_high").unwrap() - 48.971716576611435).abs() < 1e-12);
        assert_eq!(t.extra_f64("n"), Some(2.0));
    }

    /// R: t.test(x, y, paired=TRUE, alternative="greater") — p halves, CI is
    /// one-sided with a +Inf upper bound (null in extras JSON).
    #[test]
    fn paired_greater_matches_r() {
        let t = t_test_paired(&[1.0, 10.0], &[0.0, 2.0], 0.0, Alternative::Greater, 0.95).unwrap();
        assert!((t.p_value - 0.21041657583943429).abs() < 1e-14);
        assert!((t.extra_f64("conf_low").unwrap() - (-17.59813030136263)).abs() < 1e-12);
        assert_eq!(t.extra_f64("conf_high"), None); // +Inf → JSON null
    }

    /// R: t.test(x, y, paired=TRUE, conf.level=0.9).
    #[test]
    fn paired_conf_90_matches_r() {
        let t = t_test_paired(&[1.0, 10.0], &[0.0, 2.0], 0.0, Alternative::TwoSided, 0.9).unwrap();
        assert!((t.p_value - 0.42083315167886859).abs() < 1e-14);
        assert!((t.extra_f64("conf_low").unwrap() - (-17.59813030136263)).abs() < 1e-12);
        assert!((t.extra_f64("conf_high").unwrap() - 26.59813030136263).abs() < 1e-12);
    }

    /// R: t.test(c(1.2,3.1,4.7), c(2.1,4.0,6.8)) —
    /// m1=3, m2=4.3, se=1.69901932498329, t=-0.765147271066377,
    /// df=3.68773387751088, p=0.490202972932868,
    /// CI=(-6.17901953916217, 3.57901953916217).
    #[test]
    fn welch_matches_r() {
        let x = vec![1.2, 3.1, 4.7];
        let y = vec![2.1, 4.0, 6.8];
        let t = t_test_two(&x, &y, false, 0.0, Alternative::TwoSided, 0.95).unwrap();
        assert_eq!(t.method, "Welch Two Sample t-test");
        assert!((t.extra_f64("estimate1").unwrap() - 3.0).abs() < 1e-14);
        assert!((t.extra_f64("estimate2").unwrap() - 4.3).abs() < 1e-14);
        assert!((t.extra_f64("estimate").unwrap() - (3.0 - 4.3)).abs() < 1e-14);
        assert!((t.extra_f64("stderr").unwrap() - 1.69901932498329).abs() < 1e-14);
        assert!((t.stat - (-0.765147271066377)).abs() < 1e-14);
        assert!((t.dof - 3.68773387751088).abs() < 1e-14);
        assert!((t.p_value - 0.490202972932868).abs() < 1e-14);
        assert!((t.extra_f64("conf_low").unwrap() - (-6.17901953916217)).abs() < 1e-12);
        assert!((t.extra_f64("conf_high").unwrap() - 3.57901953916217).abs() < 1e-12);
    }

    /// R: same vectors, var.equal=TRUE — se identical (n₁=n₂), df=4,
    /// p=0.486834417888926, CI=(-6.01723388848631, 3.41723388848631).
    #[test]
    fn pooled_matches_r() {
        let x = vec![1.2, 3.1, 4.7];
        let y = vec![2.1, 4.0, 6.8];
        let t = t_test_two(&x, &y, true, 0.0, Alternative::TwoSided, 0.95).unwrap();
        assert_eq!(t.method, "Two Sample t-test (pooled)");
        assert!((t.extra_f64("estimate").unwrap() - (3.0 - 4.3)).abs() < 1e-14);
        assert!((t.extra_f64("stderr").unwrap() - 1.69901932498329).abs() < 1e-14);
        assert!((t.stat - (-0.765147271066377)).abs() < 1e-14);
        assert!((t.dof - 4.0).abs() < 1e-14);
        assert!((t.p_value - 0.486834417888926).abs() < 1e-14);
        assert!((t.extra_f64("conf_low").unwrap() - (-6.01723388848631)).abs() < 1e-12);
        assert!((t.extra_f64("conf_high").unwrap() - 3.41723388848631).abs() < 1e-12);
    }

    /// R: t.test(c(2.1,NA,2.5,1.8,3.0,NA,2.7), mu=2.0) → n=5 after NA-drop.
    #[test]
    fn one_sample_na_dropped_matches_r() {
        let x = vec![2.1, 2.5, 1.8, 3.0, 2.7];
        let t = t_test_one(&x, 2.0, Alternative::TwoSided, 0.95).unwrap();
        assert!((t.extra_f64("estimate").unwrap() - 2.42).abs() < 1e-14);
        assert!((t.extra_f64("stderr").unwrap() - 0.21307275752662516).abs() < 1e-14);
        assert!((t.stat - 1.9711576687485144).abs() < 1e-14);
        assert!((t.dof - 4.0).abs() < 1e-14);
        assert!((t.p_value - 0.1200102801847263).abs() < 1e-14);
        assert!((t.extra_f64("conf_low").unwrap() - 1.8284151853142052).abs() < 1e-12);
        assert!((t.extra_f64("conf_high").unwrap() - 3.0115848146857944).abs() < 1e-12);
        assert_eq!(t.extra_f64("n"), Some(5.0));
    }

    /// R: same, alternative="less" — p and one-sided CI.
    #[test]
    fn one_sample_less_matches_r() {
        let x = vec![2.1, 2.5, 1.8, 3.0, 2.7];
        let t = t_test_one(&x, 2.0, Alternative::Less, 0.95).unwrap();
        assert!((t.p_value - 0.93999485990763687).abs() < 1e-14);
        assert_eq!(t.extra_f64("conf_low"), None); // -Inf → JSON null
        assert!((t.extra_f64("conf_high").unwrap() - 2.8742384733868933).abs() < 1e-12);
    }
}
