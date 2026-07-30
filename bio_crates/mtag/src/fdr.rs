//! Max FDR (false discovery rate) calculation.
//!
//! Port of the maxFDR routines in `mtag.py` (Supplementary Note §1.1.4 of
//! Turley et al. 2018):
//!
//! - [`simplex_walk`] — enumerate lattice points on a unit simplex (grid
//!   of causal-state prior probabilities).
//! - [`scale_omega`] — rescale the genetic correlation matrix by the
//!   inverse probability of being causal for both traits.
//! - [`mtag_var_z_jt_c`] — conditional variance of the MTAG Z-statistic
//!   under a given causal state.
//! - [`compute_fdr`] — compute the FDR for a given prior-probability
//!   vector and target trait.
//! - [`fdr`] — top-level grid search over all valid prior vectors.
//! - [`ss_estimation`] — spike-slab fitting to restrict the grid.

use faer::Mat;
use statrs::distribution::{Normal, ContinuousCDF};

use crate::linalg::{invert, is_pos_semidef};

/// Standard normal distribution (μ=0, σ=1).
fn standard_normal() -> Normal {
    Normal::new(0.0, 1.0).unwrap()
}

/// Survival function of a general normal: P(X > z) where X ~ N(loc, scale²).
/// Matches scipy's `norm.sf(z, loc, scale)`.
fn norm_sf(z: f64, loc: f64, scale: f64) -> f64 {
    let n = standard_normal();
    n.sf((z - loc) / scale)
}

/// Generate all 2^P causal states as boolean matrices (each row is a state).
///
/// Port of `create_S` in `mtag.py`:
/// `np.asarray(list(itertools.product([False, True], repeat=P)))`
pub fn create_s(p: usize) -> Vec<Vec<bool>> {
    let n_states = 1usize << p;
    let mut states = Vec::with_capacity(n_states);
    for mask in 0..n_states {
        let state: Vec<bool> = (0..p).map(|i| (mask >> i) & 1 == 1).collect();
        states.push(state);
    }
    states
}

/// Generator of lattice points on an n-simplex.
///
/// Port of `simplex_walk` in `mtag.py`. Yields probability vectors of
/// length `num_dims + 1` (one weight per causal state minus 1, since the
/// all-null state has a derived probability).
///
/// - `num_dims` — dimensionality of the simplex (= `2^P - 1`).
/// - `samples_per_dim` — grid resolution (`intervals + 1`).
pub fn simplex_walk(num_dims: usize, samples_per_dim: usize) -> Vec<Vec<f64>> {
    let max_ = samples_per_dim + num_dims - 1;
    let combos = combinations(max_, num_dims);

    let mut results = Vec::new();
    for c in combos {
        let mut c_shift = vec![0usize; c.len() + 1];
        c_shift[0] = 0; // shift by -1 (conceptual -1 in Python)
        for i in 0..c.len() {
            c_shift[i + 1] = c[i];
        }
        // The Python code does:
        // [(y - x - 1) / (samples_per_dim - 1) for x, y in zip([-1] + c, c + [max_])]
        // where zip([-1] + c, c + [max_]) produces pairs:
        // (-1, c[0]), (c[0], c[1]), ..., (c[-1], max_)
        let prev = 0usize; // -1 + 1 = 0 in unsigned terms (Python: x starts from -1)
        let mut point = Vec::with_capacity(num_dims + 1);

        // Python: x runs through [-1] + c, y runs through c + [max_]
        // pairs: (-1, c[0]), (c[0], c[1]), ..., (c[n-1], max_)
        // value = (y - x - 1) / (samples_per_dim - 1)

        let mut x_vals: Vec<i64> = vec![-1];
        for &ci in &c {
            x_vals.push(ci as i64);
        }
        let mut y_vals: Vec<i64> = c.iter().map(|&v| v as i64).collect();
        y_vals.push(max_ as i64);

        for i in 0..x_vals.len() {
            let val = (y_vals[i] - x_vals[i] - 1) as f64 / (samples_per_dim as f64 - 1.0);
            point.push(val);
        }
        let _ = prev;
        results.push(point);
    }
    results
}

/// Generate all combinations of choosing `k` items from `n` (like
/// `itertools.combinations(range(n), k)`).
fn combinations(n: usize, k: usize) -> Vec<Vec<usize>> {
    fn recurse(start: usize, n: usize, k: usize, current: &mut Vec<usize>, result: &mut Vec<Vec<usize>>) {
        if current.len() == k {
            result.push(current.clone());
            return;
        }
        for i in start..n {
            current.push(i);
            recurse(i + 1, n, k, current, result);
            current.pop();
        }
    }
    let mut result = Vec::new();
    recurse(0, n, k, &mut Vec::new(), &mut result);
    result
}

/// Scale the genetic correlation matrix by the inverse probability of
/// being causal for both traits in a pair.
///
/// Port of `scale_omega` in `mtag.py`.
pub fn scale_omega(gen_corr_mat: &Mat<f64>, priors: &[f64], s: &[Vec<bool>]) -> Mat<f64> {
    let t = gen_corr_mat.nrows();
    let n_s = s.len();
    let mut omega = Mat::zeros(t, t);

    for p1 in 0..t {
        for p2 in 0..t {
            // Sum priors over states causal for both p1 and p2.
            let caus_sum: f64 = (0..n_s)
                .filter(|&k| s[k][p1] && s[k][p2])
                .map(|k| priors[k])
                .sum();
            omega[(p1, p2)] = if caus_sum != 0.0 {
                gen_corr_mat[(p1, p2)] / caus_sum
            } else {
                0.0
            };
        }
    }
    omega
}

/// Conditional variance of the MTAG Z-statistic for trait *t* given causal
/// state *c*.
///
/// Port of `MTAG_var_Z_jt_c` in `mtag.py`.
pub fn mtag_var_z_jt_c(
    t: usize,
    omega: &Mat<f64>,
    omega_c: &Mat<f64>,
    sigma_ld: &Mat<f64>,
    ns: &Mat<f64>,
) -> Vec<f64> {
    let m = ns.nrows();
    let tt = ns.ncols();

    let mut result = vec![0.0f64; m];

    for idx in 0..m {
        // Sigma_j for this SNP: diag(1/sqrt(N)) · sigma_ld · diag(1/sqrt(N))
        let mut sigma_j = Mat::zeros(tt, tt);
        for i in 0..tt {
            for j in 0..tt {
                sigma_j[(i, j)] = sigma_ld[(i, j)] / (ns[(idx, i)] * ns[(idx, j)]).sqrt();
            }
        }

        let gamma_k: Vec<f64> = (0..tt).map(|i| omega[(i, t)]).collect();
        let tau_k2 = omega[(t, t)];

        // om_min_gam = omega - outer(gamma_k, gamma_k) / tau_k2
        let mut om_min_gam = Mat::zeros(tt, tt);
        for i in 0..tt {
            for j in 0..tt {
                om_min_gam[(i, j)] = omega[(i, j)] - gamma_k[i] * gamma_k[j] / tau_k2;
            }
        }

        // xx = om_min_gam + sigma_j
        let mut xx = Mat::zeros(tt, tt);
        for i in 0..tt {
            for j in 0..tt {
                xx[(i, j)] = om_min_gam[(i, j)] + sigma_j[(i, j)];
            }
        }

        let inv_xx = invert(&xx);

        // num_L = (gamma_k/tau_k2)^T · inv_xx  → P-vector
        let g_over_tau: Vec<f64> = gamma_k.iter().map(|&v| v / tau_k2).collect();
        let mut num_l = vec![0.0f64; tt];
        for i in 0..tt {
            for j in 0..tt {
                num_l[i] += g_over_tau[j] * inv_xx[(j, i)];
            }
        }

        // num_R = inv_xx · (gamma_k/tau_k2)  → P-vector (per SNP row)
        let mut num_r = vec![0.0f64; tt];
        for i in 0..tt {
            for j in 0..tt {
                num_r[i] += inv_xx[(i, j)] * g_over_tau[j];
            }
        }

        // numer = num_L^T · (omega_c + sigma_j) · num_R
        let mut os = Mat::zeros(tt, tt);
        for i in 0..tt {
            for j in 0..tt {
                os[(i, j)] = omega_c[(i, j)] + sigma_j[(i, j)];
            }
        }
        let mut os_num_r = vec![0.0f64; tt];
        for i in 0..tt {
            for j in 0..tt {
                os_num_r[i] += os[(i, j)] * num_r[j];
            }
        }
        let numer: f64 = num_l.iter().zip(&os_num_r).map(|(a, b)| a * b).sum();

        // denom = (gamma_k/tau_k2)^T · inv_xx · (gamma_k/tau_k2)
        let mut inv_xx_g = vec![0.0f64; tt];
        for i in 0..tt {
            for j in 0..tt {
                inv_xx_g[i] += inv_xx[(i, j)] * g_over_tau[j];
            }
        }
        let denom: f64 = g_over_tau.iter().zip(&inv_xx_g).map(|(a, b)| a * b).sum();

        if denom.abs() < 1e-300 {
            result[idx] = f64::INFINITY;
        } else {
            result[idx] = numer / denom;
        }
    }

    result
}

/// Compute the FDR for a given prior probability vector and target trait.
///
/// Port of `compute_fdr` in `mtag.py`.
#[allow(clippy::too_many_arguments)]
pub fn compute_fdr(
    prob: &[f64],
    t: usize,
    omega: &Mat<f64>,
    sigma: &Mat<f64>,
    s: &[Vec<bool>],
    n_vals: &Mat<f64>,
    n_counts: &[f64],
    p_threshold: f64,
) -> f64 {
    let n = standard_normal();

    // z_threshold = norm.isf(p_threshold / 2) = norm.ppf(1 - p/2)
    let z_threshold = n.inverse_cdf(1.0 - p_threshold / 2.0);

    let n_s = s.len();

    let omega_tt = scale_omega(omega, prob, s);

    if !is_pos_semidef(&omega_tt) {
        return f64::INFINITY;
    }

    // Omega_s[k] = outer(s[k], s[k]) * omega_tt
    // i.e. Omega_s[k,i,j] = s[k,i] * s[k,j] * omega_tt[i,j]
    let mut prob_signif_cond_t = vec![0.0f64; n_s];
    let mut power_state_t = vec![0.0f64; n_s];

    let total_counts: f64 = n_counts.iter().sum();

    for k in 0..n_s {
        // Build Omega_s[k] = outer(s[k], s[k]) * omega_tt
        let tt = omega_tt.nrows();
        let mut omega_s_k = Mat::zeros(tt, tt);
        for i in 0..tt {
            for j in 0..tt {
                let s_ij = if s[k][i] && s[k][j] { 1.0 } else { 0.0 };
                omega_s_k[(i, j)] = s_ij * omega_tt[(i, j)];
            }
        }

        let sd = mtag_var_z_jt_c(t, omega, &omega_s_k, sigma, n_vals);

        // Prob_signif_cond_t[k] = weighted average of 2*sf(z_threshold, 0, sd_m) over SNPs
        let mut weighted_sum = 0.0;
        for idx in 0..sd.len() {
            let prob_signif = 2.0 * norm_sf(z_threshold, 0.0, sd[idx].sqrt());
            weighted_sum += prob_signif * n_counts[idx];
        }
        prob_signif_cond_t[k] = weighted_sum / total_counts;

        power_state_t[k] = prob_signif_cond_t[k] * prob[k];
    }

    // FDR = sum(power_state_t[~s[:,t]]) / sum(power_state_t)
    let numer: f64 = (0..n_s)
        .filter(|&k| !s[k][t])
        .map(|k| power_state_t[k])
        .sum();
    let denom: f64 = power_state_t.iter().sum();

    if denom.abs() < 1e-300 {
        f64::INFINITY
    } else {
        numer / denom
    }
}

/// Check that every trait has non-zero probability of being causal, and
/// that every pair of traits has non-zero probability of being jointly causal.
///
/// Port of `some_causal_for_allT` in `mtag.py`.
pub fn some_causal_for_all_t(probs: &[f64], s: &[Vec<bool>]) -> bool {
    let n_s = s.len();
    let t = if n_s == 0 { return false; } else { s[0].len() };

    // Each trait must have non-zero causal probability.
    for tt in 0..t {
        let sum: f64 = (0..n_s).filter(|&k| s[k][tt]).map(|k| probs[k]).sum();
        if sum <= 0.0 {
            return false;
        }
    }

    // Each pair must have non-zero joint causal probability.
    for p1 in 0..t {
        for p2 in 0..t {
            let sum: f64 = (0..n_s)
                .filter(|&k| s[k][p1] && s[k][p2])
                .map(|k| probs[k])
                .sum();
            if sum == 0.0 {
                return false;
            }
        }
    }
    true
}

/// Configuration for the max-FDR grid search.
#[derive(Clone, Debug)]
pub struct FdrConfig {
    /// Number of grid intervals (default 10).
    pub intervals: usize,
    /// Significance p-value threshold (default 5e-8).
    pub p_sig: f64,
    /// Whether to approximate N by mean (speed up).
    pub n_approx: bool,
    /// Whether to fit spike-slab to restrict grid.
    pub fit_ss: bool,
}

impl Default for FdrConfig {
    fn default() -> Self {
        Self {
            intervals: 10,
            p_sig: 5e-8,
            n_approx: true,
            fit_ss: false,
        }
    }
}

/// Run the full max-FDR grid search.
///
/// Port of `fdr` in `mtag.py`. Returns a matrix of FDR values
/// (n_gridpoints × T) and the corresponding grid points.
pub fn run_fdr(
    omega_hat: &Mat<f64>,
    sigma_hat: &Mat<f64>,
    ns: &Mat<f64>,
    _zs: &Mat<f64>,
    cfg: &FdrConfig,
) -> (Mat<f64>, Vec<Vec<f64>>) {
    use rayon::prelude::*;

    let m = ns.nrows();
    let t = ns.ncols();
    let s = create_s(t);

    // Generate probability grid via simplex walk.
    let prob_grid_raw = simplex_walk(s.len() - 1, cfg.intervals + 1);

    // Filter grid: each point must have some causal for all traits and
    // yield a PSD scaled omega.
    let prob_grid: Vec<Vec<f64>> = prob_grid_raw
        .iter()
        .filter(|p| {
            some_causal_for_all_t(p, &s) && is_pos_semidef(&scale_omega(omega_hat, p, &s))
        })
        .cloned()
        .collect();

    let n_grid = prob_grid.len();

    // Prepare N values and weights.
    let (n_vals, n_weights): (Mat<f64>, Vec<f64>) = if cfg.n_approx {
        // Use mean N per trait.
        let mut means = Mat::zeros(1, t);
        for j in 0..t {
            let col_mean: f64 = (0..m).map(|i| ns[(i, j)]).sum::<f64>() / m as f64;
            means[(0, j)] = col_mean;
        }
        (means, vec![1.0])
    } else {
        // Use unique N rows with counts.
        // For simplicity in the common case, use all rows.
        // A full unique-row optimization can be added later.
        (ns.clone(), vec![1.0; m])
    };

    let _total_counts: f64 = n_weights.iter().sum();

    // FDR matrix: n_grid × t
    let mut fdr_mat = Mat::zeros(n_grid, t);

    // For each (grid_point, trait) compute FDR in parallel.
    let tasks: Vec<(usize, usize)> = (0..t)
        .flat_map(|tt| (0..n_grid).map(move |g| (g, tt)))
        .collect();
    let results: Vec<(usize, usize, f64)> = tasks
        .par_iter()
        .map(|&(g, tt)| {
            let fdr_val = compute_fdr(
                &prob_grid[g],
                tt,
                omega_hat,
                sigma_hat,
                &s,
                &n_vals,
                &n_weights,
                cfg.p_sig,
            );
            (g, tt, fdr_val)
        })
        .collect();

    for (g, tt, fdr_val) in results {
        fdr_mat[(g, tt)] = fdr_val;
    }

    (fdr_mat, prob_grid)
}

// --- Spike-slab estimation (for grid restriction) ---

/// Negative log-likelihood of betas under a spike-slab distribution.
///
/// Port of `neglogL_single_SS` in `mtag.py`.
fn neglog_l_single_ss(x: &[f64], beta: &[f64], se: &[f64], transformed: bool) -> f64 {
    let (prob_null, tau) = if transformed {
        let pn = 1.0 / (1.0 + (-x[0]).exp());
        let t = (-x[1]).exp();
        (pn, t)
    } else {
        (x[0], x[1])
    };

    let mut neg_l = 0.0;
    for i in 0..beta.len() {
        let causal_scale = (tau * tau + se[i] * se[i]).sqrt();
        let causal_pdf = normal_pdf(beta[i], 0.0, causal_scale);
        let noncausal_pdf = normal_pdf(beta[i], 0.0, se[i]);
        let mixture = (1.0 - prob_null) * causal_pdf + prob_null * noncausal_pdf;
        if mixture > 0.0 {
            neg_l -= mixture.ln();
        }
    }
    neg_l
}

/// Normal PDF at `x` with mean `mu` and std `sigma`.
fn normal_pdf(x: f64, mu: f64, sigma: f64) -> f64 {
    let z = (x - mu) / sigma;
    (-0.5 * z * z - 0.5 * (2.0 * std::f64::consts::PI).ln() - sigma.ln()).exp()
}

/// Estimate spike-slab parameters (pi_null, tau) for each trait via Nelder-Mead.
///
/// Port of `ss_estimation` + `_optim_ss` in `mtag.py`.
pub fn ss_estimation(
    betas: &Mat<f64>,
    se: &Mat<f64>,
    max_iter: usize,
    tol: f64,
) -> Vec<(f64, f64)> {
    // (pi_null, tau) per trait
    let m = betas.nrows();
    let t = betas.ncols();

    (0..t)
        .map(|tt| {
            let beta_t: Vec<f64> = (0..m).map(|i| betas[(i, tt)]).collect();
            let se_t: Vec<f64> = (0..m).map(|i| se[(i, tt)]).collect();

            // Starting params: (0.5, 1e-3) → transformed
            let start_pi = 0.5f64;
            let start_tau = 1e-3f64;
            let x_0 = vec![
                (start_pi / (1.0 - start_pi)).ln(), // inverse logit
                -start_tau.ln(),
            ];

            let opt_x = crate::nelder::nelder_mead_generic(
                |x| neglog_l_single_ss(x, &beta_t, &se_t, true),
                &x_0,
                0.5,
                tol,
                tol,
                max_iter,
            );

            let pi_null = 1.0 / (1.0 + (-opt_x[0]).exp());
            let tau = (-opt_x[1]).exp();
            (pi_null, tau)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_s() {
        let s = create_s(2);
        assert_eq!(s.len(), 4);
        // product([F,T], repeat=2) in Python order:
        // (F,F), (T,F), (F,T), (T,T) — due to bit ordering
        assert_eq!(s[0], vec![false, false]);
        assert_eq!(s[1], vec![true, false]);
        assert_eq!(s[2], vec![false, true]);
        assert_eq!(s[3], vec![true, true]);
    }

    #[test]
    fn test_simplex_walk_1d() {
        // 1-simplex with samples_per_dim=3 → points 0, 0.5, 1.0
        let points = simplex_walk(1, 3);
        assert_eq!(points.len(), 3); // C(3,1) = 3
        assert!((points[0][0] - 0.0).abs() < 1e-12);
        assert!((points[1][0] - 0.5).abs() < 1e-12);
        assert!((points[2][0] - 1.0).abs() < 1e-12);
    }
}
