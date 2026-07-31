//! Multivariate normal sampling for null-distribution simulation.
//!
//! Faithful to R's `MASS::mvrnorm(n, mu, Sigma, empirical = FALSE)`:
//! uses a Cholesky-based transform `X = mu + Z · Lᵀ` where `L` is the lower
//! Cholesky factor of `Sigma` and `Z` is a matrix of standard normals.
//!
//! R and Rust cannot share an RNG stream, so simulated p-values are validated
//! for convergence rather than exact reproduction (see cross-validation tests).

use faer::{Mat, MatRef};
use rand::Rng;
use rand_distr::{Distribution, Normal};

/// Sample `n` rows from `N(mu, Sigma)` where `mu` is length-K and `Sigma` is
/// K×K. Returns an `n × K` matrix.
///
/// This mirrors `MASS::mvrnorm` with `empirical = FALSE`:
/// 1. Cholesky-decompose `Sigma = L Lᵀ` (lower-triangular `L`).
/// 2. Generate `n × K` standard-normal matrix `Z`.
/// 3. Return `X = 1 · muᵀ + Z · Lᵀ`.
pub fn mvrnorm<R: Rng + ?Sized>(
    n: usize,
    mu: &[f64],
    sigma: MatRef<f64>,
    rng: &mut R,
) -> Mat<f64> {
    let k = mu.len();
    debug_assert_eq!(sigma.nrows(), k);
    debug_assert_eq!(sigma.ncols(), k);

    // Cholesky of Sigma → lower-triangular L (Sigma = L Lᵀ)
    let l = cholesky_lower(sigma);

    let normal = Normal::new(0.0, 1.0).unwrap();
    let mut x = Mat::zeros(n, k);

    for i in 0..n {
        // Generate z ~ N(0, I_K)
        let z: Vec<f64> = (0..k).map(|_| normal.sample(rng)).collect();
        // x[i,:] = mu + z · Lᵀ  →  x[i,j] = mu[j] + sum_m z[m] * L[j,m]
        for j in 0..k {
            let mut s = mu[j];
            for m in 0..=j {
                // L is lower triangular: L[j,m] = 0 for m > j
                s += z[m] * l[(j, m)];
            }
            x[(i, j)] = s;
        }
    }
    x
}

/// Lower-triangular Cholesky factor `L` such that `Sigma = L · Lᵀ`.
///
/// Returns the zero matrix if `Sigma` is not positive-definite (matching R's
/// `mvrnorm` which would error — callers should ensure PD input or handle the
/// fallback).
fn cholesky_lower(sigma: MatRef<f64>) -> Mat<f64> {
    let n = sigma.nrows();
    let mut l = Mat::zeros(n, n);
    for i in 0..n {
        for j in 0..=i {
            let mut sum = 0.0;
            for m in 0..j {
                sum += l[(i, m)] * l[(j, m)];
            }
            let val = sigma[(i, j)] - sum;
            if j == i {
                if val <= 0.0 {
                    // Not PD — return zeros (caller should handle)
                    return Mat::zeros(n, n);
                }
                l[(i, j)] = val.sqrt();
            } else {
                l[(i, j)] = val / l[(j, j)];
            }
        }
    }
    l
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand_chacha::ChaCha8Rng;
    use rand::SeedableRng;

    #[test]
    fn mvrnorm_mean_convergence() {
        let mu = vec![0.0, 0.0];
        let sigma = Mat::from_fn(2, 2, |i, j| if i == j { 1.0 } else { 0.5 });
        let mut rng = ChaCha8Rng::seed_from_u64(42);
        let n = 100_000;
        let x = mvrnorm(n, &mu, sigma.as_ref(), &mut rng);

        // Check sample mean ≈ 0
        let m0: f64 = (0..n).map(|i| x[(i, 0)]).sum::<f64>() / n as f64;
        let m1: f64 = (0..n).map(|i| x[(i, 1)]).sum::<f64>() / n as f64;
        assert!(m0.abs() < 0.02, "mean0 = {m0}");
        assert!(m1.abs() < 0.02, "mean1 = {m1}");

        // Check sample variance ≈ 1
        let v0: f64 = (0..n).map(|i| x[(i, 0)] * x[(i, 0)]).sum::<f64>() / n as f64;
        assert!((v0 - 1.0).abs() < 0.02, "var0 = {v0}");

        // Check sample correlation ≈ 0.5
        let cov01: f64 = (0..n).map(|i| x[(i, 0)] * x[(i, 1)]).sum::<f64>() / n as f64;
        assert!((cov01 - 0.5).abs() < 0.02, "cov01 = {cov01}");
    }
}
