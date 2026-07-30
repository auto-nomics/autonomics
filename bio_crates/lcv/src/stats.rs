//! Distribution helpers reproducing R's Student's t functions (`dt` / `pt`),
//! used by the LCV likelihood grid and p-value computation.
//!
//! The LCV reference calls:
//!   - `dt(x, df)` — t density at `x` with `df` degrees of freedom (likelihood grid).
//!   - `pt(q, df)` — t CDF (lower tail) (fully-causal and gcp-zero p-values).
//!
//! Both delegate to [`statrs`], which matches R's implementation to machine
//! precision.

use statrs::distribution::{Continuous, ContinuousCDF, StudentsT};

/// Student's t density at `x` with `df` degrees of freedom — R `dt(x, df)`.
pub fn dt(x: f64, df: f64) -> f64 {
    StudentsT::new(0.0, 1.0, df).expect("valid df").pdf(x)
}

/// Student's t CDF `P(T ≤ q)` with `df` degrees of freedom — R `pt(q, df)`.
pub fn pt(q: f64, df: f64) -> f64 {
    StudentsT::new(0.0, 1.0, df).expect("valid df").cdf(q)
}

/// Two-tailed t-distribution p-value — R `2 * pt(-abs(t), df)`.
pub fn pt_two_tailed(t: f64, df: f64) -> f64 {
    2.0 * pt(-t.abs(), df)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dt_matches_r() {
        // R: dt(1.5, 10) = 0.1274448
        assert!((dt(1.5, 10.0) - 0.1274448).abs() < 1e-6);
    }

    #[test]
    fn pt_matches_r() {
        // R: pt(1.5, 10) = 0.9177463
        assert!((pt(1.5, 10.0) - 0.9177463).abs() < 1e-6);
        // R: pt(-1.5, 10) = 0.08225366
        assert!((pt(-1.5, 10.0) - 0.08225366).abs() < 1e-6);
    }
}
