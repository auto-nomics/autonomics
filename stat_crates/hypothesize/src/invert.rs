//! Test↔confidence-set duality — port of `hypothesize::invert_test` plus the
//! analytical CI shortcuts for Wald / z tests.
//!
//! A `(1−α)` confidence set is exactly the set of null values θ₀ for which a
//! level-α test would **not** reject. [`invert_test`] makes that operational
//! for any scalar test via a grid search; [`confint_wald`] and [`confint_z`]
//! are the closed-form shortcuts for the common cases.

use crate::{Alternative, HypoError, HypothesisTest, Result, dist::normal_inv};

/// A confidence set obtained by inverting a hypothesis test.
///
/// `set` is the collection of grid points that would **not** be rejected at
/// level `alpha`. For unimodal continuous tests it is a contiguous interval;
/// for discrete or multimodal tests it can be a union.
#[derive(Clone, Debug)]
pub struct ConfidenceSet {
    /// Grid points not rejected at level `alpha`.
    pub set: Vec<f64>,
    /// Significance level used for inversion.
    pub alpha: f64,
    /// Confidence level `1 − alpha`.
    pub level: f64,
    /// The full candidate grid.
    pub grid: Vec<f64>,
}

impl ConfidenceSet {
    /// Lower bound of the set (`f64::NEG_INFINITY` when empty / unbounded).
    pub fn lower(&self) -> f64 {
        self.set.iter().copied().fold(f64::INFINITY, f64::min)
    }

    /// Upper bound of the set (`f64::INFINITY` when empty / unbounded).
    pub fn upper(&self) -> f64 {
        self.set.iter().copied().fold(f64::NEG_INFINITY, f64::max)
    }
}

/// Invert a scalar test into a confidence set by grid search.
///
/// `test_fn(theta)` must return a [`HypothesisTest`] for the null
/// `H₀: θ = theta`. The confidence set is `{ theta ∈ grid : p_value(theta) ≥ alpha }`.
pub fn invert_test<F>(test_fn: F, grid: &[f64], alpha: f64) -> ConfidenceSet
where
    F: FnMut(f64) -> HypothesisTest,
{
    invert_test_dyn(grid, alpha, test_fn)
}

fn invert_test_dyn<F: FnMut(f64) -> HypothesisTest>(
    grid: &[f64],
    alpha: f64,
    mut test_fn: F,
) -> ConfidenceSet {
    let set: Vec<f64> = grid
        .iter()
        .copied()
        .filter(|&theta| test_fn(theta).p_value >= alpha)
        .collect();
    ConfidenceSet {
        set,
        alpha,
        level: 1.0 - alpha,
        grid: grid.to_vec(),
    }
}

/// Analytical `(1−α)` CI for a univariate Wald test.
///
/// Returns `(lower, upper)`. Requires the test to carry `estimate` and `se`
/// in its `extras` (set by [`crate::wald_uni`]).
pub fn confint_wald(t: &HypothesisTest, level: f64) -> Result<(f64, f64)> {
    let estimate = t
        .extra_f64("estimate")
        .ok_or_else(|| HypoError::InvalidInput("test has no 'estimate' extra".into()))?;
    let se = t
        .extra_f64("se")
        .ok_or_else(|| HypoError::InvalidInput("test has no 'se' extra".into()))?;
    let alpha = 1.0 - level;
    let z = normal_inv(1.0 - alpha / 2.0);
    Ok((estimate - z * se, estimate + z * se))
}

/// Analytical `(1−α)` CI for a z-test, honouring one-sided alternatives.
///
/// Requires `estimate`, `sigma`, and `n` in `extras` (set by [`crate::z_test`]).
/// For `less` the lower bound is `−∞`; for `greater` the upper is `+∞`.
pub fn confint_z(t: &HypothesisTest, level: f64) -> Result<(f64, f64)> {
    let estimate = t
        .extra_f64("estimate")
        .ok_or_else(|| HypoError::InvalidInput("test has no 'estimate' extra".into()))?;
    let sigma = t
        .extra_f64("sigma")
        .ok_or_else(|| HypoError::InvalidInput("test has no 'sigma' extra".into()))?;
    let n = t
        .extra_f64("n")
        .ok_or_else(|| HypoError::InvalidInput("test has no 'n' extra".into()))?;
    let se = sigma / n.sqrt();
    let alpha = 1.0 - level;
    Ok(match t.alternative {
        Alternative::TwoSided => {
            let z = normal_inv(1.0 - alpha / 2.0);
            (estimate - z * se, estimate + z * se)
        }
        Alternative::Less => {
            let z = normal_inv(1.0 - alpha);
            (f64::NEG_INFINITY, estimate + z * se)
        }
        Alternative::Greater => {
            let z = normal_inv(1.0 - alpha);
            (estimate - z * se, f64::INFINITY)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{wald_uni, z_test};

    #[test]
    fn invert_wald_recovers_confint() {
        let est = 2.5_f64;
        let se = 0.8_f64;
        let grid: Vec<f64> = (0..=5000).map(|i| i as f64 * 0.001).collect();
        let cs = invert_test(
            |theta| wald_uni(est, se, theta).unwrap(),
            &grid,
            0.05,
        );
        // Analytical 95% CI: 2.5 ± 1.96*0.8 = (0.932, 4.068)
        let (lo, hi) = confint_wald(&wald_uni(est, se, 0.0).unwrap(), 0.95).unwrap();
        assert!((cs.lower() - lo).abs() < 0.01, "{} vs {}", cs.lower(), lo);
        assert!((cs.upper() - hi).abs() < 0.01, "{} vs {}", cs.upper(), hi);
    }

    #[test]
    fn confint_wald_level() {
        let w = wald_uni(2.5, 0.8, 0.0).unwrap();
        let (lo, hi) = confint_wald(&w, 0.95).unwrap();
        let z = normal_inv(0.975);
        assert!((lo - (2.5 - z * 0.8)).abs() < 1e-9);
        assert!((hi - (2.5 + z * 0.8)).abs() < 1e-9);
    }

    #[test]
    fn confint_z_two_sided() {
        let x = vec![10.0; 50];
        let z = z_test(&x, 9.0, 2.0, Alternative::TwoSided).unwrap();
        let (lo, hi) = confint_z(&z, 0.95).unwrap();
        let z_crit = normal_inv(0.975);
        let se = 2.0 / 50.0_f64.sqrt();
        assert!((lo - (10.0 - z_crit * se)).abs() < 1e-9);
        assert!((hi - (10.0 + z_crit * se)).abs() < 1e-9);
    }

    #[test]
    fn confint_z_one_sided() {
        let x = vec![10.0; 50];
        let z = z_test(&x, 9.0, 2.0, Alternative::Less).unwrap();
        let (lo, hi) = confint_z(&z, 0.95).unwrap();
        assert!(lo.is_infinite() && lo < 0.0);
        let z_crit = normal_inv(0.95);
        let se = 2.0 / 50.0_f64.sqrt();
        assert!((hi - (10.0 + z_crit * se)).abs() < 1e-9);
    }

    #[test]
    fn invert_empty_when_all_rejected() {
        // A test that always rejects (tiny p for every grid point).
        let grid = vec![0.0, 1.0, 2.0];
        let cs = invert_test(|_| wald_uni(1e6, 1.0, 0.0).unwrap(), &grid, 0.05);
        assert!(cs.set.is_empty());
        assert!(cs.lower().is_infinite());
    }
}
