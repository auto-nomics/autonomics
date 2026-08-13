//! Survival analysis utilities — C-index, time bins, Brier score.
//!
//! Pure Rust (no Burn dependency).

use serde::{Deserialize, Serialize};

// ═══════════════════════════════════════════════════════════════════════
// Harrell's C-index
// ═══════════════════════════════════════════════════════════════════════

/// Compute Harrell's concordance index.
pub fn c_index(risk_scores: &[f64], times: &[f64], events: &[usize]) -> f64 {
    let n = risk_scores.len();
    debug_assert_eq!(times.len(), n);
    debug_assert_eq!(events.len(), n);

    let mut concordant = 0.0;
    let mut permissible = 0.0;

    for i in 0..n {
        for j in (i + 1)..n {
            let comparable = if times[i] != times[j] {
                if times[i] < times[j] {
                    events[i] == 1
                } else {
                    events[j] == 1
                }
            } else if events[i] == 1 && events[j] == 1 {
                true
            } else if events[i] == 1 || events[j] == 1 {
                true
            } else {
                false
            };

            if !comparable {
                continue;
            }

            permissible += 1.0;

            if times[i] < times[j] {
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
                if risk_scores[i] == risk_scores[j] {
                    concordant += 1.0;
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
// Time-dependent AUC
// ═══════════════════════════════════════════════════════════════════════

pub fn td_auc(risk_scores: &[f64], times: &[f64], events: &[usize], t: f64) -> f64 {
    let n = risk_scores.len();

    let cases: Vec<usize> = (0..n)
        .filter(|&i| times[i] <= t && events[i] == 1)
        .collect();
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

pub fn brier_score(predicted_probs: &[f64], times: &[f64], events: &[usize], t: f64) -> f64 {
    let n = times.len();
    let g_t = km_censoring(times, events, t);
    let g_t_safe = g_t.max(1e-8);

    let mut score = 0.0;
    let mut count = 0;

    for i in 0..n {
        let g_ti = km_censoring(times, events, times[i]).max(1e-8);
        let p = predicted_probs[i];

        if times[i] <= t && events[i] == 1 {
            score += (1.0 - p).powi(2) / g_ti;
            count += 1;
        } else if times[i] > t {
            score += p.powi(2) / g_t_safe;
            count += 1;
        }
    }

    if count == 0 {
        return f64::NAN;
    }
    score / n as f64
}

// ═══════════════════════════════════════════════════════════════════════
// Time bins
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeBins {
    pub edges: Vec<f64>,
}

impl TimeBins {
    pub fn fit(times: &[f64], events: &[usize], n_bins: usize, method: &str) -> Self {
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
                let step = (t_max - t_min) / n_bins as f64;
                (0..=n_bins).map(|k| t_min + step * k as f64).collect()
            }
        };

        Self { edges }
    }

    pub fn n_bins(&self) -> usize {
        self.edges.len().saturating_sub(1)
    }

    pub fn bin_of(&self, t: f64) -> Option<usize> {
        let nb = self.n_bins();
        if nb == 0 {
            return None;
        }
        if t < self.edges[0] {
            return Some(0);
        }
        for k in 0..nb {
            if t >= self.edges[k] && t < self.edges[k + 1] {
                return Some(k);
            }
        }
        if t >= self.edges[nb] {
            return Some(nb - 1);
        }
        None
    }

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

fn km_censoring(times: &[f64], events: &[usize], t: f64) -> f64 {
    let n = times.len();
    if n == 0 {
        return 1.0;
    }

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
        let at_risk = times.iter().filter(|&&x| x >= ct - 1e-10).count();
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
        let risk = vec![3.0, 2.0, 1.0];
        let times = vec![1.0, 2.0, 3.0];
        let events = vec![1, 1, 1];
        let c = c_index(&risk, &times, &events);
        assert!((c - 1.0).abs() < 1e-10, "c = {c}");
    }

    #[test]
    fn test_cindex_anti() {
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
    fn test_td_auc_basic() {
        let risk = vec![3.0, 1.0, 2.0, 0.5];
        let times = vec![1.0, 5.0, 2.0, 6.0];
        let events = vec![1, 0, 1, 0];
        let auc = td_auc(&risk, &times, &events, 3.0);
        assert!((auc - 1.0).abs() < 1e-10, "auc = {auc}");
    }

    #[test]
    fn test_time_bins_uniform() {
        let times = vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0];
        let events = vec![1; 6];
        let bins = TimeBins::fit(&times, &events, 5, "uniform");
        assert_eq!(bins.n_bins(), 5);
        assert!((bins.edges[0] - 0.0).abs() < 1e-10);
        assert!((bins.edges[5] - 5.0).abs() < 1e-10);
    }
}
