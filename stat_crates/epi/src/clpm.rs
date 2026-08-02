//! Cross-Lagged Panel Model (CLPM) for two-wave longitudinal data.
//!
//! The basic CLPM estimates reciprocal effects between two variables measured
//! at two time points:
//!
//! ```text
//! Y₂ = α_y · Y₁ + β₁ · X₁ + ε_y
//! X₂ = α_x · X₁ + β₂ · Y₁ + ε_x
//! ```
//!
//! `α_y`, `α_x` are **autoregressive** (stability) effects.
//! `β₁` (X₁→Y₂) and `β₂` (Y₁→X₂) are the **cross-lagged** effects — the key
//! parameters of interest. Both regressions are estimated via OLS.
//!
//! For the basic 2-wave, 2-variable model, separate OLS regressions yield the
//! same estimates as FIML/SEM.

use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

use statkit::regression;

use crate::error::{EpiError, Result};

/// Result of a cross-lagged panel model.
#[derive(Debug, Clone)]
pub struct ClpmResult {
    /// Autoregressive effect Y₁ → Y₂ (stability of Y).
    pub ar_y: f64,
    /// SE of AR(Y).
    pub ar_y_se: f64,
    /// p-value of AR(Y).
    pub ar_y_p: f64,
    /// Cross-lagged effect X₁ → Y₂.
    pub cross_xy: f64,
    /// SE of cross-lagged X→Y.
    pub cross_xy_se: f64,
    /// p-value of cross-lagged X→Y.
    pub cross_xy_p: f64,
    /// Autoregressive effect X₁ → X₂ (stability of X).
    pub ar_x: f64,
    /// SE of AR(X).
    pub ar_x_se: f64,
    /// p-value of AR(X).
    pub ar_x_p: f64,
    /// Cross-lagged effect Y₁ → X₂.
    pub cross_yx: f64,
    /// SE of cross-lagged Y→X.
    pub cross_yx_se: f64,
    /// p-value of cross-lagged Y→X.
    pub cross_yx_p: f64,
    /// R² of Y₂ model.
    pub r_sq_y: f64,
    /// R² of X₂ model.
    pub r_sq_x: f64,
    /// Bootstrap 95% CI for cross-lagged X→Y.
    pub cross_xy_ci: (f64, f64),
    /// Bootstrap 95% CI for cross-lagged Y→X.
    pub cross_yx_ci: (f64, f64),
    /// Number of observations.
    pub n_obs: usize,
}

/// Fit a 2-wave cross-lagged panel model.
///
/// `x1`, `y1` are time-1 measurements; `x2`, `y2` are time-2 measurements.
/// Each vector must be the same length (paired data).
pub fn clpm(
    x1: &[f64],
    y1: &[f64],
    x2: &[f64],
    y2: &[f64],
    n_bootstrap: usize,
    seed: u64,
) -> Result<ClpmResult> {
    let n = x1.len();
    if n == 0 || y1.len() != n || x2.len() != n || y2.len() != n {
        return Err(EpiError::DimensionMismatch {
            a: n,
            b: y1.len().max(x2.len()).max(y2.len()),
        });
    }

    // ── OLS: Y₂ ~ Y₁ + X₁ ──────────────────────────────────────────────
    let fit_y = regression::ols(&[y1, x1], y2, true).map_err(epi_from_stat)?;
    // Index: 0=intercept, 1=AR(Y), 2=cross(X→Y)

    // ── OLS: X₂ ~ X₁ + Y₁ ──────────────────────────────────────────────
    let fit_x = regression::ols(&[x1, y1], x2, true).map_err(epi_from_stat)?;
    // Index: 0=intercept, 1=AR(X), 2=cross(Y→X)

    // ── Bootstrap CIs for cross-lagged effects ──────────────────────────
    let (cross_xy_ci, cross_yx_ci) = if n_bootstrap > 0 {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let indices: Vec<usize> = (0..n).collect();
        let mut boot_xy = Vec::with_capacity(n_bootstrap);
        let mut boot_yx = Vec::with_capacity(n_bootstrap);

        for _ in 0..n_bootstrap {
            let idx: Vec<usize> = (0..n)
                .map(|_| indices[(rng.random::<f64>() * n as f64) as usize])
                .collect();
            let x1b: Vec<f64> = idx.iter().map(|&i| x1[i]).collect();
            let y1b: Vec<f64> = idx.iter().map(|&i| y1[i]).collect();
            let x2b: Vec<f64> = idx.iter().map(|&i| x2[i]).collect();
            let y2b: Vec<f64> = idx.iter().map(|&i| y2[i]).collect();

            if let Ok(fy) = regression::ols(&[&y1b, &x1b], &y2b, true) {
                boot_xy.push(fy.coefficients[2]);
            }
            if let Ok(fx) = regression::ols(&[&x1b, &y1b], &x2b, true) {
                boot_yx.push(fx.coefficients[2]);
            }
        }
        (percentile_ci(&boot_xy), percentile_ci(&boot_yx))
    } else {
        ((f64::NAN, f64::NAN), (f64::NAN, f64::NAN))
    };

    Ok(ClpmResult {
        ar_y: fit_y.coefficients[1],
        ar_y_se: fit_y.std_errors[1],
        ar_y_p: fit_y.p_values[1],
        cross_xy: fit_y.coefficients[2],
        cross_xy_se: fit_y.std_errors[2],
        cross_xy_p: fit_y.p_values[2],
        ar_x: fit_x.coefficients[1],
        ar_x_se: fit_x.std_errors[1],
        ar_x_p: fit_x.p_values[1],
        cross_yx: fit_x.coefficients[2],
        cross_yx_se: fit_x.std_errors[2],
        cross_yx_p: fit_x.p_values[2],
        r_sq_y: fit_y.r_squared,
        r_sq_x: fit_x.r_squared,
        cross_xy_ci,
        cross_yx_ci,
        n_obs: n,
    })
}

fn percentile_ci(boot: &[f64]) -> (f64, f64) {
    if boot.len() < 2 {
        return (f64::NAN, f64::NAN);
    }
    let mut sorted = boot.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = sorted.len();
    (
        sorted[(0.025 * n as f64).floor() as usize],
        sorted[(0.975 * n as f64).ceil() as usize],
    )
}

fn epi_from_stat(e: statkit::StatError) -> EpiError {
    EpiError::Numerical(e.to_string())
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * b.abs().max(1.0)
    }

    #[test]
    fn clpm_recovers_known_effects() {
        // Known: Y₂ = 0.5·Y₁ + 0.3·X₁ + noise
        //         X₂ = 0.4·X₁ + 0.2·Y₁ + noise
        let n = 300;
        let mut rng = ChaCha8Rng::seed_from_u64(42);
        let y1: Vec<f64> = (0..n).map(|_| rng.random::<f64>() * 2.0 - 1.0).collect();
        let x1: Vec<f64> = (0..n).map(|_| rng.random::<f64>() * 2.0 - 1.0).collect();
        let y2: Vec<f64> = (0..n)
            .map(|i| 0.5 * y1[i] + 0.3 * x1[i] + rng.random::<f64>() * 0.3 - 0.15)
            .collect();
        let x2: Vec<f64> = (0..n)
            .map(|i| 0.4 * x1[i] + 0.2 * y1[i] + rng.random::<f64>() * 0.3 - 0.15)
            .collect();

        let result = clpm(&x1, &y1, &x2, &y2, 100, 42).unwrap();

        assert!(
            approx_eq(result.ar_y, 0.5, 0.1),
            "AR(Y) ~0.5, got {}",
            result.ar_y
        );
        assert!(
            approx_eq(result.cross_xy, 0.3, 0.1),
            "cross X→Y ~0.3, got {}",
            result.cross_xy
        );
        assert!(
            approx_eq(result.ar_x, 0.4, 0.1),
            "AR(X) ~0.4, got {}",
            result.ar_x
        );
        assert!(
            approx_eq(result.cross_yx, 0.2, 0.1),
            "cross Y→X ~0.2, got {}",
            result.cross_yx
        );
    }

    #[test]
    fn clpm_autoregressive_stronger_than_cross() {
        // Autoregressive effects should typically be stronger than cross-lagged.
        let n = 200;
        let mut rng = ChaCha8Rng::seed_from_u64(99);
        let y1: Vec<f64> = (0..n).map(|_| rng.random::<f64>()).collect();
        let x1: Vec<f64> = (0..n).map(|_| rng.random::<f64>()).collect();
        let y2: Vec<f64> = (0..n)
            .map(|i| 0.7 * y1[i] + 0.1 * x1[i] + rng.random::<f64>() * 0.2)
            .collect();
        let x2: Vec<f64> = (0..n)
            .map(|i| 0.6 * x1[i] + 0.05 * y1[i] + rng.random::<f64>() * 0.2)
            .collect();

        let result = clpm(&x1, &y1, &x2, &y2, 0, 0).unwrap();
        assert!(result.ar_y > result.cross_xy, "AR(Y) > cross(X→Y)");
        assert!(result.ar_x > result.cross_yx, "AR(X) > cross(Y→X)");
    }

    #[test]
    fn clpm_r_squared_positive() {
        let n = 100;
        let mut rng = ChaCha8Rng::seed_from_u64(1);
        let y1: Vec<f64> = (0..n).map(|_| rng.random::<f64>()).collect();
        let x1: Vec<f64> = (0..n).map(|_| rng.random::<f64>()).collect();
        let y2: Vec<f64> = (0..n)
            .map(|i| 0.5 * y1[i] + rng.random::<f64>() * 0.3)
            .collect();
        let x2: Vec<f64> = (0..n)
            .map(|i| 0.5 * x1[i] + rng.random::<f64>() * 0.3)
            .collect();

        let result = clpm(&x1, &y1, &x2, &y2, 0, 0).unwrap();
        assert!(result.r_sq_y > 0.0);
        assert!(result.r_sq_x > 0.0);
    }
}
