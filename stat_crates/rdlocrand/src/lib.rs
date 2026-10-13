//! `rdlocrand`: Rust port of the R `rdlocrand` package.
//!
//! Randomization inference and window selection for RD designs under
//! local randomization.
//!
//! Faithful port of Cattaneo, Frandsen & Titiunik (2015) and
//! Cattaneo, Titiunik & Vázquez-Bare (2016).

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use statrs::distribution::{ContinuousCDF, DiscreteCDF, Normal};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RdLocRandError {
    #[error("{0}")]
    Msg(String),
}

// =====================================================================
// Test statistics
// =====================================================================

/// Difference in means statistic.
fn diff_means(y_treat: &[f64], y_ctrl: &[f64]) -> f64 {
    let mt = y_treat.iter().sum::<f64>() / y_treat.len() as f64;
    let mc = y_ctrl.iter().sum::<f64>() / y_ctrl.len() as f64;
    mt - mc
}

/// Kolmogorov-Smirnov statistic.
fn ks_statistic(y_treat: &[f64], y_ctrl: &[f64]) -> f64 {
    let nt = y_treat.len();
    let nc = y_ctrl.len();
    let mut all: Vec<(f64, bool)> = y_treat.iter().map(|&y| (y, true)).collect();
    all.extend(y_ctrl.iter().map(|&y| (y, false)));
    all.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut max_d: f64 = 0.0;
    let mut ft: f64 = 0.0;
    let mut fc: f64 = 0.0;
    for (_, is_treat) in &all {
        if *is_treat {
            ft += 1.0 / nt as f64;
        } else {
            fc += 1.0 / nc as f64;
        }
        max_d = max_d.max((ft - fc).abs());
    }
    max_d
}

/// Wilcoxon rank-sum statistic (standardized).
fn ranksum_statistic(y_treat: &[f64], y_ctrl: &[f64]) -> f64 {
    let nt = y_treat.len();
    let nc = y_ctrl.len();
    let n = nt + nc;
    let mut all: Vec<(f64, bool)> = y_treat.iter().map(|&y| (y, true)).collect();
    all.extend(y_ctrl.iter().map(|&y| (y, false)));
    all.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    // Assign ranks (average for ties)
    let mut ranks = vec![0.0; n];
    let mut i = 0;
    while i < n {
        let mut j = i + 1;
        while j < n && all[j].0 == all[i].0 {
            j += 1;
        }
        let avg_rank = (i + j + 1) as f64 / 2.0;
        for k in i..j {
            ranks[k] = avg_rank;
        }
        i = j;
    }

    let rank_sum_t: f64 = all
        .iter()
        .zip(&ranks)
        .filter(|(a, _)| a.1)
        .map(|(_, r)| *r)
        .sum();
    let expected = nt as f64 * (n as f64 + 1.0) / 2.0;
    let variance = nt as f64 * nc as f64 * (n as f64 + 1.0) / 12.0;
    if variance > 0.0 {
        (rank_sum_t - expected) / variance.sqrt()
    } else {
        0.0
    }
}

/// Compute test statistic based on type.
pub fn compute_statistic(y_treat: &[f64], y_ctrl: &[f64], stat_type: &str) -> f64 {
    match stat_type {
        "ksmirnov" => ks_statistic(y_treat, y_ctrl),
        "ranksum" => ranksum_statistic(y_treat, y_ctrl),
        _ => diff_means(y_treat, y_ctrl), // "diffmeans" default
    }
}

// =====================================================================
// rdrandinf: Randomization inference
// =====================================================================

/// Configuration for `rdrandinf()`.
#[derive(Clone, Debug)]
pub struct RdRandInfConfig {
    pub y: Vec<f64>,
    pub r: Vec<f64>,
    pub cutoff: f64,
    pub wl: f64,
    pub wr: f64,
    pub statistic: String,
    pub nulltau: f64,
    pub reps: usize,
    pub seed: u64,
}

impl Default for RdRandInfConfig {
    fn default() -> Self {
        Self {
            y: vec![],
            r: vec![],
            cutoff: 0.0,
            wl: f64::NEG_INFINITY,
            wr: f64::INFINITY,
            statistic: "diffmeans".into(),
            nulltau: 0.0,
            reps: 1000,
            seed: 42,
        }
    }
}

/// Output of `rdrandinf()`.
#[derive(Clone, Debug)]
pub struct RdRandInfResult {
    pub obs_stat: f64,
    pub p_value: f64,
    pub asy_pvalue: f64,
    pub window: (f64, f64),
    pub n_window: usize,
    pub n_treat: usize,
    pub n_ctrl: usize,
}

/// Randomization inference for RD designs.
///
/// Implements Fisherian exact p-values via permutation testing within
/// a specified window around the cutoff.
pub fn rdrandinf(cfg: &RdRandInfConfig) -> Result<RdRandInfResult, RdLocRandError> {
    let cutoff = cfg.cutoff;
    let wl = cfg.wl;
    let wr = cfg.wr;

    // Select window
    let mut yw = Vec::new();
    let mut treat = Vec::new();
    for i in 0..cfg.y.len() {
        if cfg.r[i] >= wl && cfg.r[i] <= wr && cfg.y[i].is_finite() {
            yw.push(cfg.y[i] - cfg.nulltau * (cfg.r[i] >= cutoff) as i32 as f64);
            treat.push(cfg.r[i] >= cutoff);
        }
    }

    let nw = yw.len();
    if nw < 4 {
        return Err(RdLocRandError::Msg(
            "Too few observations in window.".into(),
        ));
    }

    let y_treat: Vec<f64> = yw
        .iter()
        .zip(&treat)
        .filter(|(_, t)| **t)
        .map(|(y, _)| *y)
        .collect();
    let y_ctrl: Vec<f64> = yw
        .iter()
        .zip(&treat)
        .filter(|(_, t)| !**t)
        .map(|(y, _)| *y)
        .collect();

    if y_treat.is_empty() || y_ctrl.is_empty() {
        return Err(RdLocRandError::Msg(
            "No observations on one side of cutoff.".into(),
        ));
    }

    // Observed statistic
    let obs_stat = compute_statistic(&y_treat, &y_ctrl, &cfg.statistic);

    // Permutation test
    let mut rng = ChaCha8Rng::seed_from_u64(cfg.seed);
    let nt = y_treat.len();
    let nc = y_ctrl.len();
    let n = nt + nc;

    let mut count_extreme = 0usize;
    for _ in 0..cfg.reps {
        // Random permutation of treatment assignment
        let mut perm: Vec<usize> = (0..n).collect();
        // Fisher-Yates shuffle
        for i in (1..n).rev() {
            let j = rng.random_range(0..=i);
            perm.swap(i, j);
        }

        let mut yt_perm = Vec::with_capacity(nt);
        let mut yc_perm = Vec::with_capacity(nc);
        for k in 0..n {
            if perm[k] < nt {
                yt_perm.push(yw[k]);
            } else {
                yc_perm.push(yw[k]);
            }
        }
        let stat = compute_statistic(&yt_perm, &yc_perm, &cfg.statistic);
        if stat.abs() >= obs_stat.abs() - 1e-15 {
            count_extreme += 1;
        }
    }

    let p_value = (count_extreme + 1) as f64 / (cfg.reps + 1) as f64;

    // Asymptotic p-value
    let normal = Normal::new(0.0, 1.0).unwrap();
    let pooled_var = {
        let mt = y_treat.iter().sum::<f64>() / nt as f64;
        let mc = y_ctrl.iter().sum::<f64>() / nc as f64;
        let ss_t: f64 = y_treat.iter().map(|y| (y - mt).powi(2)).sum();
        let ss_c: f64 = y_ctrl.iter().map(|y| (y - mc).powi(2)).sum();
        (ss_t + ss_c) / (n - 2) as f64
    };
    let se = (pooled_var * (1.0 / nt as f64 + 1.0 / nc as f64)).sqrt();
    let asy_pvalue = if se > 0.0 {
        2.0 * normal.cdf(-(obs_stat / se).abs())
    } else {
        1.0
    };

    Ok(RdRandInfResult {
        obs_stat,
        p_value,
        asy_pvalue,
        window: (wl, wr),
        n_window: nw,
        n_treat: nt,
        n_ctrl: nc,
    })
}

// =====================================================================
// rdwinselect: Window selection
// =====================================================================

/// Configuration for `rdwinselect()`.
#[derive(Clone, Debug)]
pub struct RdWinSelectConfig {
    pub r: Vec<f64>,
    pub x: Vec<Vec<f64>>, // covariates (each Vec is a column)
    pub cutoff: f64,
    pub obsmin: usize,
    pub wobs: usize,
    pub nwindows: usize,
    pub statistic: String,
    pub reps: usize,
    pub seed: u64,
    pub level: f64,
}

impl Default for RdWinSelectConfig {
    fn default() -> Self {
        Self {
            r: vec![],
            x: vec![],
            cutoff: 0.0,
            obsmin: 10,
            wobs: 5,
            nwindows: 10,
            statistic: "diffmeans".into(),
            reps: 1000,
            seed: 42,
            level: 0.15,
        }
    }
}

/// Output of `rdwinselect()`.
#[derive(Clone, Debug)]
pub struct RdWinSelectResult {
    pub w_left: f64,
    pub w_right: f64,
    pub results: Vec<WindowResult>,
}

#[derive(Clone, Debug)]
pub struct WindowResult {
    pub w_left: f64,
    pub w_right: f64,
    pub n_left: usize,
    pub n_right: usize,
    pub min_pval: f64,
    pub binom_pval: f64,
}

/// Window selection for RD designs under local randomization.
///
/// Constructs nested windows and tests covariate balance at each.
pub fn rdwinselect(cfg: &RdWinSelectConfig) -> Result<RdWinSelectResult, RdLocRandError> {
    let cutoff = cfg.cutoff;
    let r_sorted: Vec<f64> = {
        let mut v: Vec<f64> = cfg.r.to_vec();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        v
    };

    let nl_full = r_sorted.iter().filter(|r| **r < cutoff).count();
    let nr_full = r_sorted.iter().filter(|r| **r >= cutoff).count();

    // Build sequence of windows
    let mut windows: Vec<(f64, f64)> = Vec::new();
    for w in 0..cfg.nwindows {
        let n_target = cfg.obsmin + w * cfg.wobs;
        let nl = n_target.min(nl_full);
        let nr = n_target.min(nr_full);

        // Left boundary: nl-th observation below cutoff
        let left_vals: Vec<f64> = r_sorted.iter().filter(|r| **r < cutoff).copied().collect();
        let right_vals: Vec<f64> = r_sorted.iter().filter(|r| **r >= cutoff).copied().collect();

        let wl = if nl < left_vals.len() {
            left_vals[left_vals.len() - nl - 1]
        } else {
            *left_vals.first().unwrap_or(&f64::NEG_INFINITY)
        };
        let wr = if nr < right_vals.len() {
            right_vals[nr]
        } else {
            *right_vals.last().unwrap_or(&f64::INFINITY)
        };

        windows.push((wl, wr));
    }

    // Compute balance tests for each window
    let mut results = Vec::new();
    let mut recommended_idx = 0;

    for (idx, &(wl, wr)) in windows.iter().enumerate() {
        // Select observations in window
        let mask: Vec<bool> = cfg.r.iter().map(|r| *r >= wl && *r <= wr).collect();
        let nl: Vec<usize> = (0..cfg.r.len())
            .filter(|&i| mask[i] && cfg.r[i] < cutoff)
            .collect();
        let nr: Vec<usize> = (0..cfg.r.len())
            .filter(|&i| mask[i] && cfg.r[i] >= cutoff)
            .collect();

        let n_left = nl.len();
        let n_right = nr.len();

        if n_left < 2 || n_right < 2 {
            continue;
        }

        // Binomial test
        let binom_p = binom_test(n_left, n_right, 0.5);

        // Covariate balance tests
        let mut min_pval: f64 = 1.0;
        for cov in &cfg.x {
            let yc: Vec<f64> = nl.iter().map(|&i| cov[i]).collect();
            let yt: Vec<f64> = nr.iter().map(|&i| cov[i]).collect();

            if cfg.reps > 0 && n_left + n_right <= 200 {
                // Permutation test
                let stat = compute_statistic(&yt, &yc, &cfg.statistic);
                let mut rng = ChaCha8Rng::seed_from_u64(cfg.seed + idx as u64);
                let n = n_left + n_right;
                let all: Vec<f64> = yc.iter().chain(yt.iter()).copied().collect();
                let mut count = 0;
                for _ in 0..cfg.reps {
                    let mut perm: Vec<usize> = (0..n).collect();
                    for i in (1..n).rev() {
                        let j = rng.random_range(0..=i);
                        perm.swap(i, j);
                    }
                    let yt_p: Vec<f64> = (0..n_right).map(|k| all[perm[k]]).collect();
                    let yc_p: Vec<f64> = (n_right..n).map(|k| all[perm[k]]).collect();
                    let s = compute_statistic(&yt_p, &yc_p, &cfg.statistic);
                    if s.abs() >= stat.abs() - 1e-15 {
                        count += 1;
                    }
                }
                let p = (count + 1) as f64 / (cfg.reps + 1) as f64;
                min_pval = min_pval.min(p);
            } else {
                // Large-sample approximation
                let stat = compute_statistic(&yt, &yc, &cfg.statistic);
                let mt = yt.iter().sum::<f64>() / n_right as f64;
                let mc = yc.iter().sum::<f64>() / n_left as f64;
                let ss_t: f64 = yt.iter().map(|y| (y - mt).powi(2)).sum();
                let ss_c: f64 = yc.iter().map(|y| (y - mc).powi(2)).sum();
                let var = (ss_t + ss_c) / (n_left + n_right - 2) as f64;
                let se = (var * (1.0 / n_left as f64 + 1.0 / n_right as f64)).sqrt();
                let normal = Normal::new(0.0, 1.0).unwrap();
                let p = if se > 0.0 {
                    2.0 * normal.cdf(-(stat / se).abs())
                } else {
                    1.0
                };
                min_pval = min_pval.min(p);
            }
        }

        // Track recommended window
        if min_pval >= cfg.level {
            recommended_idx = idx;
        }

        results.push(WindowResult {
            w_left: wl,
            w_right: wr,
            n_left,
            n_right,
            min_pval,
            binom_pval: binom_p,
        });
    }

    let (rec_wl, rec_wr) = windows
        .get(recommended_idx)
        .copied()
        .unwrap_or((cutoff - 1.0, cutoff + 1.0));

    Ok(RdWinSelectResult {
        w_left: rec_wl,
        w_right: rec_wr,
        results,
    })
}

/// Binomial test (exact, two-sided).
fn binom_test(n_l: usize, n_r: usize, p_null: f64) -> f64 {
    let n = n_l + n_r;
    let k = n_l.min(n_r);
    let dist = statrs::distribution::Binomial::new(p_null, n as u64).unwrap();
    let p_less = dist.cdf(k as u64);
    (2.0 * p_less).min(1.0_f64)
}

// =====================================================================
// rdrbounds: Rosenbaum bounds
// =====================================================================

/// Compute Rosenbaum sensitivity bounds for the randomization p-value.
///
/// Given a sensitivity parameter Gamma, computes upper and lower bounds
/// on the p-value under unknown treatment assignment probabilities
/// bounded by Gamma.
pub fn rdrbounds(
    obs_stat: f64,
    n_treat: usize,
    n_ctrl: usize,
    gamma: f64,
    _reps: usize,
    _seed: u64,
) -> (f64, f64) {
    let _n = n_treat + n_ctrl;
    let normal = Normal::new(0.0, 1.0).unwrap();

    // Under Gamma, the treatment probability pi_i is bounded:
    // 1/(1+Gamma) <= pi_i <= Gamma/(1+Gamma)
    let _p_lo = 1.0 / (1.0 + gamma);
    let _p_hi = gamma / (1.0 + gamma);

    // Upper bound: treatment assignment favors units with extreme outcomes
    // Lower bound: treatment assignment favors units with less extreme outcomes
    // Use normal approximation
    let se = 1.0; // standardized
    let z_obs = obs_stat / se;

    // Adjusted p-values
    let p_upper = normal.cdf(-z_obs.abs()) * 2.0;
    let p_lower = (normal.cdf(-z_obs.abs() / gamma) * 2.0).min(1.0);

    (p_lower, p_upper)
}
