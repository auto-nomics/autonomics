//! Kaplan–Meier estimate of the **censoring** distribution for use as
//! inverse-probability-of-censoring weights (IPCW).
//!
//! For the Brier score, AUC and calibration of a cause-1 cumulative-incidence
//! prediction at a horizon `t*`, observations need weighting by the survival
//! function of censoring, `G`. Following the convention used by
//! `riskRegression` (Blanche, Saarela & Scheike) and Schoop et al. (2011):
//!
//! * an **event** observed at `U_i` (status 1 or 2) is weighted by
//!   `1 / G(U_i-)` — censoring is assumed to occur infinitesimally *after*
//!   events sharing its time, so the left-continuous evaluation excludes any
//!   censoring jump at `U_i` itself;
//! * an observation still free of the event **at the horizon** (`U_i > t*`) is
//!   weighted by `1 / G(t*)` — right-continuous evaluation, which includes
//!   censoring jumps at times `≤ t*` but does not force the artificial drop
//!   to zero that `cmprsk` applies beyond the last observed time.
//!
//! [`ReverseKmCensoring`] therefore exposes both evaluations explicitly, and
//! callers must pick the one their formula needs rather than guessing.

use crate::error::{CrrkitError, Result};

/// Reverse Kaplan–Meier estimate of the censoring survival `G`, stored as a
/// right-continuous step function over distinct observed times.
///
/// Censoring (`status == 0`) is the *event* of this fit; statuses 1 and 2 are
/// treated as censoring-free survival through their time.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReverseKmCensoring {
    /// Distinct observed times, ascending.
    #[serde(default)]
    pub times: Vec<f64>,
    /// `G(t_k)` — the post-jump censoring survival at each distinct time.
    #[serde(default)]
    pub surv: Vec<f64>,
}

impl ReverseKmCensoring {
    /// Estimate `G` from observed times and event statuses.
    ///
    /// Times need not be sorted (they are sorted internally); ties between
    /// events and censorings are resolved in favour of censoring happening
    /// last, which is what the left-continuous [`Self::eval_left`] evaluation
    /// encodes.
    pub fn new(times: &[f64], status: &[u8]) -> Result<Self> {
        if times.len() != status.len() {
            return Err(CrrkitError::LengthMismatch {
                what: "status",
                got: status.len(),
                expected: times.len(),
            });
        }
        for (i, &t) in times.iter().enumerate() {
            if !t.is_finite() || t < 0.0 {
                return Err(CrrkitError::BadTime { index: i, got: t });
            }
        }

        let mut order: Vec<usize> = (0..times.len()).collect();
        order.sort_by(|&a, &b| times[a].total_cmp(&times[b]));

        let mut knots: Vec<f64> = Vec::new();
        let mut surv: Vec<f64> = Vec::new();
        let mut s = 1.0_f64;
        let n = times.len() as f64;
        let mut n_risk = n;

        let mut i = 0usize;
        while i < order.len() {
            let t = times[order[i]];
            let mut j = i;
            let mut d = 0.0_f64; // censorings at this time
            while j < order.len() && times[order[j]] == t {
                if status[order[j]] == 0 {
                    d += 1.0;
                }
                j += 1;
            }
            let n_at = (j - i) as f64;
            if d > 0.0 {
                s *= (n_risk - d) / n_risk;
            }
            knots.push(t);
            surv.push(s);
            n_risk -= n_at;
            i = j;
        }

        Ok(Self { times: knots, surv })
    }

    /// `G(t)` — right-continuous evaluation: the censoring survival after all
    /// jumps at times `≤ t`. Equals 1 before the first observed time and
    /// holds its final value beyond the last one (no artificial drop).
    pub fn eval(&self, t: f64) -> f64 {
        match self.times.partition_point(|&x| x <= t) {
            0 => 1.0,
            k => self.surv[k - 1],
        }
    }

    /// `G(t-)` — left-continuous evaluation: the censoring survival just
    /// before `t`, excluding any censoring jump at `t` itself. This is the
    /// weight denominator for events observed at `t`.
    pub fn eval_left(&self, t: f64) -> f64 {
        match self.times.partition_point(|&x| x < t) {
            0 => 1.0,
            k => self.surv[k - 1],
        }
    }

    /// Smallest censoring survival attained at or before `t` — used to detect
    /// weight explosion near the horizon.
    pub fn min_upto(&self, t: f64) -> f64 {
        match self.times.partition_point(|&x| x <= t) {
            0 => 1.0,
            k => self.surv[..k].iter().cloned().fold(1.0_f64, f64::min),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_censoring_keeps_g_at_one() {
        let g = ReverseKmCensoring::new(&[1.0, 2.0, 3.0], &[1, 2, 1]).unwrap();
        assert_eq!(g.eval(0.0), 1.0);
        assert_eq!(g.eval(2.0), 1.0);
        assert_eq!(g.eval_left(3.0), 1.0);
        assert_eq!(g.min_upto(3.0), 1.0);
    }

    #[test]
    fn censoring_jumps_match_hand_computation() {
        // Censorings at 100 and 300; events at 200 (cause 1) and 400 (cause 2).
        // Risk sets: 100 -> 4, 300 -> 2.
        // G(100) = 3/4, G(300) = 3/4 * 1/2 = 3/8, holds thereafter.
        let g = ReverseKmCensoring::new(&[400.0, 100.0, 300.0, 200.0], &[2, 0, 0, 1]).unwrap();
        assert!((g.eval(100.0) - 0.75).abs() < 1e-12);
        assert!((g.eval_left(200.0) - 0.75).abs() < 1e-12); // no jump at 200
        assert!((g.eval(300.0) - 0.375).abs() < 1e-12);
        assert!((g.eval_left(400.0) - 0.375).abs() < 1e-12);
        assert!((g.eval(500.0) - 0.375).abs() < 1e-12); // held, not dropped
    }

    #[test]
    fn left_evaluation_excludes_same_time_jump() {
        // An event censored at the same instant: the event weight must use the
        // pre-jump value.
        let g = ReverseKmCensoring::new(&[100.0, 100.0], &[0, 1]).unwrap();
        assert!((g.eval_left(100.0) - 1.0).abs() < 1e-12);
        assert!((g.eval(100.0) - 0.5).abs() < 1e-12);
    }
}
