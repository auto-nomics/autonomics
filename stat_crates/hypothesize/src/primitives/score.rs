//! Score test (Lagrange multiplier) — port of `hypothesize::score_test`.
//!
//! **Univariate** (`fisher_info` scalar):
//! ```text
//! S = U(θ₀)² / I(θ₀)  ~  χ²(1)
//! ```
//!
//! **Multivariate** (`fisher_info` matrix):
//! ```text
//! S = U(θ₀)ᵀ I(θ₀)⁻¹ U(θ₀)  ~  χ²(k)
//! ```
//!
//! The score test is one of the "holy trinity" alongside Wald and LRT. It
//! needs only the score and information at the null, so it is cheapest when
//! the alternative model is expensive to fit.

use serde_json::json;

use super::HypothesisTest;
use crate::{Alternative, HypoError, KEY_KIND, Result, dist::chisq_sf, dist::solve_spd, extras};

/// Univariate score test.
pub fn score_uni(score: f64, fisher_info: f64, null_value: Option<f64>) -> Result<HypothesisTest> {
    if !fisher_info.is_finite() || fisher_info <= 0.0 {
        return Err(HypoError::InvalidInput(format!(
            "'fisher_info' must be positive, got {fisher_info}"
        )));
    }
    let stat = score * score / fisher_info;
    let p_value = chisq_sf(stat, 1.0);
    let mut e = extras([
        (KEY_KIND, json!("score_test")),
        ("score", json!(score)),
        ("fisher_info", json!(fisher_info)),
    ]);
    if let Some(n) = null_value {
        e.insert("null_value".into(), json!(n));
    }
    Ok(HypothesisTest::new(
        stat,
        p_value,
        1.0,
        Alternative::TwoSided,
        "Score Test (univariate)",
        e,
    ))
}

/// Multivariate score test.
///
/// `score` and `fisher_info` are length-`k` / `k×k` respectively.
pub fn score_multi(
    score: &[f64],
    fisher_info: &[Vec<f64>],
    null_value: Option<&[f64]>,
) -> Result<HypothesisTest> {
    let k = score.len();
    if k == 0 {
        return Err(HypoError::InvalidInput("empty score vector".into()));
    }
    if fisher_info.len() != k {
        return Err(HypoError::LengthMismatch {
            a: k,
            b: fisher_info.len(),
        });
    }
    // I⁻¹ · score
    let inv_info_score = solve_spd(fisher_info, score)?;
    let stat = score
        .iter()
        .zip(inv_info_score.iter())
        .map(|(s, x)| s * x)
        .sum::<f64>();
    let p_value = chisq_sf(stat, k as f64);
    let mut e = extras([
        (KEY_KIND, json!("score_test")),
        ("score", json!(score)),
        ("fisher_info", json!(fisher_info)),
        ("k", json!(k as u64)),
    ]);
    if let Some(n) = null_value {
        e.insert("null_value".into(), json!(n));
    }
    Ok(HypothesisTest::new(
        stat,
        p_value,
        k as f64,
        Alternative::TwoSided,
        "Score Test (multivariate)",
        e,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn univariate() {
        // score=2, I=2 → S = 4/2 = 2, χ²(1)
        let t = score_uni(2.0, 2.0, Some(0.0)).unwrap();
        assert!((t.stat - 2.0).abs() < 1e-12);
        assert!((t.p_value - chisq_sf(2.0, 1.0)).abs() < 1e-12);
        assert_eq!(t.extra_f64("null_value"), Some(0.0));
    }

    #[test]
    fn multivariate_diagonal() {
        // U = (1, 2), I = diag(1, 1) → S = 1 + 4 = 5
        let t = score_multi(&[1.0, 2.0], &vec![vec![1.0, 0.0], vec![0.0, 1.0]], None).unwrap();
        assert!((t.stat - 5.0).abs() < 1e-12);
        assert!((t.p_value - chisq_sf(5.0, 2.0)).abs() < 1e-12);
    }

    #[test]
    fn bad_info_rejected() {
        assert!(score_uni(1.0, 0.0, None).is_err());
        assert!(score_uni(1.0, -1.0, None).is_err());
    }
}
