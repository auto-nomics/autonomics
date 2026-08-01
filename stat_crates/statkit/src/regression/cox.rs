//! Cox proportional hazards regression via partial likelihood.
//!
//! Fits the semiparametric model `h(t|X) = h₀(t)·exp(Xβ)` by maximising
//! Cox's partial likelihood with Breslow's tie approximation. Newton-Raphson
//! (IRLS) iterations solve the score equations; the information matrix
//! provides Wald standard errors, z-tests, and hazard-ratio confidence
//! intervals. Harrell's concordance index (C) quantifies discriminative
//! ability.
//!
//! # Notation
//!
//! - **Time** `t_i`: observed time (event or censoring) for subject `i`.
//! - **Event** `δ_i ∈ {0, 1}`: 1 if `t_i` is an event time, 0 if censored.
//! - **Risk set** `R(t)` = `{j : t_j ≥ t}` — subjects still at risk at time `t`.
//! - **S₀(t)** = `Σ_{j∈R(t)} exp(η_j)`, **S₁(t)** = `Σ_{j∈R(t)} X_j·exp(η_j)`.
//!
//! With Breslow ties, a group of `d` tied events at time `t` contributes:
//!
//! ```text
//! logL += Σ_events η_i − d · log S₀(t)
//! score  += Σ_events X_i − d · S₁/S₀
//! info   += d · (S₂/S₀ − (S₁/S₀)⊗(S₁/S₀))
//! ```

use faer::linalg::solvers::{DenseSolveCore, Llt, Solve};
use faer::{Mat, Side};
use statrs::distribution::{ContinuousCDF, Normal};

use crate::error::{Result, StatError};

// ── Result ─────────────────────────────────────────────────────────────────

/// Result of a Cox PH regression fit.
#[derive(Debug, Clone)]
pub struct CoxResult {
    /// Estimated coefficients (no intercept — absorbed by the baseline hazard).
    pub coefficients: Vec<f64>,
    /// Standard error of each coefficient.
    pub std_errors: Vec<f64>,
    /// Wald z-statistic `β / SE`.
    pub z_stats: Vec<f64>,
    /// Two-sided p-value from the standard Normal.
    pub p_values: Vec<f64>,
    /// Hazard ratio `exp(β)`.
    pub hazard_ratios: Vec<f64>,
    /// 95% CI lower bound for the hazard ratio.
    pub hr_ci_lower: Vec<f64>,
    /// 95% CI upper bound for the hazard ratio.
    pub hr_ci_upper: Vec<f64>,
    /// Fitted linear predictor `η = Xβ` for each observation.
    pub linear_predictor: Vec<f64>,
    /// Partial log-likelihood at convergence.
    pub log_likelihood: f64,
    /// Log-likelihood of the null model (β = 0).
    pub null_log_likelihood: f64,
    /// Harrell's concordance index (C-statistic).
    pub concordance: f64,
    /// Number of observations.
    pub n_obs: usize,
    /// Number of events (δ = 1).
    pub n_events: usize,
    /// Number of parameters.
    pub n_params: usize,
    /// Whether Newton-Raphson converged.
    pub converged: bool,
    /// Iterations used.
    pub n_iter: usize,
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Fit a Cox proportional hazards model.
///
/// `time` is the observed survival time (event or censoring). `event` must be
/// 0 (censored) or 1 (event). `predictors` are parallel numeric columns. No
/// intercept is fitted (it is absorbed into the baseline hazard).
///
/// Ties are handled via Breslow's approximation (matching R's
/// `coxph(..., ties = "breslow")`).
pub fn cox(time: &[f64], event: &[f64], predictors: &[&[f64]]) -> Result<CoxResult> {
    const MAX_ITER: usize = 50;
    const TOL: f64 = 1e-8;

    let n = time.len();
    if n == 0 {
        return Err(StatError::EmptyInput);
    }
    if event.len() != n {
        return Err(StatError::LengthMismatch {
            a: n,
            b: event.len(),
        });
    }
    for &e in event {
        if e != 0.0 && e != 1.0 {
            return Err(StatError::InvalidInput(format!(
                "event indicator must be 0 or 1, got {e}"
            )));
        }
    }
    let p = predictors.len();
    if p == 0 {
        return Err(StatError::InvalidInput(
            "at least one predictor required".to_string(),
        ));
    }
    for pred in predictors.iter() {
        if pred.len() != n {
            return Err(StatError::LengthMismatch {
                a: n,
                b: pred.len(),
            });
        }
    }

    let n_events = event.iter().filter(|&&e| e == 1.0).count();
    if n_events == 0 {
        return Err(StatError::InvalidInput(
            "no events in the dataset — cannot fit Cox model".to_string(),
        ));
    }

    // Sort indices by time descending for risk-set accumulation.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        time[b]
            .partial_cmp(&time[a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // ── Newton-Raphson ──────────────────────────────────────────────────
    let mut beta = vec![0.0_f64; p];
    let mut converged = false;
    let mut n_iter = 0;

    for iter in 0..MAX_ITER {
        n_iter = iter + 1;

        let (score, info, ll_current) =
            score_info_ll(time, event, predictors, &beta, &order, p, n)?;

        // Newton update: β += I⁻¹ · U
        let a = Mat::from_fn(p, p, |i, j| info[i][j]);
        let u = Mat::from_fn(p, 1, |i, _| score[i]);
        let llt = Llt::new(a.as_ref(), Side::Lower)
            .ok()
            .ok_or(StatError::SingularMatrix)?;
        let delta_mat = llt.solve(&u);
        let delta: Vec<f64> = (0..p).map(|i| delta_mat[(i, 0)]).collect();

        // Step-halving: if the full Newton step worsens the log-likelihood
        // (e.g. near monotone-likelihood / separation), halve until it improves.
        let mut step = 1.0_f64;
        let beta_trial = loop {
            let trial = (0..p)
                .map(|i| beta[i] + delta[i] * step)
                .collect::<Vec<_>>();
            let ll_trial = score_info_ll(time, event, predictors, &trial, &order, p, n)
                .map(|(_, _, ll)| ll)
                .unwrap_or(f64::NEG_INFINITY);
            if ll_trial >= ll_current || step < 1e-6 {
                break trial;
            }
            step *= 0.5;
        };

        let max_delta = delta
            .iter()
            .map(|d| (d * step).abs())
            .fold(0.0_f64, f64::max);
        beta = beta_trial;

        if max_delta < TOL {
            converged = true;
            break;
        }
    }

    // ── Final inference ────────────────────────────────────────────────
    let (score, info, ll) = score_info_ll(time, event, predictors, &beta, &order, p, n)?;
    let _ = score; // not needed after convergence

    // Inverse information for standard errors.
    let a = Mat::from_fn(p, p, |i, j| info[i][j]);
    let llt = Llt::new(a.as_ref(), Side::Lower)
        .ok()
        .ok_or(StatError::SingularMatrix)?;
    let inv_info = llt.inverse();

    // Null log-likelihood (β = 0 → all η = 0 → exp(η) = 1).
    let beta_zero = vec![0.0; p];
    let (_, _, null_ll) = score_info_ll(time, event, predictors, &beta_zero, &order, p, n)?;

    // Linear predictor.
    let eta: Vec<f64> = (0..n)
        .map(|i| (0..p).map(|j| beta[j] * predictors[j][i]).sum::<f64>())
        .collect();

    // C-index.
    let c = harrell_c(&eta, time, event);

    // Wald inference.
    let z975 = 1.959963984540054;
    let normal = Normal::new(0.0, 1.0).map_err(|e| StatError::Numerical(format!("Normal: {e}")))?;

    let std_errors: Vec<f64> = (0..p).map(|i| inv_info[(i, i)].max(0.0).sqrt()).collect();
    let z_stats: Vec<f64> = (0..p)
        .map(|i| {
            if std_errors[i] > 0.0 {
                beta[i] / std_errors[i]
            } else {
                f64::NAN
            }
        })
        .collect();
    let p_values: Vec<f64> = (0..p)
        .map(|i| {
            let z = z_stats[i].abs();
            if z.is_finite() {
                2.0 * normal.sf(z)
            } else {
                f64::NAN
            }
        })
        .collect();
    let hazard_ratios: Vec<f64> = beta.iter().map(|&b| b.exp()).collect();
    let hr_ci_lower: Vec<f64> = (0..p)
        .map(|i| (beta[i] - z975 * std_errors[i]).exp())
        .collect();
    let hr_ci_upper: Vec<f64> = (0..p)
        .map(|i| (beta[i] + z975 * std_errors[i]).exp())
        .collect();

    Ok(CoxResult {
        coefficients: beta.clone(),
        std_errors,
        z_stats,
        p_values,
        hazard_ratios,
        hr_ci_lower,
        hr_ci_upper,
        linear_predictor: eta,
        log_likelihood: ll,
        null_log_likelihood: null_ll,
        concordance: c,
        n_obs: n,
        n_events,
        n_params: p,
        converged,
        n_iter,
    })
}

// ── Score, information, and log-likelihood ─────────────────────────────────

/// Compute the partial-likelihood score vector, information matrix, and
/// log-likelihood at the given `beta`. Observations are processed in the
/// time-descending `order` so that risk-set sums accumulate incrementally.
#[allow(clippy::too_many_arguments)]
fn score_info_ll(
    time: &[f64],
    event: &[f64],
    x: &[&[f64]],
    beta: &[f64],
    order: &[usize],
    p: usize,
    n: usize,
) -> Result<(Vec<f64>, Vec<Vec<f64>>, f64)> {
    // Precompute η and exp(η) for all observations.
    let eta: Vec<f64> = (0..n)
        .map(|i| (0..p).map(|j| beta[j] * x[j][i]).sum::<f64>())
        .collect();
    let exp_eta: Vec<f64> = eta.iter().map(|&e| e.exp().min(1e300)).collect();

    let mut score = vec![0.0_f64; p];
    let mut info = vec![vec![0.0_f64; p]; p];
    let mut ll = 0.0_f64;

    // Risk-set running sums (accumulated from latest time to earliest).
    let mut s0 = 0.0_f64; // Σ exp(η)
    let mut s1 = vec![0.0_f64; p]; // Σ X·exp(η)
    let mut s2 = vec![vec![0.0_f64; p]; p]; // Σ X·X'·exp(η)

    let mut i = 0;
    while i < n {
        let t = time[order[i]];

        // Collect all observations at this time (they enter the risk set).
        let mut group_end = i;
        while group_end < n && (time[order[group_end]] - t).abs() < f64::EPSILON * t.abs().max(1.0)
        {
            let idx = order[group_end];
            let w = exp_eta[idx];
            s0 += w;
            for k in 0..p {
                let xk = x[k][idx];
                s1[k] += w * xk;
                for l in 0..p {
                    s2[k][l] += w * xk * x[l][idx];
                }
            }
            group_end += 1;
        }

        // Count events in this time group (Breslow ties).
        let event_indices: Vec<usize> = order[i..group_end]
            .iter()
            .copied()
            .filter(|&idx| event[idx] == 1.0)
            .collect();
        let d = event_indices.len();

        if d > 0 && s0 > 0.0 {
            let inv_s0 = 1.0 / s0;
            let x_bar: Vec<f64> = s1.iter().map(|s| s * inv_s0).collect();

            // Score: Σ_events X_event − d · X̄
            for &idx in &event_indices {
                for k in 0..p {
                    score[k] += x[k][idx] - x_bar[k];
                }
            }

            // Information: d · (S₂/S₀ − X̄ ⊗ X̄)
            for k in 0..p {
                for l in 0..p {
                    info[k][l] += d as f64 * (s2[k][l] * inv_s0 - x_bar[k] * x_bar[l]);
                }
            }

            // Log-likelihood: Σ_Events η − d · log(S₀)
            for &idx in &event_indices {
                ll += eta[idx];
            }
            ll -= d as f64 * s0.ln();
        }

        i = group_end;
    }

    Ok((score, info, ll))
}

// ── Harrell's concordance index ────────────────────────────────────────────

/// Compute Harrell's C-statistic from the fitted linear predictor.
///
/// For each comparable pair (i had an event before j's observed time), the
/// pair is concordant if `η_i > η_j`. Ties in `η` contribute 0.5.
fn harrell_c(eta: &[f64], time: &[f64], event: &[f64]) -> f64 {
    let n = eta.len();
    let mut comparable = 0u64;
    let mut concordant = 0.0_f64;

    for i in 0..n {
        if event[i] != 1.0 {
            continue;
        }
        for j in 0..n {
            if i == j {
                continue;
            }
            // Comparable: i had an event at time_i, and j survived past time_i.
            if time[i] < time[j] {
                comparable += 1;
                if eta[i] > eta[j] {
                    concordant += 1.0;
                } else if eta[i] == eta[j] {
                    concordant += 0.5;
                }
            }
        }
    }

    if comparable == 0 {
        return f64::NAN;
    }
    concordant / comparable as f64
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_predictor_positive_assoc() {
        // x strongly positively associated with hazard → β > 0, HR > 1.
        let time = vec![10.0, 20.0, 30.0, 40.0, 50.0, 5.0, 15.0, 25.0, 35.0, 45.0];
        let event = vec![1.0; 10];
        let x: Vec<f64> = (0..10).map(|i| if i < 5 { 0.0 } else { 1.0 }).collect();
        let fit = cox(&time, &event, &[&x[..]]).unwrap();
        assert!(fit.converged, "should converge");
        assert!(fit.coefficients[0] > 0.0, "β should be positive");
        assert!(fit.hazard_ratios[0] > 1.0, "HR should be > 1");
        assert!(fit.concordance > 0.5, "C should be > 0.5");
    }

    #[test]
    fn zero_effect_gives_beta_near_zero() {
        // x unrelated to survival time → β ≈ 0.
        let time: Vec<f64> = (1..=20).map(|i| i as f64).collect();
        let event: Vec<f64> = vec![1.0; 20];
        let x: Vec<f64> = (0..20)
            .map(|i| if i % 2 == 0 { 1.0 } else { 0.0 })
            .collect();
        let fit = cox(&time, &event, &[&x[..]]).unwrap();
        assert!(
            fit.coefficients[0].abs() < 1.0,
            "β should be near zero, got {}",
            fit.coefficients[0]
        );
    }

    #[test]
    fn censored_observations_handled() {
        // Mix of events and censored.
        let time = vec![5.0, 10.0, 15.0, 20.0, 25.0, 3.0, 8.0, 12.0, 18.0, 30.0];
        let event = vec![1.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 1.0];
        let x = vec![0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 1.0];
        let fit = cox(&time, &event, &[&x[..]]).unwrap();
        assert!(fit.converged);
        assert_eq!(fit.n_events, 6);
        assert_eq!(fit.n_obs, 10);
    }

    #[test]
    fn hr_ci_brackets_hr() {
        // Use shuffled times to avoid monotone likelihood.
        let time = vec![
            5.0, 12.0, 3.0, 18.0, 8.0, 25.0, 1.0, 15.0, 20.0, 10.0, 7.0, 22.0, 14.0, 2.0, 16.0,
            9.0, 30.0, 6.0, 11.0, 28.0, 4.0, 13.0, 24.0, 19.0, 27.0, 17.0, 21.0, 26.0, 23.0, 29.0,
        ];
        let event: Vec<f64> = vec![1.0; 30];
        let x: Vec<f64> = (0..30).map(|i| (i as f64) * 0.1).collect();
        let fit = cox(&time, &event, &[&x[..]]).unwrap();
        assert!(fit.hr_ci_lower[0] <= fit.hazard_ratios[0] + 1e-10);
        assert!(fit.hr_ci_upper[0] >= fit.hazard_ratios[0] - 1e-10);
    }

    #[test]
    fn multiple_predictors() {
        let n = 30;
        // Shuffled times.
        let time = vec![
            5.0, 12.0, 3.0, 18.0, 8.0, 25.0, 1.0, 15.0, 20.0, 10.0, 7.0, 22.0, 14.0, 2.0, 16.0,
            9.0, 30.0, 6.0, 11.0, 28.0, 4.0, 13.0, 24.0, 19.0, 27.0, 17.0, 21.0, 26.0, 23.0, 29.0,
        ];
        let event: Vec<f64> = vec![1.0; n];
        let x1: Vec<f64> = (0..n).map(|i| i as f64 * 0.1).collect();
        let x2: Vec<f64> = (0..n).map(|i| if i % 3 == 0 { 1.0 } else { 0.0 }).collect();
        let fit = cox(&time, &event, &[&x1[..], &x2[..]]).unwrap();
        assert_eq!(fit.n_params, 2);
        assert!(fit.converged);
        for i in 0..2 {
            assert!(fit.p_values[i].is_finite());
        }
    }

    #[test]
    fn rejects_no_events() {
        let time = vec![10.0, 20.0, 30.0];
        let event = vec![0.0, 0.0, 0.0];
        let x = vec![1.0, 2.0, 3.0];
        assert!(cox(&time, &event, &[&x[..]]).is_err());
    }

    #[test]
    fn rejects_invalid_event() {
        let time = vec![10.0, 20.0, 30.0];
        let event = vec![1.0, 0.5, 0.0];
        let x = vec![1.0, 2.0, 3.0];
        assert!(cox(&time, &event, &[&x[..]]).is_err());
    }

    #[test]
    fn log_likelihood_negative() {
        let time = vec![
            5.0, 12.0, 3.0, 18.0, 8.0, 25.0, 1.0, 15.0, 20.0, 10.0, 7.0, 22.0, 14.0, 2.0, 16.0,
            9.0, 30.0, 6.0, 11.0, 28.0,
        ];
        let event: Vec<f64> = vec![1.0; 20];
        let x: Vec<f64> = (0..20).map(|i| i as f64).collect();
        let fit = cox(&time, &event, &[&x[..]]).unwrap();
        assert!(fit.log_likelihood.is_finite());
        assert!(fit.null_log_likelihood.is_finite());
    }

    #[test]
    fn concordance_range() {
        let time = vec![
            5.0, 12.0, 3.0, 18.0, 8.0, 25.0, 1.0, 15.0, 20.0, 10.0, 7.0, 22.0, 14.0, 2.0, 16.0,
            9.0, 30.0, 6.0, 11.0, 28.0,
        ];
        let event: Vec<f64> = vec![1.0; 20];
        let x: Vec<f64> = (0..20).map(|i| i as f64).collect();
        let fit = cox(&time, &event, &[&x[..]]).unwrap();
        assert!(fit.concordance >= 0.0 && fit.concordance <= 1.0);
    }
}
