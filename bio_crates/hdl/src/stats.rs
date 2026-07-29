//! Distribution helpers for HDL-L inference.
//!
//! The LRT P value uses the χ² survival function, re-exported from
//! [`lava::stats`] (which itself wraps [`statrs`], matching R's `pchisq`).
//! The likelihood-based CI cutoff needs the χ² *quantile* `qchisq(1-alpha, 1)`,
//! which lava does not expose — implemented here via [`statrs`].

pub use lava::stats::pchisq_sf;

use statrs::distribution::{ChiSquared, ContinuousCDF};

/// χ² quantile (inverse CDF), matching R's `qchisq(p, df)` (lower tail).
///
/// Used for the likelihood-based CI cutoff:
/// `c = exp(-qchisq(1 - alpha, 1) / 2)` (e.g. alpha = 0.05 → 95% CI).
pub fn qchisq(p: f64, df: u32) -> f64 {
    ChiSquared::new(df as f64).expect("valid df").inverse_cdf(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qchisq_matches_r() {
        // R: qchisq(0.95, 1) ≈ 3.841459
        let v = qchisq(0.95, 1);
        assert!((v - 3.841459).abs() < 1e-5, "got {v}");
    }
}
