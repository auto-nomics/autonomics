//! Survival analysis utilities — C-index, time bins, Brier score.

use serde::{Deserialize, Serialize};

// ═══════════════════════════════════════════════════════════════════════
// Harrell's C-index
// ═══════════════════════════════════════════════════════════════════════

/// Compute Harrell's concordance index.
///
/// A pair (i, j) is comparable if the shorter time has an event.
/// Concordant if the sample with shorter time has higher risk.
///
/// - `risk_scores`: predicted risk (higher = more risky).
/// - `times`: observed survival times.
/// - `events`: 1 if event occurred, 0 if censored.
pub fn c_index(risk_scores: &[f64], times: &[f64], events: &[usize]) -> f64 {
    let n = risk_scores.len();
    debug_assert_eq!(times.len(), n);
    debug_assert_eq!(events.len(), n);

    let mut concordant = 0.0;
    let mut permissible = 0.0;

    for i in 0..n {
        for j in (i + 1)..n {
            // Check if pair is comparable.
            let comparable = if times[i] != times[j] {
                // The one with shorter time must have an event.
                if times[i] < times[j] {
                    events[i] == 1
                } else {
                    events[j] == 1
                }
            } else {
                // Same time: comparable if both had events, or one had event.
                if events[i] == 1 && events[j] == 1 {
                    true
                } else if events[i] == 1 || events[j] == 1 {
                    true
                } else {
                    false
                }
            };

            if !comparable {
                continue;
            }

            permissible += 1.0;

            if times[i] < times[j] {
                // i should have higher risk.
                if risk_scores[i] > risk_scores[j] {
                    concordant += 1.0;
                } else if risk_scores[i] == risk_scores[j] {
                    concordant += 0.5;
                }
            } else if times[j] < times[i] {
                if risk_scores[j] > risk_scores[i] {
                    concordant += 1.0;
                } else if risk_scores[j] == risk_scores[i] {
                    concordant += 0.5;
                }
            } else {
                // Same time, both events: tie in risk → 0.5.
                if risk_scores[i] == risk_scores[j] {
                    concordant += 1.0; // treated as concordant
                } else {
                    concordant += 0.5;
                }
            }
        }
    }

    if permissible == 0.0 {
        0.5
    } else {
        concordant / permissible
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Time-dependent AUC (inverse probability weighting)
// ═══════════════════════════════════════════════════════════════════════

/// Compute time-dependent AUC at a given time point `t`.
///
/// Cases: event before `t`. Controls: event-free at `t`.
/// Uses the incident/dynamic AUC estimator.
pub fn td_auc(risk_scores: &[f64], times: &[f64], events: &[usize], t: f64) -> f64 {
    let n = risk_scores.len();

    // Cases: t_i <= t and e_i == 1.
    let cases: Vec<usize> = (0..n)
        .filter(|&i| times[i] <= t && events[i] == 1)
        .collect();
    // Controls: t_i > t.
    let controls: Vec<usize> = (0..n).filter(|&i| times[i] > t).collect();

    if cases.is_empty() || controls.is_empty() {
        return 0.5;
    }

    let mut concordant = 0.0;
    let mut total = 0.0;
    for &c in &cases {
        for &ctrl in &controls {
            total += 1.0;
            if risk_scores[c] > risk_scores[ctrl] {
                concordant += 1.0;
            } else if risk_scores[c] == risk_scores[ctrl] {
                concordant += 0.5;
            }
        }
    }

    concordant / total
}

// ═══════════════════════════════════════════════════════════════════════
// Brier score
// ═══════════════════════════════════════════════════════════════════════

/// Brier score at a single time point.
///
/// `predicted_probs`: P(T > t) for each sample (survival probability).
/// `times`, `events`: observed.
/// `t`: the time point.
/// Uses inverse probability of censoring weighting (IPCW) with KM estimate
/// of censoring distribution.
pub fn brier_score(
    predicted_probs: &[f64],
    times: &[f64],
    events: &[usize],
    t: f64,
) -> f64 {
    let n = times.len();

    // KM estimate of censoring distribution G(t) = P(C > t).
    let g_t = km_censoring(times, events, t);
    let g_t_safe = g_t.max(1e-8);

    // Also need G(t_i) for IPCW weights.
    let mut score = 0.0;
    let mut count = 0;

    for i in 0..n {
        let g_ti = km_censoring(times, events, times[i]).max(1e-8);
        let p = predicted_probs[i];

        if times[i] <= t && events[i] == 1 {
            // Case: event before t → (1 - S(t))^2 / G(t_i).
            score += (1.0 - p).powi(2) / g_ti;
            count += 1;
        } else if times[i] > t {
            // Control: event-free at t → S(t)^2 / G(t).
            score += p.powi(2) / g_t_safe;
            count += 1;
        }
        // If times[i] <= t and censored → not counted (handled by IPCW).
    }

    if count == 0 {
        return f64::NAN;
    }

    score / n as f64
}

/// Integrated Brier score over a range of time points.
pub fn integrated_brier_score(
    predicted_probs_matrix: &[Vec<f64>], // [sample][time_point_idx]
    times: &[f64],
    events: &[usize],
    eval_times: &[f64],
) -> f64 {
    if eval_times.is_empty() {
        return f64::NAN;
    }

    let mut total = 0.0;
    for (idx, &t) in eval_times.iter().enumerate() {
        let probs_at_t: Vec<f64> = predicted_probs_matrix.iter().map(|p| p[idx]).collect();
        total += brier_score(&probs_at_t, times, events, t);
    }

    let span = eval_times.last().unwrap() - eval_times.first().unwrap_or(&0.0);
    if span.abs() < 1e-10 {
        total / eval_times.len() as f64
    } else {
        total / (eval_times.len() as f64) * span / span.max(1e-10)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Time bins for discrete-time survival
// ═══════════════════════════════════════════════════════════════════════

/// Discrete time bins computed from observed event times.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeBins {
    /// Cut points (n_bins + 1 edges, including 0 and max).
    pub edges: Vec<f64>,
}

impl TimeBins {
    /// Create bins using quantile-based or uniform cut points.
    pub fn fit(times: &[f64], events: &[usize], n_bins: usize, method: &str) -> Self {
        // Only use event times for bin computation.
        let event_times: Vec<f64> = (0..times.len())
            .filter(|&i| events[i] == 1)
            .map(|i| times[i])
            .collect();

        let mut sorted = if event_times.is_empty() {
            times.to_vec()
        } else {
            event_times
        };
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let n = sorted.len();
        if n == 0 {
            return Self {
                edges: vec![0.0, 1.0],
            };
        }

        let t_min = sorted[0].max(0.0);
        let t_max = sorted[n - 1];

        let edges: Vec<f64> = match method {
            "quantile" => {
                let mut cuts = vec![0.0; n_bins + 1];
                cuts[0] = t_min;
                cuts[n_bins] = t_max;
                for k in 1..n_bins {
                    let quantile = k as f64 / n_bins as f64;
                    let idx = ((n as f64) * quantile).round() as usize;
                    let idx = idx.min(n - 1);
                    cuts[k] = sorted[idx];
                }
                cuts
            }
            _ => {
                // Uniform.
                let step = (t_max - t_min) / n_bins as f64;
                (0..=n_bins)
                    .map(|k| t_min + step * k as f64)
                    .collect()
            }
        };

        Self { edges }
    }

    /// Number of bins.
    pub fn n_bins(&self) -> usize {
        self.edges.len().saturating_sub(1)
    }

    /// Find the bin index for a given time value.
    /// Returns `None` if time is beyond the maximum edge.
    /// Times below the minimum edge are clamped to bin 0.
    pub fn bin_of(&self, t: f64) -> Option<usize> {
        let nb = self.n_bins();
        if nb == 0 {
            return None;
        }
        // Clamp below to first bin.
        if t < self.edges[0] {
            return Some(0);
        }
        for k in 0..nb {
            if t >= self.edges[k] && t < self.edges[k + 1] {
                return Some(k);
            }
        }
        // At or above the last edge → last bin.
        if t >= self.edges[nb] {
            return Some(nb - 1);
        }
        None
    }

    /// Midpoint of bin `k`.
    pub fn bin_midpoint(&self, k: usize) -> f64 {
        if k + 1 >= self.edges.len() {
            return self.edges[k];
        }
        (self.edges[k] + self.edges[k + 1]) / 2.0
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════

/// KM estimate of censoring distribution G(t) = P(C > t).
/// Treats events as the "censored" outcome and censoring as the "event".
fn km_censoring(times: &[f64], events: &[usize], t: f64) -> f64 {
    let n = times.len();
    if n == 0 {
        return 1.0;
    }

    // Collect unique times where censoring occurred.
    let mut censor_times: Vec<f64> = (0..n)
        .filter(|&i| events[i] == 0)
        .map(|i| times[i])
        .collect();
    censor_times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    censor_times.dedup_by(|a, b| (*a - *b).abs() < 1e-10);

    let mut g = 1.0;
    for &ct in &censor_times {
        if ct > t {
            break;
        }
        // Number at risk just before ct.
        let at_risk = times.iter().filter(|&&x| x >= ct - 1e-10).count();
        // Number censored at ct.
        let n_censored = (0..n)
            .filter(|&i| events[i] == 0 && (times[i] - ct).abs() < 1e-10)
            .count();
        if at_risk > 0 {
            g *= 1.0 - n_censored as f64 / at_risk as f64;
        }
    }

    g
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cindex_perfect() {
        // Higher risk → shorter time. Perfect concordance.
        let risk = vec![3.0, 2.0, 1.0];
        let times = vec![1.0, 2.0, 3.0];
        let events = vec![1, 1, 1];
        let c = c_index(&risk, &times, &events);
        assert!((c - 1.0).abs() < 1e-10, "c = {c}");
    }

    #[test]
    fn test_cindex_anti() {
        // Higher risk → longer time. Perfect anti-concordance.
        let risk = vec![1.0, 2.0, 3.0];
        let times = vec![1.0, 2.0, 3.0];
        let events = vec![1, 1, 1];
        let c = c_index(&risk, &times, &events);
        assert!((c - 0.0).abs() < 1e-10, "c = {c}");
    }

    #[test]
    fn test_cindex_random() {
        let risk = vec![1.0, 1.0, 1.0];
        let times = vec![1.0, 2.0, 3.0];
        let events = vec![1, 1, 1];
        let c = c_index(&risk, &times, &events);
        assert!((c - 0.5).abs() < 1e-10, "c = {c}");
    }

    #[test]
    fn test_cindex_with_censoring() {
        let risk = vec![2.0, 1.0, 0.5];
        let times = vec![1.0, 5.0, 3.0];
        let events = vec![1, 0, 1];
        let c = c_index(&risk, &times, &events);
        // Pairs: (0,2): comparable (t0<t2, e0=1), risk[0]>risk[2] → concordant.
        // (0,1): comparable (t0<t1, e0=1), risk[0]>risk[1] → concordant.
        // (1,2): t2<t1 but e2=1 → comparable, risk[2]<risk[1] → anti → 0.
        // (0,2)+(0,1) = 2 concordant, (2,1) = 1 anti. Total 3 permissible.
        // c = 2/3.
        assert!((c - 2.0 / 3.0).abs() < 1e-10, "c = {c}");
    }

    #[test]
    fn test_td_auc_basic() {
        let risk = vec![3.0, 1.0, 2.0, 0.5];
        let times = vec![1.0, 5.0, 2.0, 6.0];
        let events = vec![1, 0, 1, 0];
        // At t=3: cases = {0 (t=1,e=1), 2 (t=2,e=1)}, controls = {1 (t=5), 3 (t=6)}.
        // Case 0 risk=3 > both controls → 2 concordant.
        // Case 2 risk=2 > both controls → 2 concordant.
        // Total 4 pairs, all concordant → AUC = 1.0.
        let auc = td_auc(&risk, &times, &events, 3.0);
        assert!((auc - 1.0).abs() < 1e-10, "auc = {auc}");
    }

    #[test]
    fn test_time_bins_quantile() {
        let times = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let events = vec![1; 8];
        let bins = TimeBins::fit(&times, &events, 4, "quantile");
        assert_eq!(bins.n_bins(), 4);
        assert!(bins.bin_of(0.5).is_some());
        assert!(bins.bin_of(4.5).is_some());
    }

    #[test]
    fn test_time_bins_uniform() {
        let times = vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0];
        let events = vec![1; 6];
        let bins = TimeBins::fit(&times, &events, 5, "uniform");
        assert_eq!(bins.n_bins(), 5);
        // Edges should be 0, 1, 2, 3, 4, 5.
        assert!((bins.edges[0] - 0.0).abs() < 1e-10);
        assert!((bins.edges[5] - 5.0).abs() < 1e-10);
    }
}
