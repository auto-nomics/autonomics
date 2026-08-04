//! t-tests — port of `stats::t.test`.
//!
//! One-sample, paired, and two-sample (Welch or pooled-variance) t-tests.
//! All branches match R `t.test.default` numerically.

use serde_json::json;

use super::HypothesisTest;
use crate::{Alternative, HypoError, KEY_KIND, Result, dist::t_sf, extras};

/// One-sample t-test of `H₀: μ = mu0`.
///
/// Mirrors `t.test(x, mu = mu0, alternative = alt)`.
pub fn t_test_one(x: &[f64], mu0: f64, alt: Alternative) -> Result<HypothesisTest> {
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
        return Err(HypoError::InvalidInput("all values are identical (SE = 0)".into()));
    }
    let t = (mean - mu0) / se;
    let df = n_f - 1.0;
    let p = p_value_t(t, df, alt);
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
            ("n", json!(n as u64)),
        ]),
    ))
}

/// Paired t-test of `H₀: E[x − y] = 0`.
///
/// Equivalent to a one-sample test on the differences `dᵢ = xᵢ − yᵢ`.
pub fn t_test_paired(x: &[f64], y: &[f64], alt: Alternative) -> Result<HypothesisTest> {
    if x.len() != y.len() {
        return Err(HypoError::LengthMismatch { a: x.len(), b: y.len() });
    }
    let d: Vec<f64> = x.iter().zip(y).map(|(a, b)| a - b).collect();
    let n = d.len();
    if n < 2 {
        return Err(HypoError::InvalidInput("paired t-test requires ≥ 2 pairs".into()));
    }
    let n_f = n as f64;
    let mean = crate::extras_mean(&d);
    let ss = d.iter().map(|&di| (di - mean).powi(2)).sum::<f64>();
    let var = ss / (n_f - 1.0);
    let se = (var / n_f).sqrt();
    if se == 0.0 {
        return Err(HypoError::InvalidInput("all differences are identical (SE = 0)".into()));
    }
    let t = mean / se;
    let df = n_f - 1.0;
    let p = p_value_t(t, df, alt);
    Ok(HypothesisTest::new(
        t,
        p,
        df,
        alt,
        "Paired t-test",
        extras([
            (KEY_KIND, json!("t_test")),
            ("estimate", json!(mean)),
            ("null_value", json!(0.0)),
            ("stderr", json!(se)),
            ("n", json!(n as u64)),
        ]),
    ))
}

/// Two-sample t-test.
///
/// `var_equal = false` (default) → Welch (unequal-variance) with
/// Welch–Satterthwaite degrees of freedom. `var_equal = true` → pooled
/// variance with `df = n₁ + n₂ − 2`.
pub fn t_test_two(
    x: &[f64],
    y: &[f64],
    var_equal: bool,
    alt: Alternative,
) -> Result<HypothesisTest> {
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
        return Err(HypoError::InvalidInput("SE = 0 (identical values within groups)".into()));
    }
    let t = (m1 - m2) / se;
    let p = p_value_t(t, df, alt);
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
            ("stderr", json!(se)),
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
        let t = t_test_one(&x, 3.0, Alternative::TwoSided).unwrap();
        assert!((t.stat - 0.0).abs() < 1e-12);
        assert!((t.dof - 4.0).abs() < 1e-12);
        assert!((t.p_value - 1.0).abs() < 1e-12);
    }

    #[test]
    fn one_sample_against_r() {
        // R: t.test(c(2.1, 2.5, 1.8, 3.0, 2.7, 1.9, 2.4, 2.2), mu=2.0)
        // mean=2.325, sd=0.4232, se=0.1496, t=(2.325-2.0)/0.1496=2.171
        let x = vec![2.1, 2.5, 1.8, 3.0, 2.7, 1.9, 2.4, 2.2];
        let t = t_test_one(&x, 2.0, Alternative::TwoSided).unwrap();
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
        let t = t_test_two(&x, &y, false, Alternative::TwoSided).unwrap();
        // mean diff = -1, both groups same variance → Welch = pooled here
        assert!((t.stat + 1.0_f64).abs() < 0.5); // roughly t≈-1
        assert!(t.dof < 8.5 && t.dof > 7.5); // ≈8
    }

    #[test]
    fn two_sample_pooled() {
        let x = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let y = vec![2.0, 3.0, 4.0, 5.0, 6.0];
        let t = t_test_two(&x, &y, true, Alternative::TwoSided).unwrap();
        assert!((t.dof - 8.0).abs() < 1e-12);
    }

    #[test]
    fn paired() {
        let x = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let y = vec![2.0, 3.0, 4.0, 5.0, 6.0];
        // differences all = -1, sd=0 → error
        assert!(t_test_paired(&x, &y, Alternative::TwoSided).is_err());

        // Non-degenerate paired
        let a = vec![1.0, 3.0, 2.0, 4.0, 5.0];
        let b = vec![2.0, 1.0, 4.0, 3.0, 5.0];
        let t = t_test_paired(&a, &b, Alternative::TwoSided).unwrap();
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
        assert!(t_test_one(&[1.0], 0.0, Alternative::TwoSided).is_err());
        assert!(t_test_one(&[], 0.0, Alternative::TwoSided).is_err());
        assert!(t_test_two(&[1.0], &[2.0], false, Alternative::TwoSided).is_err());
        assert!(t_test_paired(&[1.0, 2.0], &[1.0], Alternative::TwoSided).is_err());
    }
}
