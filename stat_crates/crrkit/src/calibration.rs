//! Competing-risk calibration of a cause-1 cumulative-incidence prediction
//! (Gerds & Andersen 2014, as in `riskRegression` calibration plots).
//!
//! The observed event probability at `t*` is estimated by the IPCW average
//! of the cause-1 event indicator,
//!
//! ```text
//!   obs(t*) = (1/n) Σ_i I(U_i ≤ t*, δ_i = 1) / G(U_i-)
//! ```
//!
//! which is unbiased for `P(cause 1 by t*)` under independent censoring and
//! matches the quantity the Brier decomposition calibrates against.
//! Competing deaths enter the denominator `n` as known non-events.
//!
//! Two views are provided:
//!
//! * [`grouped_calibration`] — mean predicted vs observed per quantile group
//!   of the prediction. Group count is caller-controlled: small validation
//!   sets must not be forced into ten groups (SAP §9.4);
//! * [`calibration_slope`] — IPCW-weighted least squares of the binary
//!   known-outcome indicator on the prediction, giving a slope (ideal 1) and
//!   an intercept-in-the-large (ideal 0) on the identity link.

use crate::brier::BrierOptions;
use crate::censoring::ReverseKmCensoring;
use crate::error::{CrrkitError, Result};
use crate::brier::validate_inputs;

/// One calibration group: mean prediction vs IPCW-observed event probability.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CalibrationGroup {
    /// 1-based group index over ascending prediction quantiles.
    pub group: usize,
    /// Subjects in the group.
    pub n: usize,
    /// Mean predicted cause-1 incidence at the horizon.
    pub mean_predicted: f64,
    /// IPCW-observed cause-1 incidence at the horizon.
    pub observed: f64,
    /// Subjects with a cause-1 event in the group (unweighted count).
    pub n_cause1: usize,
    /// Prediction range covered by the group, `(lo, hi)` inclusive.
    pub prediction_range: (f64, f64),
}

/// Quantile-grouped calibration at `horizon`.
///
/// Groups are formed over **all** subjects (including pre-horizon censored
/// ones, whose predictions count against the group mean) by quantiles of the
/// prediction. The observed value divides the IPCW weighted cause-1 count by
/// the group size, exactly mirroring how the full-cohort Brier divides by
/// `n`.
pub fn grouped_calibration(
    times: &[f64],
    status: &[u8],
    prob: &[f64],
    horizon: f64,
    n_groups: usize,
    _opts: &BrierOptions,
) -> Result<Vec<CalibrationGroup>> {
    validate_inputs(times, status, prob, horizon)?;
    if n_groups == 0 {
        return Err(CrrkitError::Invalid("n_groups must be positive".to_string()));
    }
    let n = times.len();
    if n == 0 {
        return Err(CrrkitError::EmptyCalibration { n });
    }
    let g = ReverseKmCensoring::new(times, status)?;

    // Subject order by prediction; groups are contiguous slices of it.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| prob[a].total_cmp(&prob[b]));

    let mut out = Vec::with_capacity(n_groups);
    for k in 0..n_groups {
        // Balanced split: subject j (0-based in sorted order) goes to group
        // floor(k * n_groups / n)... invert: group of j = j * n_groups / n.
        let lo = (k * n) / n_groups;
        let hi = ((k + 1) * n) / n_groups;
        if hi <= lo {
            continue; // more groups than subjects
        }
        let members = &order[lo..hi];

        let mut sum_pred = 0.0_f64;
        let mut obs_weighted = 0.0_f64;
        let mut n_cause1 = 0usize;
        let mut p_lo = f64::INFINITY;
        let mut p_hi = f64::NEG_INFINITY;
        for &i in members {
            sum_pred += prob[i];
            if prob[i] < p_lo {
                p_lo = prob[i];
            }
            if prob[i] > p_hi {
                p_hi = prob[i];
            }
            if times[i] <= horizon && status[i] == 1 {
                n_cause1 += 1;
                obs_weighted += 1.0 / g.eval_left(times[i]);
            }
        }
        let m = members.len();
        out.push(CalibrationGroup {
            group: k + 1,
            n: m,
            mean_predicted: sum_pred / m as f64,
            observed: obs_weighted / m as f64,
            n_cause1,
            prediction_range: (p_lo, p_hi),
        });
    }
    if out.is_empty() {
        return Err(CrrkitError::EmptyCalibration { n });
    }
    Ok(out)
}

/// IPCW-weighted identity-link calibration of the known binary outcome
/// `Y = I(cause 1 by t*)` on the prediction `p`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CalibrationSlope {
    /// Horizon `t*`.
    pub horizon: f64,
    /// Weighted slope of `Y` on `p` (ideal `1`).
    pub slope: f64,
    /// Weighted intercept-in-the-large (ideal `0`): mean residual at `p = 0`.
    pub intercept: f64,
    /// Subjects with a known outcome entering the regression.
    pub n_scored: usize,
    /// Weighted mean of the predictions.
    pub weighted_mean_prediction: f64,
    /// Weighted mean of the observed indicators.
    pub weighted_mean_outcome: f64,
    /// `G(t*)`, reported per SAP.
    pub g_at_horizon: f64,
}

/// Fit the IPCW calibration slope at `horizon`.
///
/// Subjects with a known outcome at `t*` are
/// `δ = 1, U ≤ t*` (`Y = 1`, weight `1/G(U-)`), `δ = 2, U ≤ t*`
/// (`Y = 0`, weight `1/G(U-)`), and `U > t*` (`Y = 0`, weight `1/G(t*)`).
/// Pre-horizon censored subjects are excluded from the regression.
pub fn calibration_slope(
    times: &[f64],
    status: &[u8],
    prob: &[f64],
    horizon: f64,
    opts: &BrierOptions,
) -> Result<CalibrationSlope> {
    validate_inputs(times, status, prob, horizon)?;
    let g = ReverseKmCensoring::new(times, status)?;

    // One pass to collect (p, Y, weight, time) for outcome-known subjects and
    // the weighted means, a second over the rows for the centered moments.
    let scored: Vec<(f64, f64, f64, f64)> = times
        .iter()
        .zip(status.iter())
        .zip(prob.iter())
        .filter_map(|((&u, &s), &p)| {
            let (y, w) = if u > horizon {
                (0.0, 1.0 / g.eval(horizon))
            } else if s == 1 {
                (1.0, 1.0 / g.eval_left(u))
            } else if s == 2 {
                (0.0, 1.0 / g.eval_left(u))
            } else {
                return None;
            };
            Some((p, y, w, u))
        })
        .collect();
    let n_scored = scored.len();
    if n_scored < 2 {
        return Err(CrrkitError::NoOutcomeInformation { horizon });
    }
    for &(_, _, w, u) in &scored {
        if !w.is_finite() || w > 1.0 / opts.g_floor {
            return Err(CrrkitError::CollapsedCensoring { g: 1.0 / w, t: u });
        }
    }

    let sw: f64 = scored.iter().map(|&(_, _, w, _)| w).sum();
    let p_bar = scored.iter().map(|&(p, _, w, _)| w * p).sum::<f64>() / sw;
    let y_bar = scored.iter().map(|&(_, y, w, _)| w * y).sum::<f64>() / sw;
    let mut sxy = 0.0_f64;
    let mut sxx = 0.0_f64;
    for &(p, y, w, _) in &scored {
        sxy += w * (p - p_bar) * (y - y_bar);
        sxx += w * (p - p_bar) * (p - p_bar);
    }
    if sxx <= 0.0 {
        return Err(CrrkitError::Invalid(
            "predictions are constant among scored subjects: calibration slope is undefined"
                .to_string(),
        ));
    }
    let slope = sxy / sxx;
    let intercept = y_bar - slope * p_bar;

    Ok(CalibrationSlope {
        horizon,
        slope,
        intercept,
        n_scored,
        weighted_mean_prediction: p_bar,
        weighted_mean_outcome: y_bar,
        g_at_horizon: g.eval(horizon),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> BrierOptions {
        BrierOptions::default()
    }

    #[test]
    fn perfect_prediction_gives_unit_slope_and_zero_intercept() {
        // Y equals p exactly for every subject (no censoring).
        let times = [200.0, 300.0, 400.0, 1500.0];
        let status = [1u8, 1, 2, 0];
        let prob = [1.0, 1.0, 0.0, 0.0];
        let r = calibration_slope(&times, &status, &prob, 1095.75, &opts()).unwrap();
        assert!((r.slope - 1.0).abs() < 1e-12);
        assert!(r.intercept.abs() < 1e-12);
        assert_eq!(r.n_scored, 4);
    }

    #[test]
    fn over_confident_predictions_shrink_slope_below_one() {
        // One event among four; predictions noisy around it. Identity-link
        // slope Cov(Y,p)/Var(p): weighted Cov = 3/80, Var = 41/400,
        // slope = 15/41 < 1 (over-dispersed predictions).
        let times = [200.0, 300.0, 400.0, 1500.0];
        let status = [1u8, 2, 2, 0];
        let prob = [0.6, 0.9, 0.1, 0.2];
        let r = calibration_slope(&times, &status, &prob, 1095.75, &opts()).unwrap();
        assert!((r.slope - 15.0_f64 / 41.0).abs() < 1e-12);
        assert!(r.slope > 0.0 && r.slope < 1.0);
    }

    #[test]
    fn compressed_predictions_stretch_slope_above_one() {
        // Outcomes 1/1/0/0 with predictions compressed around 0.5:
        // Cov = 0.075, Var = 0.025, slope = 3 > 1 (under-confident spread).
        let times = [200.0, 300.0, 400.0, 1500.0];
        let status = [1u8, 1, 2, 0];
        let prob = [0.7, 0.6, 0.4, 0.3];
        let r = calibration_slope(&times, &status, &prob, 1095.75, &opts()).unwrap();
        assert!((r.slope - 3.0).abs() < 1e-12);
    }

    #[test]
    fn grouped_calibration_matches_hand_values() {
        // No censoring: observed = crude cause-1 rate per group.
        let times = [200.0, 300.0, 400.0, 1500.0];
        let status = [1u8, 1, 2, 0];
        let prob = [0.9, 0.8, 0.2, 0.1];
        let groups = grouped_calibration(&times, &status, &prob, 1095.75, 2, &opts()).unwrap();
        assert_eq!(groups.len(), 2);
        assert!((groups[0].observed - 0.0).abs() < 1e-12); // both non-events
        assert!((groups[1].observed - 1.0).abs() < 1e-12); // both events
        assert!((groups[0].mean_predicted - 0.15).abs() < 1e-12);
        assert!((groups[1].mean_predicted - 0.85).abs() < 1e-12);
    }

    #[test]
    fn grouped_calibration_more_groups_than_subjects_is_reported_not_crashed() {
        let times = [200.0, 300.0];
        let status = [1u8, 2];
        let prob = [0.4, 0.6];
        let groups = grouped_calibration(&times, &status, &prob, 1095.75, 5, &opts()).unwrap();
        assert!(groups.len() >= 1 && groups.len() <= 2);
    }

    #[test]
    fn constant_predictions_rejected() {
        let times = [200.0, 300.0, 400.0];
        let status = [1u8, 2, 0];
        let prob = [0.5, 0.5, 0.5];
        assert!(calibration_slope(&times, &status, &prob, 1095.75, &opts()).is_err());
    }
}
