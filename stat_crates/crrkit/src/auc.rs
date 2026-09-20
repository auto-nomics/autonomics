//! IPCW time-dependent AUC for a cause-1 risk score under competing risks
//! (Uno et al. 2007 adapted to competing endpoints, as in
//! `riskRegression::Score(metrics = "auc")`).
//!
//! At horizon `t*` the classification task is *cause-1 event by `t*`*:
//!
//! * **cases** — subjects with `δ = 1` and `U ≤ t*`, weighted `1 / G(U-)`;
//! * **controls** — subjects known free of the cause-1 event at `t*`:
//!   either event-free beyond the horizon (`U > t*`, weight `1 / G(t*)`) or
//!   dead of the competing cause before it (`δ = 2, U ≤ t*`, weight
//!   `1 / G(U-)`). Censoring *before* the horizon leaves the outcome unknown,
//!   so such subjects enter no pair.
//!
//! The AUC is the IPCW-weighted concordance probability over all
//! case–control pairs, with ties counted one half:
//!
//! ```text
//! AUC(t*) = Σ_{i case, j ctrl} w_i w_j [ I(s_i > s_j) + ½ I(s_i = s_j) ]
//!         / Σ_{i case, j ctrl} w_i w_j
//! ```
//!
//! Scores `s` point in the **risky** direction (higher score ⇔ higher
//! predicted risk). The pairwise loop is `O(n_cases × n_controls)`; the
//! rare-disease validation cohorts this is built for (order 10²–10³
//! subjects) stay well inside that budget.

use crate::brier::BrierOptions;
use crate::censoring::ReverseKmCensoring;
use crate::error::{CrrkitError, Result};

/// Result of an IPCW time-dependent AUC evaluation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IpcwAuc {
    /// Horizon `t*`.
    pub horizon: f64,
    /// Weighted concordance probability in `[0, 1]`.
    pub auc: f64,
    /// Number of case subjects (cause-1 events by `t*`).
    pub n_cases: usize,
    /// Number of control subjects (competing deaths plus event-free).
    pub n_controls: usize,
    /// Number of comparable case–control pairs actually scored.
    pub n_pairs: usize,
    /// Weighted count of concordant pairs (ties one half).
    pub concordant_weight: f64,
    /// Total pair weight (the denominator).
    pub total_weight: f64,
    /// `G(t*)`, reported alongside the estimate per SAP.
    pub g_at_horizon: f64,
    /// Number of pair weights above the extreme threshold — for pair weights
    /// this is `w_i * w_j`, so the threshold default is squared.
    pub n_extreme_pair_weights: usize,
}

/// Compute the IPCW time-dependent AUC at `horizon`.
///
/// * `times`, `status` — as in [`crate::brier::ipcw_brier`];
/// * `score` — risk score, higher = riskier (not required to be a
///   probability).
pub fn ipcw_auc(
    times: &[f64],
    status: &[u8],
    score: &[f64],
    horizon: f64,
    opts: &BrierOptions,
) -> Result<IpcwAuc> {
    if score.len() != times.len() {
        return Err(CrrkitError::LengthMismatch {
            what: "score",
            got: score.len(),
            expected: times.len(),
        });
    }
    for (i, &s) in score.iter().enumerate() {
        if !s.is_finite() {
            return Err(CrrkitError::Invalid(format!(
                "score at position {i} is not finite"
            )));
        }
    }
    // Probabilities and scores share the validation contract except for the
    // [0, 1] range; reuse the finite-time/status part via a zero-vector proxy
    // is wrong, so validate times/status/horizon directly here.
    validate_core(times, status, horizon)?;
    let g = ReverseKmCensoring::new(times, status)?;

    // Split subjects into (score, weight) pairs by role.
    let mut cases: Vec<(f64, f64)> = Vec::new();
    let mut controls: Vec<(f64, f64)> = Vec::new();
    for i in 0..times.len() {
        let u = times[i];
        let s = status[i];
        let sc = score[i];
        if u > horizon {
            let w = safe_weight(1.0 / g.eval(horizon), opts, u)?;
            controls.push((sc, w));
        } else if s == 1 {
            let w = safe_weight(1.0 / g.eval_left(u), opts, u)?;
            cases.push((sc, w));
        } else if s == 2 {
            let w = safe_weight(1.0 / g.eval_left(u), opts, u)?;
            controls.push((sc, w));
        }
        // s == 0 with u <= horizon: outcome unknown, no role.
    }

    if cases.is_empty() || controls.is_empty() {
        return Err(CrrkitError::NoOutcomeInformation { horizon });
    }

    let extreme_sq = opts.extreme_weight_threshold * opts.extreme_weight_threshold;
    let mut concordant = 0.0_f64;
    let mut total = 0.0_f64;
    let mut n_pairs = 0usize;
    let mut n_extreme = 0usize;
    for &(si, wi) in &cases {
        for &(sj, wj) in &controls {
            let pair_w = wi * wj;
            let c = if si > sj {
                1.0
            } else if si == sj {
                0.5
            } else {
                0.0
            };
            concordant += c * pair_w;
            total += pair_w;
            n_pairs += 1;
            if pair_w > extreme_sq {
                n_extreme += 1;
            }
        }
    }

    let auc = concordant / total;
    if !auc.is_finite() {
        return Err(CrrkitError::CollapsedCensoring { g: 0.0, t: horizon });
    }

    Ok(IpcwAuc {
        horizon,
        auc,
        n_cases: cases.len(),
        n_controls: controls.len(),
        n_pairs,
        concordant_weight: concordant,
        total_weight: total,
        g_at_horizon: g.eval(horizon),
        n_extreme_pair_weights: n_extreme,
    })
}

/// Times/status/horizon checks shared with [`validate_inputs`] but without
/// the probability-range part.
fn validate_core(times: &[f64], status: &[u8], horizon: f64) -> Result<()> {
    if status.len() != times.len() {
        return Err(CrrkitError::LengthMismatch {
            what: "status",
            got: status.len(),
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
    Ok(())
}

fn safe_weight(w: f64, opts: &BrierOptions, at: f64) -> Result<f64> {
    if !w.is_finite() || w > 1.0 / opts.g_floor {
        Err(CrrkitError::CollapsedCensoring { g: 1.0 / w, t: at })
    } else {
        Ok(w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> BrierOptions {
        BrierOptions::default()
    }

    #[test]
    fn perfectly_separated_scores_give_auc_one() {
        // Two cause-1 events (scores .8, .6) vs one competing death (.4) and
        // one event-free subject (.3): every pair concordant.
        let times = [200.0, 300.0, 400.0, 1500.0];
        let status = [1u8, 1, 2, 0];
        let score = [0.8, 0.6, 0.4, 0.3];
        let r = ipcw_auc(&times, &status, &score, 1095.75, &opts()).unwrap();
        assert!((r.auc - 1.0).abs() < 1e-12);
        assert_eq!(r.n_cases, 2);
        assert_eq!(r.n_controls, 2);
        assert_eq!(r.n_pairs, 4);
    }

    #[test]
    fn fully_reversed_scores_give_auc_zero() {
        let times = [200.0, 400.0];
        let status = [1u8, 2];
        let score = [0.2, 0.9];
        let r = ipcw_auc(&times, &status, &score, 1095.75, &opts()).unwrap();
        assert!(r.auc.abs() < 1e-12);
    }

    #[test]
    fn ties_count_half() {
        let times = [200.0, 400.0];
        let status = [1u8, 2];
        let score = [0.5, 0.5];
        let r = ipcw_auc(&times, &status, &score, 1095.75, &opts()).unwrap();
        assert!((r.auc - 0.5).abs() < 1e-12);
    }

    #[test]
    fn censored_before_horizon_enter_no_pair() {
        let times = [100.0, 200.0, 400.0];
        let status = [0u8, 1, 2];
        let score = [0.9, 0.8, 0.4];
        let r = ipcw_auc(&times, &status, &score, 1095.75, &opts()).unwrap();
        assert_eq!(r.n_cases, 1);
        assert_eq!(r.n_controls, 1);
        assert!((r.auc - 1.0).abs() < 1e-12);
    }

    #[test]
    fn weight_asymmetry_moves_the_estimate() {
        // One case (score .6) against a competing-death control (.4) and a
        // beyond-horizon control (.9): one concordant and one discordant
        // pair, so with no censoring AUC = 1/2.
        let times = [200.0, 400.0, 1500.0];
        let status = [1u8, 2, 0];
        let score = [0.6, 0.4, 0.9];
        let a = ipcw_auc(&times, &status, &score, 1095.75, &opts()).unwrap();
        assert!((a.auc - 0.5).abs() < 1e-12);

        // Add a censoring at 600d in a 2-subject risk set: the two controls
        // before/at the horizon now carry weight 2 while the case (event at
        // 200, before the jump) keeps weight 1. The discordant pair doubles,
        // the concordant pair does not: AUC = 1 / (1 + 2).
        let times4 = [200.0, 400.0, 600.0, 1500.0];
        let status4 = [1u8, 2, 0, 0];
        let score4 = [0.6, 0.4, 0.0, 0.9];
        let b = ipcw_auc(&times4, &status4, &score4, 1095.75, &opts()).unwrap();
        assert!((b.auc - 1.0 / 3.0).abs() < 1e-12);
    }
}
