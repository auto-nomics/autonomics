//! Statistical distribution functions reproducing R's `pnorm`, `qnorm`,
//! `dnorm`, `qchisq`, `pchisq`.

use statrs::distribution::{ChiSquared, Continuous, ContinuousCDF, Normal};

/// Standard normal CDF: R's `pnorm(q)`.
pub fn pnorm(q: f64) -> f64 {
    Normal::standard().cdf(q)
}

/// Standard normal survival function: R's `pnorm(q, lower.tail = FALSE)`.
pub fn pnorm_sf(q: f64) -> f64 {
    1.0 - pnorm(q)
}

/// Two-sided normal tail: `2 * pnorm(-|z|)`.
pub fn pnorm_two_sided(z: f64) -> f64 {
    2.0 * pnorm(-z.abs())
}

/// Standard normal quantile: R's `qnorm(p)`.
pub fn qnorm(p: f64) -> f64 {
    Normal::standard().inverse_cdf(p)
}

/// Standard normal PDF: R's `dnorm(x)`.
pub fn dnorm(x: f64) -> f64 {
    Normal::standard().pdf(x)
}

/// χ² CDF with `df` degrees of freedom: R's `pchisq(q, df)`.
pub fn pchisq(q: f64, df: f64) -> f64 {
    if q <= 0.0 {
        return 0.0;
    }
    match ChiSquared::new(df) {
        Ok(dist) => dist.cdf(q),
        Err(_) => f64::NAN,
    }
}

/// χ² survival function: R's `pchisq(q, df, lower.tail = FALSE)`.
pub fn pchisq_sf(q: f64, df: f64) -> f64 {
    if q <= 0.0 {
        return 1.0;
    }
    match ChiSquared::new(df) {
        Ok(dist) => 1.0 - dist.cdf(q),
        Err(_) => f64::NAN,
    }
}

/// χ² quantile: R's `qchisq(p, df)`.
pub fn qchisq(p: f64, df: f64) -> f64 {
    match ChiSquared::new(df) {
        Ok(dist) => dist.inverse_cdf(p),
        Err(_) => f64::NAN,
    }
}

/// χ² survival quantile: R's `qchisq(p, df, lower.tail = FALSE)`.
pub fn qchisq_sf(p: f64, df: f64) -> f64 {
    qchisq(1.0 - p, df)
}

/// Median of a Chi-squared with 1 df, used by LDSC for λ_GC:
/// `qchisq(0.5, df = 1)` ≈ 0.4549.
pub fn median_chi2_1() -> f64 {
    qchisq(0.5, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pnorm() {
        assert!((pnorm(0.0) - 0.5).abs() < 1e-10);
        assert!((pnorm(1.96) - 0.975).abs() < 1e-3);
    }

    #[test]
    fn test_qnorm() {
        assert!((qnorm(0.5) - 0.0).abs() < 1e-10);
        assert!((qnorm(0.975) - 1.96).abs() < 1e-3);
    }

    #[test]
    fn test_pnorm_two_sided() {
        let z = 1.96;
        let p = pnorm_two_sided(z);
        assert!((p - 0.05).abs() < 1e-3);
    }

    #[test]
    fn test_pchisq() {
        assert!((pchisq(3.84, 1.0) - 0.95).abs() < 1e-2);
    }

    #[test]
    fn test_qchisq() {
        assert!((qchisq(0.95, 1.0) - 3.84).abs() < 1e-2);
    }
}
