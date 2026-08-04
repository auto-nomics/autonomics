//! One-sample z-test — port of `hypothesize::z_test`.
//!
//! Tests `H₀: μ = μ₀` against `alternative` when the population standard
//! deviation σ is known:
//!
//! ```text
//! z = (x̄ − μ₀) / (σ / √n)
//! ```
//!
//! Under H₀, `z ~ N(0, 1)`. P-values:
//! - two-sided: `2·Φ(−|z|)`
//! - less:      `Φ(z)`
//! - greater:   `1 − Φ(z)`
//!
//! The z-test is the simplest member of the Wald family; see [`super::wald`]
//! for the general univariate / multivariate Wald test.

use serde_json::json;

use super::HypothesisTest;
use crate::{Alternative, HypoError, KEY_KIND, Result, dist::normal_two_sided_p, extras};
use crate::dist::{normal_cdf, normal_sf};

/// One-sample z-test with known population σ.
///
/// Mirrors `hypothesize::z_test(x, mu0, sigma, alternative)`.
pub fn z_test(x: &[f64], mu0: f64, sigma: f64, alternative: Alternative) -> Result<HypothesisTest> {
    if x.is_empty() {
        return Err(HypoError::InvalidInput("'x' must contain at least one observation".into()));
    }
    if !sigma.is_finite() || sigma <= 0.0 {
        return Err(HypoError::InvalidInput(format!("'sigma' must be positive, got {sigma}")));
    }
    let n = x.len() as f64;
    let xbar = x.iter().copied().sum::<f64>() / n;
    let se = sigma / n.sqrt();
    let z = (xbar - mu0) / se;

    let p_value = match alternative {
        Alternative::TwoSided => normal_two_sided_p(z),
        Alternative::Less => normal_cdf(z),
        Alternative::Greater => normal_sf(z),
    };

    Ok(HypothesisTest::new(
        z,
        p_value,
        f64::INFINITY,
        alternative,
        "One Sample z-test",
        extras([
            (KEY_KIND, json!("z_test")),
            ("null_value", json!(mu0)),
            ("estimate", json!(xbar)),
            ("sigma", json!(sigma)),
            ("n", json!(x.len() as u64)),
            ("stderr", json!(se)),
        ]),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn z_test_basic_two_sided() {
        // mean=10, sigma=2, n=50, mu0=9 → z = (10-9)/(2/√50) ≈ 3.5355
        let x: Vec<f64> = (0..50).map(|_| 10.0).collect();
        let t = z_test(&x, 9.0, 2.0, Alternative::TwoSided).unwrap();
        assert!((t.stat - 3.535_534).abs() < 1e-5);
        // 2 * pnorm(-3.5355) ≈ 4.0e-4
        assert!((t.p_value - 2.0 * normal_cdf(-3.535_534)).abs() < 1e-9);
        assert_eq!(t.method, "One Sample z-test");
        assert_eq!(t.alternative, Alternative::TwoSided);
        assert!(t.dof.is_infinite());
        assert_eq!(t.extra_f64("n"), Some(50.0));
    }

    #[test]
    fn z_test_directions() {
        let x = vec![1.0; 10];
        let z_greater = z_test(&x, 0.0, 1.0, Alternative::Greater).unwrap();
        let z_less = z_test(&x, 0.0, 1.0, Alternative::Less).unwrap();
        assert!((z_greater.p_value + z_less.p_value - 1.0).abs() < 1e-12);
    }

    #[test]
    fn rejects_bad_inputs() {
        assert!(z_test(&[], 0.0, 1.0, Alternative::TwoSided).is_err());
        assert!(z_test(&[1.0], 0.0, 0.0, Alternative::TwoSided).is_err());
        assert!(z_test(&[1.0], 0.0, -1.0, Alternative::TwoSided).is_err());
    }
}
