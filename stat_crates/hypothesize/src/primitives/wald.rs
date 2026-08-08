//! Wald test — port of `hypothesize::wald_test`.
//!
//! **Univariate** (`se` provided): `z = (θ̂ − θ₀) / SE(θ̂)`, with Wald
//! statistic `W = z² ~ χ²(1)`. The z-score is stored in `extras.z`.
//!
//! **Multivariate** (`vcov` provided):
//! ```text
//! W = (θ̂ − θ₀)ᵀ Σ⁻¹ (θ̂ − θ₀)  ~  χ²(k)
//! ```

use serde_json::json;

use super::HypothesisTest;
use crate::{Alternative, HypoError, KEY_KIND, Result, dist::chisq_sf, dist::solve_spd, extras};

/// Univariate Wald test: `H₀: θ = θ₀` against `H₁: θ ≠ θ₀`.
///
/// `stat` is the χ² statistic `z²`; the z-score itself is in `extras.z`.
/// P-value is two-sided via `P(χ²₁ > z²)`.
pub fn wald_uni(estimate: f64, se: f64, null_value: f64) -> Result<HypothesisTest> {
    if !se.is_finite() || se <= 0.0 {
        return Err(HypoError::InvalidInput(format!(
            "'se' must be positive, got {se}"
        )));
    }
    let z = (estimate - null_value) / se;
    let stat = z * z;
    let p_value = chisq_sf(stat, 1.0);
    Ok(HypothesisTest::new(
        stat,
        p_value,
        1.0,
        Alternative::TwoSided,
        "Wald Test (univariate)",
        extras([
            (KEY_KIND, json!("wald_test")),
            ("z", json!(z)),
            ("estimate", json!(estimate)),
            ("se", json!(se)),
            ("null_value", json!(null_value)),
        ]),
    ))
}

/// Multivariate Wald test: `H₀: θ = θ₀` against `H₁: θ ≠ θ₀`.
///
/// `estimate` and `null_value` are length-`k`; `vcov` is the symmetric
/// positive-definite variance-covariance matrix (row-major).
pub fn wald_multi(
    estimate: &[f64],
    vcov: &[Vec<f64>],
    null_value: &[f64],
) -> Result<HypothesisTest> {
    let k = estimate.len();
    if k == 0 {
        return Err(HypoError::InvalidInput("empty estimate vector".into()));
    }
    if vcov.len() != k {
        return Err(HypoError::LengthMismatch {
            a: k,
            b: vcov.len(),
        });
    }
    if null_value.len() != k {
        return Err(HypoError::LengthMismatch {
            a: k,
            b: null_value.len(),
        });
    }
    let diff: Vec<f64> = estimate
        .iter()
        .zip(null_value)
        .map(|(e, n)| e - n)
        .collect();
    // Σ⁻¹ diff
    let inv_vcov_diff = solve_spd(vcov, &diff)?;
    let stat = diff
        .iter()
        .zip(inv_vcov_diff.iter())
        .map(|(d, s)| d * s)
        .sum::<f64>();
    let p_value = chisq_sf(stat, k as f64);
    Ok(HypothesisTest::new(
        stat,
        p_value,
        k as f64,
        Alternative::TwoSided,
        "Wald Test (multivariate)",
        extras([
            (KEY_KIND, json!("wald_test")),
            ("estimate", json!(estimate)),
            ("vcov", json!(vcov)),
            ("null_value", json!(null_value)),
            ("k", json!(k as u64)),
        ]),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dist::normal_two_sided_p;

    #[test]
    fn univariate_matches_z_squared() {
        let w = wald_uni(2.5, 0.8, 0.0).unwrap();
        let z = 2.5_f64 / 0.8;
        assert!((w.stat - z * z).abs() < 1e-12);
        assert!((w.p_value - normal_two_sided_p(z)).abs() < 1e-12);
        assert_eq!(w.extra_f64("z"), Some(z));
    }

    #[test]
    fn univariate_nonzero_null() {
        let w = wald_uni(2.5, 0.8, 2.0).unwrap();
        let z = (2.5 - 2.0) / 0.8;
        assert!((w.extra_f64("z").unwrap() - z).abs() < 1e-12);
    }

    #[test]
    fn multivariate_known_result() {
        // θ̂ = (2, 3), Σ = [[1, 0.3], [0.3, 1]], θ₀ = (0, 0)
        let est = vec![2.0, 3.0];
        let vcov = vec![vec![1.0, 0.3], vec![0.3, 1.0]];
        let w = wald_multi(&est, &vcov, &[0.0, 0.0]).unwrap();
        // Manual: Σ⁻¹ = (1/0.91) * [[1, -0.3], [-0.3, 1]]
        // diff = (2, 3); diff' Σ⁻¹ diff = (1/0.91) * (4 - 1.2 - 1.8 + 9) wait recompute
        // diff' Σ⁻¹ diff = (1/0.91) * [2 3] · [[1,-0.3],[-0.3,1]] · [2,3]ᵀ
        // inner = [2 - 0.9, -0.6 + 3] = [1.1, 2.4]
        // dot with [2, 3] = 2.2 + 7.2 = 9.4 → /0.91 = 10.3297
        assert!((w.stat - 9.4 / 0.91).abs() < 1e-9);
        assert!((w.p_value - chisq_sf(w.stat, 2.0)).abs() < 1e-12);
    }

    #[test]
    fn multivariate_singular_rejected() {
        let r = wald_multi(
            &[1.0, 1.0],
            &[vec![1.0, 1.0], vec![1.0, 1.0]],
            &[0.0, 0.0],
        );
        assert!(r.is_err());
    }

    #[test]
    fn rejects_bad_se() {
        assert!(wald_uni(0.0, 0.0, 0.0).is_err());
        assert!(wald_uni(0.0, -1.0, 0.0).is_err());
    }
}
