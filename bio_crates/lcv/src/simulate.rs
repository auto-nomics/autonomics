//! Simulation of summary statistics under the LCV model, ported from
//! `LCV/R/SimulateLCV.R`.
//!
//! Generates per-SNP "estimated effect sizes" (Z-scores) for two traits under
//! the latent-causal-variable model: a shared latent factor `L` with effects
//! `q₁`, `q₂` on the two traits, plus trait-specific pleiotropic components.
//!
//! **Note**: the R reference uses R's Mersenne-Twister RNG; this port uses a
//! seeded `ChaCha8Rng`. The two cannot share a random stream, so simulated
//! arrays will differ between R and Rust for the same seed — but the
//! statistical model is identical.

use rand::{Rng, SeedableRng};
use rand_distr::Normal;

/// Parameters for [`simulate_lcv`].
#[derive(Debug, Clone)]
pub struct SimParams {
    /// Number of SNPs.
    pub m: usize,
    /// Sample size for trait 1.
    pub n1: f64,
    /// Sample size for trait 2.
    pub n2: f64,
    /// Heritability of trait 1.
    pub h2_1: f64,
    /// Heritability of trait 2.
    pub h2_2: f64,
    /// Effect of latent factor L on trait 1.
    pub q1: f64,
    /// Effect of latent factor L on trait 2.
    pub q2: f64,
    /// Proportion of SNPs causal for L.
    pub p_pi: f64,
    /// Proportion of SNPs causal for trait 1 only.
    pub p_g1: f64,
    /// Proportion of SNPs causal for trait 2 only.
    pub p_g2: f64,
}

/// Simulated summary statistics: `(z1, z2)` — the estimated per-normalised
/// effect sizes on the two traits.
pub fn simulate_lcv(params: &SimParams, seed: u64) -> (Vec<f64>, Vec<f64>) {
    let m = params.m;
    let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(seed);
    let normal = Normal::new(0.0, 1.0).unwrap();

    // ── Latent factor causal effects (π) ──
    // n_pi causal (N(0,1)), rest zero; shuffle; normalise to var = 1.
    let pi = make_sparse_component(m, params.p_pi, &mut rng, &normal);

    // ── Trait-specific pleiotropic effects (γ) ──
    // Normalise to var = 1, then scale by √(1−q²).
    let gamma1 = make_sparse_component(m, params.p_g1, &mut rng, &normal)
        .iter()
        .map(|&v| v * (1.0 - params.q1 * params.q1).sqrt())
        .collect::<Vec<_>>();
    let gamma2 = make_sparse_component(m, params.p_g2, &mut rng, &normal)
        .iter()
        .map(|&v| v * (1.0 - params.q2 * params.q2).sqrt())
        .collect::<Vec<_>>();

    // ── True causal effect sizes (β) ──
    // b_k = q_k·π + γ_k; normalise to ||b||=1 then scale by √h².
    let mut b1: Vec<f64> = (0..m).map(|i| params.q1 * pi[i] + gamma1[i]).collect();
    let mut b2: Vec<f64> = (0..m).map(|i| params.q2 * pi[i] + gamma2[i]).collect();
    normalise_norm(&mut b1, params.h2_1.sqrt());
    normalise_norm(&mut b2, params.h2_2.sqrt());

    // ── Estimated effect sizes (Z) = β + N(0, 1/N) ──
    let sd1 = (1.0 / params.n1).sqrt();
    let sd2 = (1.0 / params.n2).sqrt();
    let z1: Vec<f64> = b1
        .iter()
        .map(|&b| b + rng.sample(Normal::new(0.0, sd1).unwrap()))
        .collect();
    let z2: Vec<f64> = b2
        .iter()
        .map(|&b| b + rng.sample(Normal::new(0.0, sd2).unwrap()))
        .collect();

    (z1, z2)
}

/// Generate a sparse component: `n_causal = floor(m * p)` entries drawn from
/// `N(0,1)`, the rest zero, randomly shuffled, then normalised to sample
/// variance = 1 (matching R `var()`, n-1 divisor).
fn make_sparse_component<R: Rng>(
    m: usize,
    p: f64,
    rng: &mut R,
    normal: &Normal<f64>,
) -> Vec<f64> {
    let n_causal = ((m as f64) * p).floor() as usize;
    let n_causal = n_causal.min(m);
    let n_zero = m - n_causal;

    let mut v: Vec<f64> = (0..n_causal).map(|_| rng.sample(normal)).collect();
    v.extend(std::iter::repeat_n(0.0, n_zero));

    // Fisher-Yates shuffle (R's sample() permutes)
    rand::seq::SliceRandom::shuffle(v.as_mut_slice(), rng);

    // Normalise to var = 1 (R: pi / sqrt(var(pi)), var uses n-1)
    let mu = v.iter().sum::<f64>() / m as f64;
    let var = v.iter().map(|&x| (x - mu).powi(2)).sum::<f64>() / (m as f64 - 1.0);
    let scale = var.sqrt();
    v.iter_mut().for_each(|x| *x /= scale);

    v
}

/// Normalise `v` to unit L2-norm, then scale by `target` (R:
/// `b / sqrt(t(b)%*%b) * target`).
fn normalise_norm(v: &mut [f64], target: f64) {
    let norm: f64 = v.iter().map(|&x| x * x).sum::<f64>().sqrt();
    let scale = target / norm;
    v.iter_mut().for_each(|x| *x *= scale);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulate_runs() {
        let params = SimParams {
            m: 5000,
            n1: 20000.0,
            n2: 50000.0,
            h2_1: 0.3,
            h2_2: 0.3,
            q1: 1.0,
            q2: 0.2,
            p_pi: 0.05,
            p_g1: 0.05,
            p_g2: 0.2,
        };
        let (z1, z2) = simulate_lcv(&params, 42);
        assert_eq!(z1.len(), 5000);
        assert_eq!(z2.len(), 5000);
        // Noise should give finite values
        assert!(z1.iter().all(|&z| z.is_finite()));
        assert!(z2.iter().all(|&z| z.is_finite()));
    }
}
