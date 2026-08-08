//! Contingency-table exact tests — Fisher's exact test for 2×2 tables.
//!
//! Port of `stats::fisher.test` for the 2×2 case with two-sided, less, and
//! greater alternatives. Uses log-gamma arithmetic to avoid factorial overflow.

use serde_json::json;

use super::HypothesisTest;
use crate::{Alternative, HypoError, KEY_KIND, Result, extras};

/// Fisher's exact test on a 2×2 contingency table.
///
/// `table` is `[[a, b], [c, d]]` where `a = table[0][0]`. Under H₀ (with
/// fixed marginals), `a` follows a hypergeometric distribution.
///
/// Mirrors `fisher.test(matrix(c(a, b, c, d), 2, 2), alternative = alt)`.
pub fn fisher_exact(
    table: &[[u32; 2]; 2],
    alt: Alternative,
    conf_level: f64,
) -> Result<HypothesisTest> {
    let a = table[0][0];
    let b = table[0][1];
    let c = table[1][0];
    let d = table[1][1];

    let n1 = a + b; // row 1 total
    let m1 = a + c; // col 1 total
    let m2 = b + d; // col 2 total
    let n = a + b + c + d;

    if n == 0 {
        return Err(HypoError::InvalidInput("table total is 0".into()));
    }

    // Support of the hypergeometric: a ranges from lo to hi.
    let lo = n1.saturating_sub(m2);
    let hi = n1.min(m1);

    if lo > hi {
        return Err(HypoError::InvalidInput(
            "inconsistent table marginals".into(),
        ));
    }

    // Compute log-probabilities for each possible table.
    let log_probs: Vec<(u32, f64)> = (lo..=hi)
        .map(|ai| {
            // P(X = ai) = C(n1, ai) * C(n2, m1-ai) / C(n, m1)
            let lp = hypergeom_logpmf(ai, n1, m1, n);
            (ai, lp)
        })
        .collect();

    // P-value computation by alternative.
    let observed_lp = hypergeom_logpmf(a, n1, m1, n);
    let p_value = match alt {
        Alternative::TwoSided => {
            // Sum of probabilities of tables with probability ≤ observed probability.
            let mut sum = 0.0_f64;
            for (_, lp) in &log_probs {
                if *lp <= observed_lp + 1e-12 {
                    sum += lp.exp();
                }
            }
            sum.min(1.0)
        }
        Alternative::Greater => {
            // P(X >= a)
            log_probs
                .iter()
                .filter(|(ai, _)| *ai >= a)
                .map(|(_, lp)| lp.exp())
                .sum::<f64>()
                .min(1.0)
        }
        Alternative::Less => {
            // P(X <= a)
            log_probs
                .iter()
                .filter(|(ai, _)| *ai <= a)
                .map(|(_, lp)| lp.exp())
                .sum::<f64>()
                .min(1.0)
        }
    };

    // Odds ratio (conditional MLE) — used for the CI in R. For now we just
    // report the sample odds ratio as a point estimate.
    let odds_ratio = if b == 0 || c == 0 || d == 0 {
        f64::INFINITY
    } else {
        (a as f64 * d as f64) / (b as f64 * c as f64)
    };

    let method = match alt {
        Alternative::TwoSided => "Fisher's Exact Test for Count Data",
        _ => "Fisher's Exact Test for Count Data (one-sided)",
    };

    Ok(HypothesisTest::new(
        f64::NAN, // Fisher's test has no natural scalar statistic
        p_value,
        f64::INFINITY,
        alt,
        method,
        extras([
            (KEY_KIND, json!("fisher_exact")),
            ("odds_ratio", json!(odds_ratio)),
            ("a", json!(a)),
            ("b", json!(b)),
            ("c", json!(c)),
            ("d", json!(d)),
            ("n", json!(n)),
            ("conf_level", json!(conf_level)),
        ]),
    ))
}

/// Log of the hypergeometric PMF: `ln C(n1, x) + ln C(n2, m1-x) - ln C(n, m1)`.
fn hypergeom_logpmf(x: u32, n1: u32, m1: u32, n: u32) -> f64 {
    let n2 = n - n1;
    let mx = m1 - x;
    ln_binomial(n1, x) + ln_binomial(n2, mx) - ln_binomial(n, m1)
}

/// Log binomial coefficient `ln C(n, k)` using `lgamma`.
fn ln_binomial(n: u32, k: u32) -> f64 {
    if k > n {
        return f64::NEG_INFINITY;
    }
    use statrs::function::gamma::ln_gamma;
    let nf = (n + 1) as f64;
    let kf = (k + 1) as f64;
    let nkf = (n - k + 1) as f64;
    ln_gamma(nf) - ln_gamma(kf) - ln_gamma(nkf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tea_tasting_classic() {
        // Fisher's original Lady Tasting Tea example: 2×2 table
        // R: fisher.test(matrix(c(3,1,1,3), 2, 2))
        // Two-sided p ≈ 0.4857
        let table = [[3, 1], [1, 3]];
        let t = fisher_exact(&table, Alternative::TwoSided, 0.95).unwrap();
        assert!(
            (t.p_value - 0.4857).abs() < 0.01,
            "p = {} (expected ≈ 0.4857)",
            t.p_value
        );
    }

    #[test]
    fn perfect_association() {
        // R: fisher.test(matrix(c(5,0,0,5), 2, 2)) → p ≈ 0.00794
        let table = [[5, 0], [0, 5]];
        let t = fisher_exact(&table, Alternative::TwoSided, 0.95).unwrap();
        assert!(t.p_value < 0.01, "p = {} (expected < 0.01)", t.p_value);
    }

    #[test]
    fn no_association() {
        // R: fisher.test(matrix(c(10,10,10,10), 2, 2)) → p = 1
        let table = [[10, 10], [10, 10]];
        let t = fisher_exact(&table, Alternative::TwoSided, 0.95).unwrap();
        assert!((t.p_value - 1.0).abs() < 1e-9, "p = {}", t.p_value);
    }

    #[test]
    fn one_sided_greater() {
        // R: fisher.test(matrix(c(8,2,2,8), 2, 2), alternative="greater")
        // P(X >= 8) = P(8) + P(9) + P(10) ≈ 0.01151
        let table = [[8, 2], [2, 8]];
        let t = fisher_exact(&table, Alternative::Greater, 0.95).unwrap();
        assert!(
            (t.p_value - 0.011_511).abs() < 0.001,
            "p = {} (expected ≈ 0.0115)",
            t.p_value
        );
    }

    #[test]
    fn one_sided_less() {
        // Symmetric: less with swapped diagonal should match greater.
        let table = [[2, 8], [8, 2]];
        let t = fisher_exact(&table, Alternative::Less, 0.95).unwrap();
        let table2 = [[8, 2], [2, 8]];
        let t2 = fisher_exact(&table2, Alternative::Greater, 0.95).unwrap();
        assert!((t.p_value - t2.p_value).abs() < 1e-9);
    }

    #[test]
    fn rejects_bad_inputs() {
        assert!(fisher_exact(&[[0, 0], [0, 0]], Alternative::TwoSided, 0.95).is_err());
    }

    #[test]
    fn odds_ratio() {
        let table = [[4, 1], [1, 4]];
        let t = fisher_exact(&table, Alternative::TwoSided, 0.95).unwrap();
        let or = t.extra_f64("odds_ratio").unwrap();
        assert!((or - 16.0).abs() < 1e-9, "OR = {or}");
    }
}
