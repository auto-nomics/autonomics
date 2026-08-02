//! Group-Based Trajectory Modeling (GBTM).
//!
//! Identifies latent subgroups following distinct developmental trajectories
//! via a finite mixture of polynomials (Nagin 1999/2005). The model is fit via
//! the EM algorithm:
//!
//! # Model
//!
//! For each latent group `k = 1…K` and observation `i` at time `t_j`:
//!
//! ```text
//! yᵢⱼ | group=k  ~  N(β₀⁽ᵏ⁾ + β₁⁽ᵏ⁾·tⱼ + β₂⁽ᵏ⁾·tⱼ² + …, σₖ²)
//! ```
//!
//! The mixing proportions `πₖ` (group sizes) and regression coefficients
//! `β⁽ᵏ⁾` are estimated via EM. Group membership (posterior probability) is
//! assigned by Bayes' rule.
//!
//! # Model selection
//!
//! BIC is reported for choosing the number of groups and polynomial order.

use statkit::regression;

use crate::error::{EpiError, Result};

/// Options for GBTM fitting.
#[derive(Debug, Clone)]
pub struct GbtmOptions {
    /// Number of latent groups K (default 3).
    pub n_groups: usize,
    /// Polynomial degree for each trajectory (0=intercept, 1=linear, 2=quadratic).
    /// Default: 2 (quadratic).
    pub poly_degree: usize,
    /// Maximum EM iterations (default 500).
    pub max_iter: usize,
    /// EM convergence tolerance on log-likelihood (default 1e-6).
    pub tol: f64,
    /// Random seed for initialisation.
    pub seed: u64,
}

impl Default for GbtmOptions {
    fn default() -> Self {
        Self {
            n_groups: 3,
            poly_degree: 2,
            max_iter: 500,
            tol: 1e-6,
            seed: 42,
        }
    }
}

/// Result of a GBTM fit.
#[derive(Debug, Clone)]
pub struct GbtmResult {
    /// Mixing proportions (group sizes), length K.
    pub pi: Vec<f64>,
    /// Group-specific polynomial coefficients: `betas[k]` is a Vec of length
    /// `poly_degree + 1` (intercept, t, t², …).
    pub betas: Vec<Vec<f64>>,
    /// Group-specific residual standard deviations.
    pub sigmas: Vec<f64>,
    /// Posterior group membership probabilities: `posterior[i][k]` = P(group=k | data_i).
    pub posterior: Vec<Vec<f64>>,
    /// Assigned group for each observation (argmax of posterior).
    pub group_assignment: Vec<usize>,
    /// Fitted log-likelihood at convergence.
    pub log_likelihood: f64,
    /// BIC (lower is better): −2·LL + ln(N)·n_params.
    pub bic: f64,
    /// Number of free parameters.
    pub n_params: usize,
    /// Number of observations.
    pub n_obs: usize,
    /// Number of time points.
    pub n_time: usize,
    /// Number of groups.
    pub n_groups: usize,
    /// EM iterations used.
    pub n_iter: usize,
    /// Whether EM converged.
    pub converged: bool,
}

/// Fit a Group-Based Trajectory Model.
///
/// `data` is organised as `data[i]` = `Vec<(time, outcome)>` for subject `i`.
/// Each subject can have a different number of observations at different times.
/// All subjects' times are pooled to form the design matrix for the polynomial
/// basis.
pub fn gbtm(data: &[Vec<(f64, f64)>], opts: &GbtmOptions) -> Result<GbtmResult> {
    let n = data.len();
    if n == 0 {
        return Err(EpiError::EmptyInput);
    }
    let k = opts.n_groups;
    let deg = opts.poly_degree;
    let max_iter = opts.max_iter;
    let tol = opts.tol;

    // Flatten all observations: (subject_idx, time, outcome).
    let mut flat_time: Vec<f64> = Vec::new();
    let mut flat_y: Vec<f64> = Vec::new();
    let mut subject_idx: Vec<usize> = Vec::new();
    for (i, obs) in data.iter().enumerate() {
        for &(t, y) in obs {
            flat_time.push(t);
            flat_y.push(y);
            subject_idx.push(i);
        }
    }
    let m = flat_time.len(); // total observations

    // ── Initialise group assignments via random partitioning ────────────
    let mut rng_seed = opts.seed;
    let mut group_init: Vec<usize> = (0..n)
        .map(|_| {
            // Deterministic pseudo-random assignment based on seed.
            rng_seed = rng_seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (rng_seed >> 33) as usize % k
        })
        .collect();
    // Ensure each group has at least one member.
    for g in 0..k {
        if !group_init.contains(&g) {
            group_init[g % n] = g;
        }
    }

    // Mixing proportions.
    let mut pi = vec![0.0_f64; k];
    for &g in &group_init {
        pi[g] += 1.0;
    }
    for p in pi.iter_mut() {
        *p /= n as f64;
    }

    // ── EM iterations ────────────────────────────────────────────────────
    let mut betas: Vec<Vec<f64>> = vec![vec![0.0; deg + 1]; k];
    let mut sigmas: Vec<f64> = vec![1.0; k];
    let mut posterior: Vec<Vec<f64>> = vec![vec![0.0; k]; n];
    let mut prev_ll = f64::NEG_INFINITY;
    let mut converged = false;
    let mut n_iter = 0;

    for iter in 0..max_iter {
        n_iter = iter + 1;

        // ── M-step: fit polynomial regression for each group ─────────────
        for g in 0..k {
            // Collect observations assigned to group g (using current posteriors).
            // Use weighted OLS where weight = posterior probability.
            let mut weights: Vec<f64> = Vec::new();
            let mut y_g: Vec<f64> = Vec::new();
            let mut t_g: Vec<f64> = Vec::new();

            for j in 0..m {
                let i = subject_idx[j];
                let w = if iter == 0 {
                    if group_init[i] == g { 1.0 } else { 0.0 }
                } else {
                    posterior[i][g]
                };
                if w > 1e-10 {
                    weights.push(w);
                    y_g.push(flat_y[j]);
                    t_g.push(flat_time[j]);
                }
            }

            if weights.is_empty() || weights.iter().sum::<f64>() < 1e-10 {
                betas[g] = vec![0.0; deg + 1];
                sigmas[g] = 1.0;
                continue;
            }

            // Build polynomial predictors.
            let mut preds: Vec<Vec<f64>> = Vec::with_capacity(deg + 1);
            for d in 0..=deg {
                preds.push(t_g.iter().map(|&t| t.powi(d as i32)).collect());
            }
            let pred_slices: Vec<&[f64]> = preds.iter().map(|p| p.as_slice()).collect();

            // Weighted least squares.
            if let Ok(fit) = regression::wls(&pred_slices, &y_g, &weights, false) {
                betas[g] = fit.coefficients.clone();
                // Residual standard deviation (weighted).
                let wsum: f64 = weights.iter().sum();
                let rss: f64 = (0..y_g.len())
                    .map(|i| weights[i] * fit.residuals[i] * fit.residuals[i])
                    .sum();
                sigmas[g] = (rss / wsum).max(1e-10).sqrt();
            }
        }

        // ── E-step: compute posterior probabilities ───────────────────────
        let ll = e_step(
            &flat_time,
            &flat_y,
            &subject_idx,
            n,
            &betas,
            &sigmas,
            &pi,
            k,
            deg,
            &mut posterior,
        );

        // Update mixing proportions.
        for g in 0..k {
            pi[g] = posterior.iter().map(|p| p[g]).sum::<f64>() / n as f64;
            if pi[g] < 1e-10 {
                pi[g] = 1e-10;
            }
        }

        // Convergence check.
        if (ll - prev_ll).abs() < tol * prev_ll.abs().max(1.0) {
            converged = true;
            prev_ll = ll;
            break;
        }
        prev_ll = ll;
    }

    // ── Group assignments ────────────────────────────────────────────────
    let group_assignment: Vec<usize> = (0..n)
        .map(|i| {
            posterior[i]
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(g, _)| g)
                .unwrap_or(0)
        })
        .collect();

    // ── Model statistics ─────────────────────────────────────────────────
    let n_params = k * (deg + 1) + k + (k - 1); // betas + sigmas + mixing props
    let bic = -2.0 * prev_ll + (n as f64).ln() * n_params as f64;

    // Collect unique time points.
    let n_time = {
        let mut t_sorted = flat_time.clone();
        t_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        t_sorted.dedup_by(|a, b| (*a - *b).abs() < 1e-10);
        t_sorted.len()
    };

    Ok(GbtmResult {
        pi,
        betas,
        sigmas,
        posterior,
        group_assignment,
        log_likelihood: prev_ll,
        bic,
        n_params,
        n_obs: n,
        n_time,
        n_groups: k,
        n_iter,
        converged,
    })
}

/// E-step: compute log-likelihood and update posterior probabilities.
fn e_step(
    flat_time: &[f64],
    flat_y: &[f64],
    subject_idx: &[usize],
    n: usize,
    betas: &[Vec<f64>],
    sigmas: &[f64],
    pi: &[f64],
    k: usize,
    deg: usize,
    posterior: &mut [Vec<f64>],
) -> f64 {
    // For each subject, compute log-likelihood under each group.
    let mut log_lik = 0.0_f64;

    for i in 0..n {
        let mut log_probs: Vec<f64> = vec![0.0; k];

        for g in 0..k {
            let mut group_log_prob = pi[g].ln();

            // Find all observations for this subject.
            for j in 0..flat_time.len() {
                if subject_idx[j] != i {
                    continue;
                }
                let t = flat_time[j];
                let y = flat_y[j];
                let eta: f64 = (0..=deg).map(|d| betas[g][d] * t.powi(d as i32)).sum();
                let resid = y - eta;
                let sigma = sigmas[g].max(1e-10);
                group_log_prob -= 0.5 * (resid / sigma).powi(2)
                    + sigma.ln()
                    + 0.5 * (2.0 * std::f64::consts::PI).ln();
            }

            log_probs[g] = group_log_prob;
        }

        // Log-sum-exp for numerical stability.
        let max_lp = log_probs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let sum_exp: f64 = log_probs.iter().map(|lp| (lp - max_lp).exp()).sum();
        let log_marginal = max_lp + sum_exp.ln();
        log_lik += log_marginal;

        // Normalize to posterior probabilities.
        for g in 0..k {
            posterior[i][g] = (log_probs[g] - log_marginal).exp();
        }
    }

    log_lik
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * b.abs().max(1.0)
    }

    fn make_trajectory_data() -> Vec<Vec<(f64, f64)>> {
        // 3 groups: increasing, stable, decreasing trajectories.
        let n_per_group = 50;
        let times: Vec<f64> = vec![0.0, 1.0, 2.0, 3.0, 4.0];
        let mut data = Vec::new();

        for i in 0..n_per_group {
            let noise = (i as f64 * 0.1) % 1.0 - 0.5;
            // Group 0: increasing (slope ~1.0)
            data.push(times.iter().map(|&t| (t, 1.0 * t + noise)).collect());
        }
        for i in 0..n_per_group {
            let noise = (i as f64 * 0.1) % 1.0 - 0.5;
            // Group 1: stable (slope ~0.0)
            data.push(times.iter().map(|&t| (t, 5.0 + noise)).collect());
        }
        for i in 0..n_per_group {
            let noise = (i as f64 * 0.1) % 1.0 - 0.5;
            // Group 2: decreasing (slope ~-1.0)
            data.push(times.iter().map(|&t| (t, 10.0 - t + noise)).collect());
        }

        data
    }

    #[test]
    fn gbtm_finds_three_groups() {
        let data = make_trajectory_data();
        let opts = GbtmOptions {
            n_groups: 3,
            poly_degree: 1, // linear trajectories
            max_iter: 200,
            tol: 1e-5,
            seed: 42,
            ..Default::default()
        };
        let result = gbtm(&data, &opts).unwrap();

        assert_eq!(result.n_groups, 3);
        assert!(result.converged, "EM should converge");
        assert_eq!(result.n_obs, 150);

        // Each group should contain roughly 50 subjects.
        for g in 0..3 {
            let count = result
                .group_assignment
                .iter()
                .filter(|&&grp| grp == g)
                .count();
            assert!(
                count > 20 && count < 80,
                "Group {g} has {count} members, expected ~50"
            );
        }
    }

    #[test]
    fn gbtm_recover_trajectory_directions() {
        let data = make_trajectory_data();
        let opts = GbtmOptions {
            n_groups: 3,
            poly_degree: 1,
            seed: 42,
            ..Default::default()
        };
        let result = gbtm(&data, &opts).unwrap();

        // Sort groups by slope (beta_1).
        let mut slopes: Vec<(usize, f64)> = result
            .betas
            .iter()
            .enumerate()
            .map(|(g, b)| (g, b.get(1).copied().unwrap_or(0.0)))
            .collect();
        slopes.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        // Lowest slope should be negative (decreasing group).
        assert!(
            slopes[0].1 < 0.0,
            "Lowest slope should be negative, got {}",
            slopes[0].1
        );
        // Highest slope should be positive (increasing group).
        assert!(
            slopes[2].1 > 0.0,
            "Highest slope should be positive, got {}",
            slopes[2].1
        );
    }

    #[test]
    fn gbtm_bic_finite() {
        let data = make_trajectory_data();
        let opts = GbtmOptions::default();
        let result = gbtm(&data, &opts).unwrap();
        assert!(result.bic.is_finite());
        assert!(result.log_likelihood.is_finite());
    }

    #[test]
    fn gbtm_mixing_proportions_sum_to_one() {
        let data = make_trajectory_data();
        let result = gbtm(&data, &GbtmOptions::default()).unwrap();
        let sum: f64 = result.pi.iter().sum();
        assert!(approx_eq(sum, 1.0, 1e-6));
    }
}
