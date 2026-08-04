//! Correlation tests — port of `stats::cor.test`.
//!
//! Pearson and Spearman correlation tests with t-distribution p-values.
//! Kendall's τ uses the exact variance formula.

use serde_json::json;

use super::HypothesisTest;
use crate::{
    Alternative, HypoError, KEY_KIND, Result,
    dist::{normal_cdf, normal_two_sided_p, t_sf},
    extras,
};

/// Correlation method selector.
#[derive(
    Clone, Copy, PartialEq, Eq, Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum CorMethod {
    Pearson,
    Spearman,
    Kendall,
}

/// Test for association between paired samples.
///
/// Mirrors `cor.test(x, y, method = ..., alternative = ...)`.
pub fn cor_test(
    x: &[f64],
    y: &[f64],
    method: CorMethod,
    alt: Alternative,
) -> Result<HypothesisTest> {
    if x.len() != y.len() {
        return Err(HypoError::LengthMismatch {
            a: x.len(),
            b: y.len(),
        });
    }
    let n = x.len();
    if n < 3 {
        return Err(HypoError::InvalidInput(
            "correlation test requires ≥ 3 paired observations".into(),
        ));
    }
    match method {
        CorMethod::Pearson => pearson(x, y, n, alt),
        CorMethod::Spearman => spearman(x, y, n, alt),
        CorMethod::Kendall => kendall(x, y, n, alt),
    }
}

fn pearson(x: &[f64], y: &[f64], n: usize, alt: Alternative) -> Result<HypothesisTest> {
    let nf = n as f64;
    let mx = crate::extras_mean(x);
    let my = crate::extras_mean(y);
    let mut sxy = 0.0;
    let mut sxx = 0.0;
    let mut syy = 0.0;
    for i in 0..n {
        let dx = x[i] - mx;
        let dy = y[i] - my;
        sxy += dx * dy;
        sxx += dx * dx;
        syy += dy * dy;
    }
    if sxx == 0.0 || syy == 0.0 {
        return Err(HypoError::InvalidInput(
            "zero variance in at least one sample (constant values)".into(),
        ));
    }
    let r = sxy / (sxx * syy).sqrt();
    let df = nf - 2.0;
    let t = r * (df / (1.0 - r * r)).sqrt();
    let p = crate::primitives::t::p_value_t(t, df, alt);
    Ok(HypothesisTest::new(
        t,
        p,
        df,
        alt,
        "Pearson's product-moment correlation",
        extras([
            (KEY_KIND, json!("cor_test")),
            ("method", json!("pearson")),
            ("estimate", json!(r)),
            ("n", json!(n as u64)),
        ]),
    ))
}

fn spearman(x: &[f64], y: &[f64], n: usize, alt: Alternative) -> Result<HypothesisTest> {
    let rx = rank_average(x);
    let ry = rank_average(y);
    // Spearman is Pearson on ranks (with average ties).
    let result = pearson(&rx, &ry, n, alt)?;
    Ok(HypothesisTest::new(
        result.stat,
        result.p_value,
        result.dof,
        result.alternative,
        "Spearman's rank correlation rho",
        {
            let mut e = result.extras.clone();
            e.insert(KEY_KIND.into(), json!("cor_test"));
            e.insert("method".into(), json!("spearman"));
            e
        },
    ))
}

fn kendall(x: &[f64], y: &[f64], n: usize, alt: Alternative) -> Result<HypothesisTest> {
    let nf = n as f64;
    // Count concordant and discordant pairs.
    let mut concordant = 0_i64;
    let mut discordant = 0_i64;
    for i in 0..n {
        for j in (i + 1)..n {
            let sgx = (x[j] - x[i]).signum();
            let sgy = (y[j] - y[i]).signum();
            let prod = sgx * sgy;
            if prod > 0.0 {
                concordant += 1;
            } else if prod < 0.0 {
                discordant += 1;
            }
        }
    }
    let tau = (concordant - discordant) as f64 / (nf * (nf - 1.0) / 2.0);
    // Variance of τ under H₀ (no tie correction for now):
    // var(τ) = 2(2n+5) / (9n(n-1))
    let var_tau = 2.0 * (2.0 * nf + 5.0) / (9.0 * nf * (nf - 1.0));
    let z = tau / var_tau.sqrt();
    let p = match alt {
        Alternative::TwoSided => normal_two_sided_p(z),
        Alternative::Greater => 1.0 - normal_cdf(z),
        Alternative::Less => normal_cdf(z),
    };
    Ok(HypothesisTest::new(
        z,
        p,
        f64::INFINITY,
        alt,
        "Kendall's rank correlation tau",
        extras([
            (KEY_KIND, json!("cor_test")),
            ("method", json!("kendall")),
            ("estimate", json!(tau)),
            ("n", json!(n as u64)),
            ("concordant", json!(concordant)),
            ("discordant", json!(discordant)),
        ]),
    ))
}

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
        // Tied positions i..j get the average of ranks i+1..j.
        let avg = ((i + 1 + j) as f64) / 2.0; // average of (i+1, i+2, ..., j) = (i+1+j)/2
        for k in i..j {
            ranks[idx[k]] = avg;
        }
        i = j;
    }
    ranks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pearson_perfect_positive() {
        let x: Vec<f64> = (1..=10).map(|i| i as f64).collect();
        let y: Vec<f64> = x.iter().map(|&v| 2.0 * v + 1.0).collect();
        let t = cor_test(&x, &y, CorMethod::Pearson, Alternative::TwoSided).unwrap();
        assert!((t.extra_f64("estimate").unwrap() - 1.0).abs() < 1e-12);
        // r=1 → t = +Inf → p ≈ 0
        assert!(t.p_value < 1e-10);
    }

    #[test]
    fn pearson_against_manual() {
        // x = 1..8, y = x + noise
        let x = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let y = vec![2.0, 4.0, 5.0, 7.0, 9.0, 11.0, 12.0, 15.0];
        let t = cor_test(&x, &y, CorMethod::Pearson, Alternative::TwoSided).unwrap();
        // Manual Pearson r ≈ 0.9958
        let r = t.extra_f64("estimate").unwrap();
        assert!((r - 0.9958).abs() < 0.001);
        assert_eq!(t.dof, 6.0);
    }

    #[test]
    fn spearman_matches_pearson_on_monotonic() {
        // For strictly monotonic data, Spearman r = 1.
        let x = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let y = vec![10.0, 20.0, 30.0, 40.0, 50.0];
        let t = cor_test(&x, &y, CorMethod::Spearman, Alternative::TwoSided).unwrap();
        assert!((t.extra_f64("estimate").unwrap() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn kendall_basic() {
        // Strictly increasing → τ = 1
        let x = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let y = vec![10.0, 20.0, 30.0, 40.0, 50.0];
        let t = cor_test(&x, &y, CorMethod::Kendall, Alternative::TwoSided).unwrap();
        assert!((t.extra_f64("estimate").unwrap() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn rejects_bad_inputs() {
        assert!(cor_test(&[1.0], &[1.0], CorMethod::Pearson, Alternative::TwoSided).is_err());
        assert!(
            cor_test(
                &[1.0, 2.0],
                &[1.0],
                CorMethod::Pearson,
                Alternative::TwoSided
            )
            .is_err()
        );
        // Constant data → zero variance
        assert!(
            cor_test(
                &[1.0, 1.0, 1.0],
                &[1.0, 2.0, 3.0],
                CorMethod::Pearson,
                Alternative::TwoSided
            )
            .is_err()
        );
    }

    #[test]
    fn rank_average_ties() {
        let v = vec![3.0, 1.0, 2.0, 1.0];
        let r = rank_average(&v);
        // Sorted: 1,1,2,3 → ranks 1.5,1.5,3,4 → back: 3→4, 1→1.5, 2→3, 1→1.5
        assert!((r[0] - 4.0).abs() < 1e-12);
        assert!((r[1] - 1.5).abs() < 1e-12);
        assert!((r[2] - 3.0).abs() < 1e-12);
        assert!((r[3] - 1.5).abs() < 1e-12);
    }
}
