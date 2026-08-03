//! Single-Effect Regression (SER) — the core mathematical building block of SuSiE.
//!
//! Each SER computes the posterior for one single-effect component: given
//! residual betahat / shat2, it computes the log-Bayes-factor per variable,
//! optimizes the prior variance V, and returns posterior moments (mu, mu2),
//! posterior inclusion probabilities (alpha), and the KL divergence.
//!
//! Faithfully ports:
//! - `gaussian_ser_lbf`  — Wakefield approximate Bayes factor
//! - `lbf_stabilization` — handles infinite shat2
//! - `compute_posterior_weights` — softmax(alpha + log(pi))
//! - `gaussian_ser_moments` — conjugate normal posterior
//! - `gaussian_ser_posterior_e_loglik` — E_q[loglik] for KL
//! - `optimize_scalar_prior_variance` — Brent + null-threshold check

use crate::brent::brent_minimize;
use crate::data::PriorMethod;
use crate::{log_sum_exp, EPS, SQRT_EPS};

/// Per-variable SER statistics (betahat, shat2).
pub struct SerStats {
    pub betahat: Vec<f64>,
    pub shat2: Vec<f64>,
    /// init point for Brent (log scale)
    pub optim_init: f64,
    /// bounds for Brent [lower, upper] on log scale
    pub optim_bounds: (f64, f64),
}

/// Result of a single-effect regression update.
pub struct SerResult {
    /// Posterior inclusion probabilities (length p).
    pub alpha: Vec<f64>,
    /// Posterior mean conditional on inclusion.
    pub mu: Vec<f64>,
    /// Posterior second moment conditional on inclusion.
    pub mu2: Vec<f64>,
    /// Optimized prior variance.
    pub v: f64,
    /// Model-level log Bayes factor.
    pub lbf_model: f64,
    /// Per-variable log Bayes factors (stabilized).
    pub lbf_variable: Vec<f64>,
    /// KL divergence for this effect.
    pub kl: f64,
}

/// Compute SER statistics from residuals and predictor weights.
///
/// Mirrors `compute_ser_statistics.ss`:
///   betahat_j = residuals_j / predictor_weights_j
///   shat2_j   = sigma2 / predictor_weights_j
pub fn compute_ser_stats(
    residuals: &[f64],
    sigma2: f64,
    predictor_weights: &[f64],
) -> SerStats {
    let p = residuals.len();
    let betahat: Vec<f64> = (0..p)
        .map(|j| residuals[j] / predictor_weights[j])
        .collect();
    let shat2: Vec<f64> = (0..p)
        .map(|j| sigma2 / predictor_weights[j])
        .collect();

    // optim_init = log(max(betahat² - shat2, 1)) on log scale
    let init_val: f64 = betahat
        .iter()
        .zip(shat2.iter())
        .map(|(&b, &s)| b * b - s)
        .fold(1.0_f64, |acc, x| acc.max(x))
        .max(1.0);
    let optim_init: f64 = init_val.ln();

    SerStats {
        betahat,
        shat2,
        optim_init,
        optim_bounds: (-30.0, 15.0),
    }
}

/// Gaussian single-effect log Bayes factors (Wakeform ABF).
///
/// `lbf_j = -0.5 * log(1 + V/shat2_j) + 0.5 * betahat_j² * V / (shat2_j * (V + shat2_j))`
///
/// Variables where betahat or shat2 are non-finite get lbf = 0.
pub fn gaussian_ser_lbf(betahat: &[f64], shat2: &[f64], v: f64) -> Vec<f64> {
    betahat
        .iter()
        .zip(shat2.iter())
        .map(|(&b, &s)| {
            if !b.is_finite() || !s.is_finite() {
                return 0.0;
            }
            let s_safe = s.max(EPS);
            -0.5 * (1.0 + v / s_safe).ln()
                + 0.5 * b * b * v / (s_safe * (v + s_safe))
        })
        .collect()
}

/// Stabilize lbf: when shat2 is infinite, set lbf=0 and lpo = prior.
///
/// Mirrors `lbf_stabilization`.
fn lbf_stabilization(lbf: &mut [f64], prior_weights: &[f64], shat2: &[f64]) -> Vec<f64> {
    let p = lbf.len();
    let mut lpo = vec![0.0; p];
    for j in 0..p {
        if shat2[j].is_infinite() {
            lbf[j] = 0.0;
            lpo[j] = (prior_weights[j] + SQRT_EPS).ln();
        } else {
            lpo[j] = lbf[j] + (prior_weights[j] + SQRT_EPS).ln();
        }
    }
    lpo
}

/// Compute posterior weights (alpha) and model-level lbf from log posterior odds.
///
/// Mirrors `compute_posterior_weights`:
///   w = exp(lpo - max(lpo))
///   alpha = w / sum(w)
///   lbf_model = log(sum(w)) + max(lpo)
fn compute_posterior_weights(lpo: &[f64]) -> (Vec<f64>, f64) {
    let max_lpo = lpo.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let w: Vec<f64> = lpo.iter().map(|&v| (v - max_lpo).exp()).collect();
    let sum_w: f64 = w.iter().sum();
    let alpha: Vec<f64> = w.iter().map(|&wi| wi / sum_w).collect();
    (alpha, sum_w.ln() + max_lpo)
}

/// Gaussian posterior moments for one SER.
///
///   post_var  = V * shat2 / (V + shat2)
///   post_mean = post_var / shat2 * betahat
pub fn gaussian_ser_moments(betahat: &[f64], shat2: &[f64], v: f64) -> (Vec<f64>, Vec<f64>) {
    let p = betahat.len();
    let mut post_mean = vec![0.0; p];
    let mut post_mean2 = vec![0.0; p];
    for j in 0..p {
        if !betahat[j].is_finite() || !shat2[j].is_finite() {
            continue;
        }
        let post_var = v * shat2[j] / (v + shat2[j]);
        post_mean[j] = post_var / shat2[j] * betahat[j];
        post_mean2[j] = post_var + post_mean[j] * post_mean[j];
    }
    (post_mean, post_mean2)
}

/// Gaussian posterior expected log-likelihood for one SER.
///
/// Mirrors `gaussian_ser_posterior_e_loglik`:
///   -0.5 * Σ_j [(-2*Eb_j*betahat_j + Eb2_j) / shat2_j]  (finite shat2 only)
pub fn gaussian_ser_posterior_e_loglik(
    alpha: &[f64],
    mu: &[f64],
    mu2: &[f64],
    betahat: &[f64],
    shat2: &[f64],
) -> f64 {
    let p = alpha.len();
    let mut sum = 0.0;
    for j in 0..p {
        if !shat2[j].is_finite() {
            continue;
        }
        let eb = alpha[j] * mu[j];
        let eb2 = alpha[j] * mu2[j];
        sum += (-2.0 * eb * betahat[j] + eb2) / shat2[j];
    }
    -0.5 * sum
}

/// Model-level lbf for a given V (used by the optimizer).
///
/// `lbf_model = logSumExp(gaussian_ser_lbf(betahat, shat2, V) + log(pi))`
fn lbf_model_at_v(
    stats: &SerStats,
    v: f64,
    prior_weights: &[f64],
    shat2: &[f64],
) -> f64 {
    let mut lbf = gaussian_ser_lbf(&stats.betahat, &stats.shat2, v);
    // lbf_stabilization: infinite shat2 → lbf=0, lpo=prior
    let mut lpo = vec![0.0; lbf.len()];
    for j in 0..lbf.len() {
        if shat2[j].is_infinite() {
            lbf[j] = 0.0;
            lpo[j] = (prior_weights[j] + SQRT_EPS).ln();
        } else {
            lpo[j] = lbf[j] + (prior_weights[j] + SQRT_EPS).ln();
        }
    }
    log_sum_exp(&lpo)
}

/// Run the full single-effect regression for one effect.
///
/// Mirrors `single_effect_regression` + pre/post-loglik-prior hooks.
///
/// For `optim`/`simple`: V is optimized/computed BEFORE the posterior.
/// For `EM`: the posterior is computed at the current V, THEN V is updated
///           as Σ alpha_j * mu2_j (post-hook, using just-computed moments).
pub fn single_effect_regression(
    stats: &SerStats,
    prior_weights: &[f64],
    current_v: f64,
    _prev_alpha: Option<&[f64]>,
    _prev_mu2: Option<&[f64]>,
    estimate_prior_variance: bool,
    method: PriorMethod,
    check_null_threshold: f64,
) -> SerResult {
    // ── pre-loglik prior hook: optimize V for optim/simple (no-op for EM) ──
    let v = if estimate_prior_variance && method != PriorMethod::Em {
        optimize_prior_variance(
            stats,
            prior_weights,
            current_v,
            method,
            check_null_threshold,
        )
    } else {
        current_v
    };

    // Compute lbf, alpha, lbf_model at V
    let mut lbf = gaussian_ser_lbf(&stats.betahat, &stats.shat2, v);
    let lpo = lbf_stabilization(&mut lbf, prior_weights, &stats.shat2);
    let (alpha, lbf_model) = compute_posterior_weights(&lpo);

    // Posterior moments at V
    let (mu, mu2) = gaussian_ser_moments(&stats.betahat, &stats.shat2, v);

    // KL = -lbf_model + E_q[loglik]
    let e_loglik =
        gaussian_ser_posterior_e_loglik(&alpha, &mu, &mu2, &stats.betahat, &stats.shat2);
    let kl = -lbf_model + e_loglik;

    // ── post-loglik prior hook: EM update using just-computed alpha/mu2 ──
    let v_final = if estimate_prior_variance && method == PriorMethod::Em {
        // EM does NOT apply check_null_threshold (susieR intentional, see
        // single_effect_regression.R comments)
        alpha.iter().zip(mu2.iter()).map(|(&a, &m)| a * m).sum()
    } else {
        v
    };

    SerResult {
        alpha,
        mu,
        mu2,
        v: v_final,
        lbf_model,
        lbf_variable: lbf,
        kl,
    }
}

/// Optimize the scalar prior variance (optim/simple only; EM handled in post-hook).
///
/// Mirrors `optimize_scalar_prior_variance`:
/// - `optim`: Brent on log(V) in [optim_bounds.0, optim_bounds.1]
/// - `simple`: no optimization (use current_v)
///
/// After optimization, check: if loglik(0) + threshold >= loglik(V), set V=0.
fn optimize_prior_variance(
    stats: &SerStats,
    prior_weights: &[f64],
    current_v: f64,
    method: PriorMethod,
    check_null_threshold: f64,
) -> f64 {
    let shat2 = &stats.shat2;
    let (lo, hi) = stats.optim_bounds;

    let mut v: f64 = current_v;

    match method {
        PriorMethod::Optim => {
            // Brent minimize neg_loglik(logV) on [lo, hi]
            let neg_loglik = |log_v: f64| -> f64 {
                let vv = log_v.exp();
                -lbf_model_at_v(stats, vv, prior_weights, shat2)
            };

            let v_param = brent_minimize(&neg_loglik, lo, hi);
            let v_new = v_param.exp();

            // Compare with init: keep the better one
            let v_param_init = current_v.ln();
            if neg_loglik(v_param) > neg_loglik(v_param_init) {
                v = current_v;
            } else {
                v = v_new;
            }
        }
        PriorMethod::Simple => {
            // No optimization
        }
        PriorMethod::Em => {
            // Should not reach here (EM is handled in the post-hook)
        }
    }

    // Null-threshold check: if loglik(0) + threshold >= loglik(V), set V=0
    let lbf_at_0 = lbf_model_at_v(stats, 0.0, prior_weights, shat2);
    let lbf_at_v = lbf_model_at_v(stats, v, prior_weights, shat2);
    if lbf_at_0 + check_null_threshold >= lbf_at_v {
        v = 0.0;
    }

    v
}
