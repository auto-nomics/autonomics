//! Fisher's method for combining independent p-values.
//!
//! ```text
//! X² = −2 · Σᵢ log(pᵢ)  ~  χ²(2k)
//! ```
//!
//! Under the global null, every pᵢ ~ U(0,1) so −2·log(pᵢ) ~ χ²(2); the sum
//! of independent χ² is χ² with summed dof. Fisher's statistic is therefore
//! χ² with `2k` degrees of freedom.
//!
//! Port of `hypothesize::fisher_combine`.

use serde_json::json;

use super::{PvalSource, extract_pvals, validate_strict};
use crate::{
    Alternative, HypothesisTest, KEY_COMPONENT_PVALS, KEY_KIND, KEY_N_TESTS, Result,
    dist::chisq_sf, extras,
};

/// Combine independent p-values or tests via Fisher's method.
pub fn fisher_combine<S: PvalSource>(srcs: &[S]) -> Result<HypothesisTest> {
    fisher_combine_pvals(&extract_pvals(srcs)?)
}

/// Same as [`fisher_combine`] but takes raw p-values directly.
pub fn fisher_combine_pvals(pvals: &[f64]) -> Result<HypothesisTest> {
    validate_strict(pvals)?;
    let k = pvals.len();
    if k == 0 {
        return Err(crate::HypoError::InvalidInput(
            "no p-values supplied".into(),
        ));
    }
    let stat = -2.0 * pvals.iter().map(|p| p.ln()).sum::<f64>();
    let dof = 2.0 * k as f64;
    let p_value = chisq_sf(stat, dof);
    Ok(HypothesisTest::new(
        stat,
        p_value,
        dof,
        Alternative::TwoSided,
        "Fisher's Combined Probability Test",
        extras([
            (KEY_KIND, json!("fisher_combined_test")),
            (KEY_N_TESTS, json!(k as u64)),
            (KEY_COMPONENT_PVALS, json!(pvals)),
        ]),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fisher_two_pvals() {
        // p = (0.08, 0.12, 0.04) → X² = -2*sum(log) ≈ 12.9296
        let pvals = vec![0.08_f64, 0.12, 0.04];
        let t = fisher_combine_pvals(&pvals).unwrap();
        let expected_stat = -2.0 * (0.08_f64).ln() - 2.0 * (0.12_f64).ln() - 2.0 * (0.04_f64).ln();
        assert!((t.stat - expected_stat).abs() < 1e-9);
        assert!((t.p_value - chisq_sf(expected_stat, 6.0)).abs() < 1e-9);
        assert_eq!(t.dof, 6.0);
        assert_eq!(t.extra_f64("n_tests"), Some(3.0));
    }

    #[test]
    fn fisher_accepts_tests() {
        let z = crate::z_test(&[1.0; 10], 0.0, 1.0, Alternative::TwoSided).unwrap();
        let z_p = z.p_value;
        let t = fisher_combine(&[z.clone(), z]).unwrap();
        assert!(t.p_value < z_p);
        // χ²(4)
        assert_eq!(t.dof, 4.0);
    }

    #[test]
    fn rejects_zero_pval() {
        assert!(fisher_combine_pvals(&[0.0, 0.5]).is_err());
        assert!(fisher_combine_pvals(&[0.5, 1.5]).is_err());
    }

    #[test]
    fn empty_rejected() {
        assert!(fisher_combine_pvals(&[]).is_err());
    }
}
