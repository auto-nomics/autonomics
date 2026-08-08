//! Likelihood ratio test — port of `hypothesize::lrt`.
//!
//! ```text
//! Λ = −2 · (ℓ₀ − ℓ₁) = −2 · log(L₀ / L₁)  ~  χ²(df)
//! ```
//!
//! `df` is typically the difference in the number of free parameters between
//! the alternative and null models. When both inputs are [`LogLik`] values
//! carrying `df`, the difference is auto-derived (matching R's `logLik`
//! branch).

use serde_json::json;

use super::HypothesisTest;
use crate::{Alternative, HypoError, KEY_KIND, Result, dist::chisq_sf, extras};

/// A maximised log-likelihood with its parameter count.
///
/// Mirrors R's `logLik` objects, whose `df` attribute holds the number of
/// free parameters. When two `LogLik`s are passed to [`lrt`] the difference
/// `df_alt − df_null` is used as the LRT degrees of freedom.
#[derive(Clone, Copy, Debug)]
pub struct LogLik {
    /// Maximised log-likelihood value.
    pub loglik: f64,
    /// Number of free parameters in the model.
    pub df: u32,
}

impl LogLik {
    pub fn new(loglik: f64, df: u32) -> Self {
        Self { loglik, df }
    }
}

/// Likelihood ratio test comparing nested models.
///
/// Inputs are either bare log-likelihood scalars plus an explicit `dof`, or
/// [`LogLik`] values whose `df` difference yields `dof` automatically.
pub fn lrt<I, A>(null: I, alt: A, dof: Option<u32>) -> Result<HypothesisTest>
where
    I: LrtInput,
    A: LrtInput,
{
    let (null_loglik, null_df) = null.into_loglik();
    let (alt_loglik, alt_df) = alt.into_loglik();
    let dof = match dof {
        Some(d) => d,
        None => match (null_df, alt_df) {
            (Some(a), Some(b)) => b.saturating_sub(a),
            _ => {
                return Err(HypoError::InvalidInput(
                    "'dof' is required when inputs are not both LogLik".into(),
                ));
            }
        },
    };
    if dof == 0 {
        return Err(HypoError::InvalidInput("'dof' must be positive".into()));
    }

    let stat = -2.0 * (null_loglik - alt_loglik);
    let p_value = chisq_sf(stat.max(0.0), dof as f64);

    Ok(HypothesisTest::new(
        stat,
        p_value,
        dof as f64,
        Alternative::TwoSided,
        "Likelihood Ratio Test",
        extras([
            (KEY_KIND, json!("likelihood_ratio_test")),
            ("null_loglik", json!(null_loglik)),
            ("alt_loglik", json!(alt_loglik)),
            ("dof", json!(dof)),
        ]),
    ))
}

/// Internal trait so [`lrt`] accepts `f64` and [`LogLik`] interchangeably.
///
/// Sealed — only `f64` and [`LogLik`] implement it.
pub trait LrtInput: private::Sealed {
    #[doc(hidden)]
    fn into_loglik(self) -> (f64, Option<u32>);
}

impl LrtInput for f64 {
    fn into_loglik(self) -> (f64, Option<u32>) {
        (self, None)
    }
}

impl LrtInput for LogLik {
    fn into_loglik(self) -> (f64, Option<u32>) {
        (self.loglik, Some(self.df))
    }
}

mod private {
    pub trait Sealed {}
    impl Sealed for f64 {}
    impl Sealed for super::LogLik {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_lrt_from_scalars() {
        // ℓ₀ = -150, ℓ₁ = -140, df = 3 → Λ = 20, p = P(χ²₃ > 20)
        let t = lrt(-150.0, -140.0, Some(3)).unwrap();
        assert!((t.stat - 20.0).abs() < 1e-12);
        assert!((t.p_value - chisq_sf(20.0, 3.0)).abs() < 1e-12);
        assert_eq!(t.dof, 3.0);
    }

    #[test]
    fn auto_dof_from_loglik() {
        let n = LogLik::new(-150.0, 2);
        let a = LogLik::new(-140.0, 5);
        let t = lrt(n, a, None).unwrap();
        assert_eq!(t.dof, 3.0);
        assert!((t.stat - 20.0).abs() < 1e-12);
    }

    #[test]
    fn mixed_inputs_require_dof() {
        let r = lrt(-150.0, LogLik::new(-140.0, 5), None);
        assert!(r.is_err());
    }

    #[test]
    fn zero_dof_rejected() {
        assert!(lrt(-1.0, 0.0, Some(0)).is_err());
        // equal-df LogLik ⇒ df = 0 ⇒ error
        let r = lrt(LogLik::new(-1.0, 3), LogLik::new(0.0, 3), None);
        assert!(r.is_err());
    }

    #[test]
    fn negative_stat_clamped_for_pval() {
        // ℓ₀ > ℓ₁ (unusual): stat negative, p-value computed from 0.
        let t = lrt(-1.0, -2.0, Some(2)).unwrap();
        assert!(t.stat < 0.0);
        assert!((t.p_value - 1.0).abs() < 1e-12);
    }
}
