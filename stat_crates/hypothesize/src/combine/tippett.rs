//! Tippett's method for combining independent p-values.
//!
//! - `min` variant: statistic is the smallest p-value `p_(1)`. Under the
//!   global null `p_(1) ~ Beta(1, k)`, so the combined p-value is
//!   `1 − (1 − p_(1))^k = pbeta(p_(1), 1, k)`.
//! - `max` variant: statistic is the largest p-value `p_(k)`. Under the
//!   global null `p_(k) ~ Beta(k, 1)`, so the combined p-value is
//!   `p_(k)^k = pbeta(p_(k), k, 1)`.
//!
//! Matches `poolr::tippett`.

use serde_json::json;

use super::{PvalSource, extract_pvals, validate_strict};
use crate::{Alternative, HypoError, HypothesisTest, KEY_COMPONENT_PVALS, KEY_KIND, KEY_N_TESTS, Result, extras};

/// Which order statistic Tippett's method uses.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TippettVariant {
    /// Reject for small `min(p)` (the classical Tippett, 1931).
    Min,
    /// Reject for large `max(p)` (the "anti-conservative" Tippett-max used
    /// for testing the *global* null when all p-values should be small
    /// under H₁ in opposite direction).
    Max,
}

/// Combine via Tippett's method.
pub fn tippett_combine<S: PvalSource>(srcs: &[S], variant: TippettVariant) -> Result<HypothesisTest> {
    tippett_combine_pvals(&extract_pvals(srcs)?, variant)
}

/// Same as [`tippett_combine`] but takes raw p-values directly.
pub fn tippett_combine_pvals(pvals: &[f64], variant: TippettVariant) -> Result<HypothesisTest> {
    validate_strict(pvals)?;
    let k = pvals.len();
    if k == 0 {
        return Err(HypoError::InvalidInput("no p-values supplied".into()));
    }
    let (stat, p_value) = match variant {
        TippettVariant::Min => {
            let p_min = pvals.iter().copied().fold(f64::INFINITY, f64::min);
            // Beta(1, k) CDF at x = 1 - (1-x)^k.
            (p_min, 1.0 - (1.0 - p_min).powi(k as i32))
        }
        TippettVariant::Max => {
            let p_max = pvals.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            // Beta(k, 1) CDF at x = x^k.
            (p_max, p_max.powi(k as i32))
        }
    };
    let method = match variant {
        TippettVariant::Min => "Tippett's Combined Probability Test (min)",
        TippettVariant::Max => "Tippett's Combined Probability Test (max)",
    };
    let kind_str = match variant {
        TippettVariant::Min => "tippett_min_combined_test",
        TippettVariant::Max => "tippett_max_combined_test",
    };
    Ok(HypothesisTest::new(
        stat,
        p_value,
        f64::INFINITY,
        Alternative::TwoSided,
        method,
        extras([
            (KEY_KIND, json!(kind_str)),
            (KEY_N_TESTS, json!(k as u64)),
            (KEY_COMPONENT_PVALS, json!(pvals)),
        ]),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tippett_min() {
        let pvals = vec![0.1_f64, 0.04, 0.3];
        let t = tippett_combine_pvals(&pvals, TippettVariant::Min).unwrap();
        assert!((t.stat - 0.04).abs() < 1e-12);
        let expected = 1.0 - (1.0 - 0.04_f64).powi(3);
        assert!((t.p_value - expected).abs() < 1e-12);
    }

    #[test]
    fn tippett_max() {
        let pvals = vec![0.1_f64, 0.04, 0.3];
        let t = tippett_combine_pvals(&pvals, TippettVariant::Max).unwrap();
        assert!((t.stat - 0.3).abs() < 1e-12);
        let expected = 0.3_f64.powi(3);
        assert!((t.p_value - expected).abs() < 1e-12);
    }

    #[test]
    fn single_pval_min() {
        // k = 1: p-value should equal the input.
        let t = tippett_combine_pvals(&[0.07], TippettVariant::Min).unwrap();
        assert!((t.p_value - 0.07).abs() < 1e-12);
    }
}
