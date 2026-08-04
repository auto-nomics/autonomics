//! Wilkinson's method for combining independent p-values.
//!
//! Uses the `r`-th order statistic of the sorted p-values. Under the global
//! null `p_(r) ~ Beta(r, k+1−r)`, so the combined p-value is
//! `pbeta(p_(r), r, k+1−r)` — the upper-tail `1 − cdf` is used so small
//! order statistics yield small combined p-values.
//!
//! Matches `poolr::wilkinson`.

use serde_json::json;

use super::{PvalSource, extract_pvals, validate_strict};
use crate::{Alternative, HypoError, HypothesisTest, KEY_COMPONENT_PVALS, KEY_KIND, KEY_N_TESTS, Result, extras};

/// Combine via Wilkinson's r-th order statistic. `r` is 1-indexed (`r = 1`
/// is Tippett's min; `r = k` is Tippett's max).
pub fn wilkinson_combine<S: PvalSource>(srcs: &[S], r: usize) -> Result<HypothesisTest> {
    wilkinson_combine_pvals(&extract_pvals(srcs)?, r)
}

/// Same as [`wilkinson_combine`] but takes raw p-values directly.
pub fn wilkinson_combine_pvals(pvals: &[f64], r: usize) -> Result<HypothesisTest> {
    validate_strict(pvals)?;
    let k = pvals.len();
    if k == 0 {
        return Err(HypoError::InvalidInput("no p-values supplied".into()));
    }
    if r == 0 || r > k {
        return Err(HypoError::InvalidInput(format!(
            "r must be in 1..={k}, got {r}"
        )));
    }
    let mut sorted = pvals.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let stat = sorted[r - 1];
    // Beta(r, k+1−r) CDF at stat. Use the regularised incomplete beta via
    // statrs's Beta distribution.
    let p_value = beta_cdf(stat, r as f64, (k + 1 - r) as f64);
    Ok(HypothesisTest::new(
        stat,
        p_value,
        f64::INFINITY,
        Alternative::TwoSided,
        "Wilkinson's Combined Probability Test",
        extras([
            (KEY_KIND, json!("wilkinson_combined_test")),
            (KEY_N_TESTS, json!(k as u64)),
            ("r", json!(r as u64)),
            (KEY_COMPONENT_PVALS, json!(pvals)),
        ]),
    ))
}

/// Regularised incomplete beta `I_x(a, b)` via statrs's Beta distribution.
fn beta_cdf(x: f64, a: f64, b: f64) -> f64 {
    use statrs::distribution::{Beta, ContinuousCDF};
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    Beta::new(a, b).map(|d| d.cdf(x)).unwrap_or(f64::NAN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wilkinson_r1_matches_tippett_min() {
        let pvals = vec![0.1_f64, 0.04, 0.3];
        let w = wilkinson_combine_pvals(&pvals, 1).unwrap();
        // CDF of Beta(1, 3) at 0.04 = 1 - (1 - 0.04)^3
        let expected = 1.0 - (1.0 - 0.04_f64).powi(3);
        assert!((w.p_value - expected).abs() < 1e-9);
    }

    #[test]
    fn wilkinson_rk_matches_tippett_max() {
        let pvals = vec![0.1_f64, 0.04, 0.3];
        let w = wilkinson_combine_pvals(&pvals, 3).unwrap();
        // CDF of Beta(3, 1) at 0.3 = 0.3^3
        assert!((w.p_value - 0.3_f64.powi(3)).abs() < 1e-9);
    }

    #[test]
    fn invalid_r_rejected() {
        assert!(wilkinson_combine_pvals(&[0.1, 0.2], 0).is_err());
        assert!(wilkinson_combine_pvals(&[0.1, 0.2], 3).is_err());
    }
}
