//! Distribution functions matching R's `pchisq` and `pgamma`.
//!
//! Used for computing p-values from SHom (χ²₁) and SHet (shifted gamma)
//! statistics.

use statrs::distribution::{ContinuousCDF, Gamma, GammaError};

/// Chi-squared survival function: `P(X > x)` for `X ~ χ²(df)`.
///
/// Equivalent to R's `pchisq(x, df, lower.tail = FALSE)`.
pub fn pchisq_sf(x: f64, df: f64) -> f64 {
    if x <= 0.0 {
        return 1.0;
    }
    use statrs::distribution::ChiSquared;
    match ChiSquared::new(df) {
        Ok(dist) => 1.0 - dist.cdf(x),
        Err(_) => f64::NAN,
    }
}

/// Gamma survival function: `P(X > x)` for `X ~ Gamma(shape, scale)`.
///
/// Equivalent to R's `pgamma(x, shape, scale, lower.tail = FALSE)`.
///
/// **Note**: R's `pgamma` parameterises by shape and *scale* (not rate).
/// `statrs::Gamma` uses shape (α) and rate (β), so rate = 1/scale.
pub fn pgamma_sf(x: f64, shape: f64, scale: f64) -> f64 {
    if x <= 0.0 {
        return 1.0;
    }
    let rate = 1.0 / scale;
    match Gamma::new(shape, rate) {
        Ok(dist) => 1.0 - dist.cdf(x),
        Err(GammaError::ShapeInvalid) => f64::NAN,
        Err(GammaError::RateInvalid) => f64::NAN,
        Err(_) => f64::NAN,
    }
}

/// SHom p-value from the test statistic.
///
/// `p = pchisq(stat, df = 1, lower.tail = FALSE)`
pub fn shom_pvalue(stat: f64) -> f64 {
    pchisq_sf(stat, 1.0)
}

/// SHet p-value from the test statistic and fitted gamma parameters.
///
/// `p = pgamma(stat - a, shape = k, scale = θ, lower.tail = FALSE)`
pub fn shet_pvalue(stat: f64, params: crate::gamma::GammaParams) -> f64 {
    pgamma_sf(stat - params.shift, params.shape, params.scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pchisq_sf_matches_r() {
        // pchisq(3.84, 1, lower.tail=F) ≈ 0.05
        let p = pchisq_sf(3.841459, 1.0);
        assert!((p - 0.05).abs() < 1e-4, "got {p}");

        // pchisq(6.635, 1, lower.tail=F) ≈ 0.01
        let p = pchisq_sf(6.634897, 1.0);
        assert!((p - 0.01).abs() < 1e-4, "got {p}");
    }

    #[test]
    fn pgamma_sf_matches_r() {
        // pgamma(1, shape=2, scale=3, lower.tail=F)
        // = 1 - F(1) where F is Gamma(shape=2, scale=3)
        // R: pgamma(1, shape=2, scale=3, lower.tail=F) ≈ 0.9728
        let p = pgamma_sf(1.0, 2.0, 3.0);
        // E[X] = 6, so P(X>1) should be high
        assert!(p > 0.95 && p < 0.99, "got {p}");

        // pgamma(6, shape=2, scale=3, lower.tail=F) ≈ 0.4060
        let p = pgamma_sf(6.0, 2.0, 3.0);
        assert!((p - 0.4060).abs() < 0.01, "got {p}");
    }
}
