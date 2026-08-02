//! Latent Class Analysis (LCA).
//!
//! Identifies latent subgroups (classes) from a set of binary indicators via
//! a finite mixture of independent Bernoullis. Estimated via the EM algorithm.
//!
//! # Model
//!
//! For `K` latent classes and `J` binary indicators, subject `i`'s response
//! pattern `yᵢ = (yᵢ₁, …, yᵢⱼ)`:
//!
//! ```text
//! P(yᵢⱼ = 1 | class = k) = πⱼₖ
//! P(class = k) = ηₖ
//! P(yᵢ | class = k) = ∏ⱼ πⱼₖ^{yᵢⱼ} (1 − πⱼₖ)^{1−yᵢⱼ}
//! ```
//!
//! BIC and AIC are reported for model selection (choosing K).

use crate::error::{EpiError, Result};

/// Options for LCA fitting.
#[derive(Debug, Clone)]
pub struct LcaOptions {
    /// Number of latent classes K (default 2).
    pub n_classes: usize,
    /// Maximum EM iterations (default 1000).
    pub max_iter: usize,
    /// Convergence tolerance on log-likelihood (default 1e-7).
    pub tol: f64,
    /// Random seed for initialisation.
    pub seed: u64,
}

impl Default for LcaOptions {
    fn default() -> Self {
        Self {
            n_classes: 2,
            max_iter: 1000,
            tol: 1e-7,
            seed: 42,
        }
    }
}

/// Result of a Latent Class Analysis.
#[derive(Debug, Clone)]
pub struct LcaResult {
    /// Class prevalences `ηₖ` (mixing proportions), length K.
    pub class_prevalence: Vec<f64>,
    /// Item-response probabilities `πⱼₖ`: `item_probabilities[j][k]` =
    /// P(indicator j = 1 | class k). Shape J×K.
    pub item_probabilities: Vec<Vec<f64>>,
    /// Posterior class membership: `posterior[i][k]` = P(class=k | y_i).
    pub posterior: Vec<Vec<f64>>,
    /// Assigned class for each subject (argmax posterior).
    pub class_assignment: Vec<usize>,
    /// Log-likelihood at convergence.
    pub log_likelihood: f64,
    /// BIC = −2·LL + ln(N)·n_params.
    pub bic: f64,
    /// AIC = −2·LL + 2·n_params.
    pub aic: f64,
    /// Number of free parameters: `(K−1) + J·K`.
    pub n_params: usize,
    /// Number of subjects.
    pub n_obs: usize,
    /// Number of indicators.
    pub n_indicators: usize,
    /// Number of classes.
    pub n_classes: usize,
    /// Whether EM converged.
    pub converged: bool,
    /// EM iterations used.
    pub n_iter: usize,
}

/// Fit a Latent Class Analysis model.
///
/// `indicators` is organised as `indicators[j]` = binary responses (0/1) for
/// indicator `j` across all `N` subjects. Each inner Vec must be length `N`.
pub fn lca(indicators: &[Vec<u64>], opts: &LcaOptions) -> Result<LcaResult> {
    let j = indicators.len();
    if j == 0 {
        return Err(EpiError::EmptyInput);
    }
    let n = indicators[0].len();
    if n == 0 {
        return Err(EpiError::EmptyInput);
    }
    for ind in indicators.iter() {
        if ind.len() != n {
            return Err(EpiError::DimensionMismatch { a: n, b: ind.len() });
        }
        for &v in ind {
            if v != 0 && v != 1 {
                return Err(EpiError::Numerical(format!(
                    "indicator values must be 0 or 1, got {v}"
                )));
            }
        }
    }

    let k = opts.n_classes;
    let max_iter = opts.max_iter;
    let tol = opts.tol;

    // ── Initialise πⱼₖ with well-separated starting values ──────────────
    // Use spread-out initial probabilities to avoid EM collapsing to a
    // trivial solution. Class c gets a base probability of
    // 0.1 + 0.8 * c/(K−1), perturbed by a small random jitter.
    let mut seed = opts.seed;
    let mut eta = vec![1.0 / k as f64; k];
    let mut pi: Vec<Vec<f64>> = (0..j)
        .map(|_| {
            (0..k)
                .map(|c| {
                    seed = seed
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    let base = if k > 1 {
                        0.1 + 0.8 * c as f64 / (k - 1) as f64
                    } else {
                        0.5
                    };
                    let jitter = 0.1 * ((seed >> 33) as f64 / u64::MAX as f64 - 0.5);
                    (base + jitter).max(0.05).min(0.95)
                })
                .collect()
        })
        .collect();

    let mut posterior: Vec<Vec<f64>> = vec![vec![0.0; k]; n];
    let mut prev_ll = f64::NEG_INFINITY;
    let mut converged = false;
    let mut n_iter = 0;

    for iter in 0..max_iter {
        n_iter = iter + 1;

        // ── E-step: compute posterior P(class=k | y_i) ──────────────────
        let mut log_lik = 0.0_f64;
        for i in 0..n {
            let mut log_probs = vec![0.0_f64; k];
            for c in 0..k {
                let mut lp = eta[c].ln();
                for indicator in 0..j {
                    let p = pi[indicator][c].max(1e-10).min(1.0 - 1e-10);
                    lp += if indicators[indicator][i] == 1 {
                        p.ln()
                    } else {
                        (1.0 - p).ln()
                    };
                }
                log_probs[c] = lp;
            }
            let max_lp = log_probs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let sum_exp: f64 = log_probs.iter().map(|lp| (lp - max_lp).exp()).sum();
            let log_marginal = max_lp + sum_exp.ln();
            log_lik += log_marginal;
            for c in 0..k {
                posterior[i][c] = (log_probs[c] - log_marginal).exp();
            }
        }

        // ── M-step: update ηₖ and πⱼₖ ──────────────────────────────────
        for c in 0..k {
            let sum_post: f64 = posterior.iter().map(|p| p[c]).sum();
            eta[c] = sum_post / n as f64;
            for indicator in 0..j {
                let numer: f64 = posterior
                    .iter()
                    .enumerate()
                    .map(|(i, p)| p[c] * indicators[indicator][i] as f64)
                    .sum();
                pi[indicator][c] = if sum_post > 1e-10 {
                    (numer / sum_post).max(1e-10).min(1.0 - 1e-10)
                } else {
                    0.5
                };
            }
        }

        // Convergence check.
        if (log_lik - prev_ll).abs() < tol * prev_ll.abs().max(1.0) {
            converged = true;
            prev_ll = log_lik;
            break;
        }
        prev_ll = log_lik;
    }

    // ── Class assignments ────────────────────────────────────────────────
    let class_assignment: Vec<usize> = (0..n)
        .map(|i| {
            posterior[i]
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(c, _)| c)
                .unwrap_or(0)
        })
        .collect();

    // ── Model statistics ─────────────────────────────────────────────────
    let n_params = (k - 1) + j * k;
    let bic = -2.0 * prev_ll + (n as f64).ln() * n_params as f64;
    let aic = -2.0 * prev_ll + 2.0 * n_params as f64;

    Ok(LcaResult {
        class_prevalence: eta,
        item_probabilities: pi,
        posterior,
        class_assignment,
        log_likelihood: prev_ll,
        bic,
        aic,
        n_params,
        n_obs: n,
        n_indicators: j,
        n_classes: k,
        converged,
        n_iter,
    })
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * b.abs().max(1.0)
    }

    fn make_two_class_data(n: usize, seed: u64) -> Vec<Vec<u64>> {
        use rand::Rng;
        use rand::SeedableRng;
        use rand_chacha::ChaCha8Rng;

        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let n_indicators = 5;
        let mut data: Vec<Vec<u64>> = vec![vec![0; n]; n_indicators];

        for i in 0..n {
            let is_class0 = i < n / 2;
            for j in 0..n_indicators {
                let p = if is_class0 { 0.85 } else { 0.15 };
                data[j][i] = if rng.random::<f64>() < p { 1 } else { 0 };
            }
        }
        data
    }

    #[test]
    fn lca_finds_two_classes() {
        let data = make_two_class_data(200, 42);
        let opts = LcaOptions {
            n_classes: 2,
            seed: 42,
            ..Default::default()
        };
        let result = lca(&data, &opts).unwrap();

        assert_eq!(result.n_classes, 2);
        assert!(result.converged, "EM should converge");
        assert_eq!(result.n_obs, 200);
        assert_eq!(result.n_indicators, 5);

        // Roughly 50/50 split.
        for c in 0..2 {
            let count = result.class_assignment.iter().filter(|&&g| g == c).count();
            assert!(
                count > 60 && count < 140,
                "Class {c} has {count} members, expected ~100"
            );
        }
    }

    #[test]
    fn lca_recovers_item_probabilities() {
        // Class 0: P(indicator=1) ≈ 0.85, Class 1: P(indicator=1) ≈ 0.15.
        let data = make_two_class_data(500, 42);
        let result = lca(
            &data,
            &LcaOptions {
                n_classes: 2,
                seed: 42,
                ..Default::default()
            },
        )
        .unwrap();

        // One class should have high π, the other low π.
        let j = 0; // check first indicator
        let (high, low) = if result.item_probabilities[j][0] > result.item_probabilities[j][1] {
            (
                result.item_probabilities[j][0],
                result.item_probabilities[j][1],
            )
        } else {
            (
                result.item_probabilities[j][1],
                result.item_probabilities[j][0],
            )
        };
        assert!(high > 0.7, "High-π class should have π > 0.7, got {high}");
        assert!(low < 0.3, "Low-π class should have π < 0.3, got {low}");
    }

    #[test]
    fn lca_prevalences_sum_to_one() {
        let data = make_two_class_data(100, 42);
        let result = lca(&data, &LcaOptions::default()).unwrap();
        let sum: f64 = result.class_prevalence.iter().sum();
        assert!(approx_eq(sum, 1.0, 1e-6));
    }

    #[test]
    fn lca_bic_aic_finite() {
        let data = make_two_class_data(100, 42);
        let result = lca(&data, &LcaOptions::default()).unwrap();
        assert!(result.bic.is_finite());
        assert!(result.aic.is_finite());
        // BIC penalty > AIC penalty for N > 7 (ln(N) > 2).
        assert!(result.bic >= result.aic);
    }

    #[test]
    fn lca_rejects_non_binary() {
        let bad = vec![vec![0, 1, 2]];
        assert!(lca(&bad, &LcaOptions::default()).is_err());
    }
}
