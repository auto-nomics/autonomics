//! Shifted-gamma null distribution for the **SHet** statistic.
//!
//! Direct port of `EstimateGamma` and `EmpDist` from CPASSOC's `FunctionSet.R`.
//!
//! The SHet statistic under the null does not follow a standard distribution
//! but is well-approximated by a 3-parameter shifted gamma `Gamma(k, θ) + c`.
//! The parameters `(k, θ, c)` are estimated by matching the first three moments
//! of simulated SHet statistics (generated from `MVN(0, R)` null draws).

use crate::mvn::mvrnorm;
use crate::stats::{shet, ShetOptions};
use faer::Mat;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

/// Fitted shifted-gamma parameters `(k, θ, a)` where the null distribution is
/// `Gamma(k, θ) + a`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GammaParams {
    /// Shape parameter `k` (R: `para[1]`).
    pub shape: f64,
    /// Scale parameter `θ` (R: `para[2]`).
    pub scale: f64,
    /// Shift parameter `a` (R: `para[3]`).
    pub shift: f64,
}

/// Estimate the shifted-gamma null distribution for SHet.
///
/// Faithful port of `EstimateGamma`:
/// 1. Generate `N` draws from `MVN(0, CorrMatrix)`.
/// 2. Compute SHet for each draw.
/// 3. Fit `Gamma(k, θ) + a` by iterative moment matching (100 iterations).
///
/// `seed` controls the RNG; the R reference uses `MASS::mvrnorm` which has no
/// seed parameter, so simulated statistics are distributionally validated.
pub fn estimate_gamma(
    n: usize,
    sample_size: &[f64],
    corr_matrix: &Mat<f64>,
    opts: ShetOptions,
    seed: u64,
) -> GammaParams {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let k = sample_size.len();
    let mu = vec![0.0; k];

    // Step 1-2: Permutation = mvrnorm(N, 0, CorrMatrix); Stat = SHet(Permutation)
    let permutation = mvrnorm(n, &mu, corr_matrix.as_ref(), &mut rng);
    let stat = shet(&permutation, sample_size, corr_matrix, opts);

    // Step 3: iterative moment matching
    fit_shifted_gamma(&stat)
}

/// Port of the moment-matching loop in `EstimateGamma`.
///
/// ```text
/// a   = min(Stat) * 3/4
/// ex3 = mean(Stat³)
/// V   = var(Stat)        # R's var() uses n-1 divisor
/// repeat 100×:
///   E = mean(Stat) - a
///   k = E² / V
///   θ = V / E
///   a = (-3k(k+1)θ² + √(9k²(k+1)²θ⁴ - 12kθ(k(k+1)(k+2)θ³ - ex3))) / (6kθ)
/// ```
pub fn fit_shifted_gamma(stat: &[f64]) -> GammaParams {
    let n = stat.len() as f64;
    let mean: f64 = stat.iter().sum::<f64>() / n;
    let ex3: f64 = stat.iter().map(|s| s * s * s).sum::<f64>() / n;

    // R's var() uses n-1 divisor
    let v: f64 = if n > 1.0 {
        let ss: f64 = stat.iter().map(|s| (s - mean).powi(2)).sum::<f64>();
        ss / (n - 1.0)
    } else {
        0.0
    };

    let mut a = stat.iter().cloned().fold(f64::INFINITY, f64::min) * 3.0 / 4.0;

    let mut shape = 0.0_f64;
    let mut scale = 0.0_f64;

    for _ in 0..100 {
        let e = mean - a;
        shape = e * e / v;
        scale = v / e;

        // a = (-3k(k+1)θ² + √(9k²(k+1)²θ⁴ - 12kθ(k(k+1)(k+2)θ³ - ex3))) / (6kθ)
        let kk = shape;
        let theta = scale;
        let term1 = 3.0 * kk * (kk + 1.0) * theta * theta;
        let term2 = 9.0 * kk * kk * (kk + 1.0) * (kk + 1.0) * theta.powi(4);
        let term3_inner = kk * (kk + 1.0) * (kk + 2.0) * theta.powi(3) - ex3;
        let term2_full = term2 - 12.0 * kk * theta * term3_inner;
        let disc = term2_full.max(0.0).sqrt();
        a = (-term1 + disc) / (6.0 * kk * theta);
    }

    GammaParams {
        shape,
        scale,
        shift: a,
    }
}

/// Empirical null distribution of SHet (alternative to the gamma approximation).
///
/// Port of `EmpDist`. Returns the raw vector of `N` simulated SHet statistics.
/// Use [`empirical_pvalue`] to compute p-values from this distribution.
pub fn emp_dist(
    n: usize,
    sample_size: &[f64],
    corr_matrix: &Mat<f64>,
    opts: ShetOptions,
    seed: u64,
) -> Vec<f64> {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let k = sample_size.len();
    let mu = vec![0.0; k];
    let permutation = mvrnorm(n, &mu, corr_matrix.as_ref(), &mut rng);
    shet(&permutation, sample_size, corr_matrix, opts)
}

/// Empirical p-value: `P(stat ≥ observed)` from the simulated null.
///
/// Matches R usage: `mean(Stat >= observed)`.
pub fn empirical_pvalue(null_stats: &[f64], observed: f64) -> f64 {
    let count = null_stats.iter().filter(|&&s| s >= observed).count();
    count as f64 / null_stats.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;

    #[test]
    fn fit_shifted_gamma_recovers_params() {
        // Generate from Gamma(2, 3) + 1 and check recovery
        let mut rng = ChaCha8Rng::seed_from_u64(123);
        let normal = rand_distr::Normal::new(0.0, 1.0).unwrap();
        // Use Box-Muller to generate gamma-ish data... actually use a simple
        // sum-of-exponentials for Gamma(2,3)+1
        let n = 200_000;
        let stat: Vec<f64> = (0..n)
            .map(|_| {
                // Gamma(2,3) = sum of 2 Exp(1/3) = -3*ln(U1) - 3*ln(U2)
                let u1: f64 = rng.random_range(1e-10..1.0);
                let u2: f64 = rng.random_range(1e-10..1.0);
                -3.0 * (u1.ln() + u2.ln()) + 1.0
            })
            .collect();
        let params = fit_shifted_gamma(&stat);
        assert!((params.shape - 2.0).abs() < 0.1, "shape = {}", params.shape);
        assert!((params.scale - 3.0).abs() < 0.15, "scale = {}", params.scale);
        assert!((params.shift - 1.0).abs() < 0.15, "shift = {}", params.shift);
        let _ = normal; // suppress unused warning
    }
}
