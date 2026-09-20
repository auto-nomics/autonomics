//! IPCW Brier score for a cause-1 cumulative-incidence prediction under
//! competing risks (Schoop et al. 2011; Blanche, Saarela & Scheike as
//! implemented in `riskRegression::Score`).
//!
//! With `U` the observed time, `δ ∈ {0, 1, 2}` the status (0 censored,
//! 1 event of interest, 2 competing event), `p` the predicted cumulative
//! incidence of cause 1 at horizon `t*`, and `G` the reverse-Kaplan–Meier
//! censoring survival, each subject contributes
//!
//! ```text
//!   δ = 1, U ≤ t* :  (1 - p)² / G(U-)
//!   δ = 2, U ≤ t* :  p²     / G(U-)
//!   U > t*        :  p²     / G(t*)
//!   censored, U < t* : nothing
//! ```
//!
//! and the score is the sum of contributions divided by the **total** number
//! of subjects `n` — not by the number of contributors. Subjects with a
//! competing event before the horizon are *not* censored: their outcome
//! ("no cause-1 event before death") is known and contributes `p² / G(U-)`
//! with the same left-continuous weighting as cause-1 events.
//!
//! This module deliberately supersedes the single-event IPCW Brier in
//! `stat_crates/dl/src/survival.rs`, which treats every non-cause-1
//! observation as censoring and therefore must not be used for
//! competing-risk endpoints.

use crate::censoring::ReverseKmCensoring;
use crate::error::{CrrkitError, Result};

/// Options controlling the IPCW Brier computation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct BrierOptions {
    /// Weights strictly above this value are flagged as extreme in
    /// [`IpcwBrier::n_extreme_weights`]. Common practice flags `w > 10`.
    pub extreme_weight_threshold: f64,
    /// Floor for the censoring survival `G`; a required weight with
    /// `G` below this floor raises [`CrrkitError::CollapsedCensoring`]
    /// instead of returning an infinite score.
    pub g_floor: f64,
}

impl Default for BrierOptions {
    fn default() -> Self {
        Self {
            extreme_weight_threshold: 10.0,
            g_floor: 1e-5,
        }
    }
}

/// Result of an IPCW Brier evaluation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IpcwBrier {
    /// Horizon `t*` the predictions refer to.
    pub horizon: f64,
    /// Total number of subjects, `n`.
    pub n_total: usize,
    /// Subjects contributing a term (everything except pre-horizon censoring).
    pub n_contributors: usize,
    /// Cause-1 events at or before the horizon.
    pub n_cause1: usize,
    /// Competing events at or before the horizon.
    pub n_competing: usize,
    /// Subjects event-free beyond the horizon.
    pub n_beyond_horizon: usize,
    /// Subjects censored before the horizon (no contribution).
    pub n_censored_before_horizon: usize,
    /// The Brier score: `Σ contributions / n`.
    pub brier: f64,
    /// `G(t*)` — the censoring survival at the horizon, as reported per SAP.
    pub g_at_horizon: f64,
    /// Largest inverse-censoring weight actually used.
    pub max_weight: f64,
    /// Smallest censoring survival `G` attained up to and including the
    /// horizon (`min G(t)` for `t ≤ t*`), as reported per SAP.
    pub min_g_upto_horizon: f64,
    /// Number of contributor weights above
    /// `BrierOptions::extreme_weight_threshold`.
    pub n_extreme_weights: usize,
    /// Per-subject weighted contribution, `None` for pre-horizon censoring.
    pub contributions: Vec<Option<f64>>,
}

/// Compute the IPCW Brier score of predicted cause-1 cumulative incidence
/// at `horizon`.
///
/// * `times` — observed times `U` (days, per the frozen SAP convention);
/// * `status` — `0` censored, `1` cause-1 event, `2` competing event;
/// * `prob` — predicted `P(cause 1 by t*)` for every subject, in `[0, 1]`.
pub fn ipcw_brier(
    times: &[f64],
    status: &[u8],
    prob: &[f64],
    horizon: f64,
    opts: &BrierOptions,
) -> Result<IpcwBrier> {
    validate_inputs(times, status, prob, horizon)?;
    let g = ReverseKmCensoring::new(times, status)?;

    let n = times.len();
    let mut contributions: Vec<Option<f64>> = Vec::with_capacity(n);
    let mut sum = 0.0_f64;
    let mut n_cause1 = 0usize;
    let mut n_competing = 0usize;
    let mut n_beyond = 0usize;
    let mut n_censored_before = 0usize;
    let mut max_weight = 0.0_f64;
    let mut n_extreme = 0usize;

    for i in 0..n {
        let u = times[i];
        let s = status[i];
        let p = prob[i];

        let (term, weight) = if u > horizon {
            // Event-free at the horizon (any later status): outcome known.
            (p * p, 1.0 / g.eval(horizon))
        } else if s == 1 {
            ((1.0 - p) * (1.0 - p), 1.0 / g.eval_left(u))
        } else if s == 2 {
            (p * p, 1.0 / g.eval_left(u))
        } else {
            // Censored before the horizon: no direct contribution.
            n_censored_before += 1;
            contributions.push(None);
            continue;
        };

        check_weight(weight, opts, u)?;
        sum += term * weight;
        if weight > max_weight {
            max_weight = weight;
        }
        if weight > opts.extreme_weight_threshold {
            n_extreme += 1;
        }
        contributions.push(Some(term * weight));
        match s {
            1 if u <= horizon => n_cause1 += 1,
            2 if u <= horizon => n_competing += 1,
            _ => n_beyond += 1,
        }
    }

    let contributors = n_cause1 + n_competing + n_beyond;
    if contributors == 0 {
        return Err(CrrkitError::NoOutcomeInformation { horizon });
    }

    Ok(IpcwBrier {
        horizon,
        n_total: n,
        n_contributors: contributors,
        n_cause1,
        n_competing,
        n_beyond_horizon: n_beyond,
        n_censored_before_horizon: n_censored_before,
        brier: sum / n as f64,
        g_at_horizon: g.eval(horizon),
        min_g_upto_horizon: g.min_upto(horizon),
        max_weight,
        n_extreme_weights: n_extreme,
        contributions,
    })
}

/// Range-check the shared inputs of every scoring function in this crate.
pub(crate) fn validate_inputs(
    times: &[f64],
    status: &[u8],
    prob: &[f64],
    horizon: f64,
) -> Result<()> {
    if status.len() != times.len() {
        return Err(CrrkitError::LengthMismatch {
            what: "status",
            got: status.len(),
            expected: times.len(),
        });
    }
    if prob.len() != times.len() {
        return Err(CrrkitError::LengthMismatch {
            what: "prob",
            got: prob.len(),
            expected: times.len(),
        });
    }
    if !horizon.is_finite() || horizon <= 0.0 {
        return Err(CrrkitError::BadHorizon { got: horizon });
    }
    for (i, &t) in times.iter().enumerate() {
        if !t.is_finite() || t < 0.0 {
            return Err(CrrkitError::BadTime { index: i, got: t });
        }
    }
    for (i, &s) in status.iter().enumerate() {
        if s > 2 {
            return Err(CrrkitError::BadStatus { index: i, got: s });
        }
    }
    for (i, &p) in prob.iter().enumerate() {
        if !p.is_finite() || !(0.0..=1.0).contains(&p) {
            return Err(CrrkitError::BadProbability { value: p, got: i });
        }
    }
    Ok(())
}

/// Reject a required weight whose denominator collapsed.
fn check_weight(weight: f64, opts: &BrierOptions, at: f64) -> Result<()> {
    if !weight.is_finite() || weight > 1.0 / opts.g_floor {
        Err(CrrkitError::CollapsedCensoring {
            g: 1.0 / weight,
            t: at,
        })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> BrierOptions {
        BrierOptions::default()
    }

    #[test]
    fn cause1_event_before_horizon_contributes_squared_error() {
        // Single cause-1 event at 200d, horizon 3y, p = 0.3, no censoring:
        // contribution (1 - 0.3)^2 = 0.49, BS = 0.49 / 1.
        let r = ipcw_brier(&[200.0], &[1], &[0.3], 1095.75, &opts()).unwrap();
        assert!((r.brier - 0.49).abs() < 1e-12);
        assert_eq!(r.n_contributors, 1);
        assert_eq!(r.n_cause1, 1);
    }

    #[test]
    fn competing_death_before_horizon_contributes_p_squared() {
        // Competing event at 400d: contribution p^2 = 0.09 — treated as a
        // known non-event, NOT as censoring.
        let r = ipcw_brier(&[400.0], &[2], &[0.3], 1095.75, &opts()).unwrap();
        assert!((r.brier - 0.09).abs() < 1e-12);
        assert_eq!(r.n_competing, 1);
    }

    #[test]
    fn beyond_horizon_contributes_p_squared_at_g_of_horizon() {
        let r = ipcw_brier(&[1500.0], &[1], &[0.3], 1095.75, &opts()).unwrap();
        assert!((r.brier - 0.09).abs() < 1e-12);
        assert_eq!(r.n_beyond_horizon, 1);
    }

    #[test]
    fn cohort_with_no_outcome_information_is_rejected() {
        // A single pre-horizon censored patient carries no outcome
        // information: the score is unestimable, not zero.
        let r = ipcw_brier(&[100.0], &[0], &[0.3], 1095.75, &opts());
        assert!(matches!(r, Err(crate::error::CrrkitError::NoOutcomeInformation { .. })));
    }

    #[test]
    fn weights_use_left_continuous_g_for_events() {
        // Five subjects; censorings at 100 (risk set 5) and 300 (risk set 3):
        //   G(100) = 4/5, G(300) = 4/5 * 2/3 = 8/15, held to the horizon.
        // Cause-1 event at 200 weighted by G(200-) = 4/5, competing event at
        // 400 by G(400-) = 8/15, beyond-horizon subject by G(3y) = 8/15.
        let times = [100.0, 200.0, 300.0, 400.0, 1500.0];
        let status = [0u8, 1, 0, 2, 0];
        let prob = [0.0, 0.5, 0.0, 0.5, 0.5];
        let r = ipcw_brier(&times, &status, &prob, 1095.75, &opts()).unwrap();
        let expected =
            (0.25 / (4.0 / 5.0) + 0.25 / (8.0 / 15.0) + 0.25 / (8.0 / 15.0)) / 5.0;
        assert!((r.brier - expected).abs() < 1e-12);
        assert!((r.g_at_horizon - 8.0 / 15.0).abs() < 1e-12);
        assert!((r.max_weight - 15.0 / 8.0).abs() < 1e-12);
        // Division is by total n = 5, contributors = 3.
        assert_eq!(r.n_contributors, 3);
        assert_eq!(r.n_censored_before_horizon, 2);
    }

    #[test]
    fn no_censoring_degenerates_to_ordinary_mse() {
        // With G ≡ 1 the score must equal mean (Y - p)^2 with
        // Y = I(cause 1 by t*), including competing events as Y = 0.
        let times = [200.0, 400.0, 1500.0, 900.0];
        let status = [1u8, 2, 1, 2];
        let prob = [0.8, 0.3, 0.6, 0.1];
        let r = ipcw_brier(&times, &status, &prob, 1095.75, &opts()).unwrap();
        let mse = ((1.0 - 0.8_f64).powi(2)
            + (0.0 - 0.3_f64).powi(2)
            + (0.0 - 0.6_f64).powi(2)
            + (0.0 - 0.1_f64).powi(2))
            / 4.0;
        assert!((r.brier - mse).abs() < 1e-12);
    }

    #[test]
    fn rejects_bad_inputs() {
        assert!(ipcw_brier(&[1.0], &[1], &[1.5], 3.0, &opts()).is_err());
        assert!(ipcw_brier(&[1.0], &[3], &[0.5], 3.0, &opts()).is_err());
        assert!(ipcw_brier(&[1.0], &[1], &[0.5], 0.0, &opts()).is_err());
        assert!(ipcw_brier(&[-1.0], &[1], &[0.5], 3.0, &opts()).is_err());
        assert!(ipcw_brier(&[1.0, 2.0], &[1], &[0.5], 3.0, &opts()).is_err());
    }
}
