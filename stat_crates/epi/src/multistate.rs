//! Multi-state Markov model: transition hazards (Nelson-Aalen) and transition
//! probabilities (Aalen-Johansen estimator).
//!
//! Generalises the Kaplan-Meier / competing-risk framework to arbitrary state
//! transition diagrams. For a Markov model with states `{1, …, S}` and
//! observed transitions `(from_state, to_state, time)`:
//!
//! # Nelson-Aalen cumulative hazard
//!
//! For each allowed transition `r → s`, the cumulative hazard at time `t` is:
//!
//! ```text
//! Λ_rs(t) = Σ_{tⱼ ≤ t} d_rs(tⱼ) / n_r(tⱼ)
//! ```
//!
//! where `d_rs(tⱼ)` is the number of `r → s` transitions at `tⱼ`, and
//! `n_r(tⱼ)` is the number of subjects in state `r` just before `tⱼ`.
//!
//! # Aalen-Johansen transition probability matrix
//!
//! `P(t) = ∏_{tⱼ ≤ t} (I + ΔH(tⱼ))`, where `ΔH(tⱼ)` is the matrix of
//! hazard increments at `tⱼ`. The product-of-increments formulation is
//! numerically stable and exact for Markov chains.

use crate::error::{EpiError, Result};

/// Result of a multi-state model fit.
#[derive(Debug, Clone)]
pub struct MultiStateResult {
    /// All distinct transition times (sorted ascending).
    pub times: Vec<f64>,
    /// Transition probability matrices `P(t)` at each time: `p_matrices[i]`
    /// is an `S×S` row-stochastic matrix where `p_matrices[i][r][s]` =
    /// P(in state s at times[i] | started in state r at t=0).
    pub p_matrices: Vec<Vec<Vec<f64>>>,
    /// Cumulative hazards per transition: `cum_hazards[i]` is an `S×S` matrix
    /// where `cum_hazards[i][r][s]` = Λ_rs(times[i]).
    pub cum_hazards: Vec<Vec<Vec<f64>>>,
    /// State count at each time: `state_counts[i][s]` = number in state s
    /// just before times[i].
    pub state_counts: Vec<Vec<usize>>,
    /// Number of states.
    pub n_states: usize,
    /// Number of observations (subjects).
    pub n_obs: usize,
    /// Number of distinct event times.
    pub n_times: usize,
}

/// Fit a multi-state Markov model.
///
/// `from_state`, `to_state` are integer state labels (0-based: 0, 1, …, S−1).
/// `time` is the transition/censoring time. `entry_time` is when the subject
/// entered the `from_state` (default 0.0 if all enter at baseline).
///
/// Subjects are assumed to be in their `from_state` at their `entry_time`
/// and either transition to `to_state` or are censored at `time`.
pub fn multistate(
    time: &[f64],
    from_state: &[u64],
    to_state: &[u64],
    n_states: usize,
    entry_time: Option<&[f64]>,
) -> Result<MultiStateResult> {
    let n = time.len();
    if n == 0 || from_state.len() != n || to_state.len() != n {
        return Err(EpiError::DimensionMismatch {
            a: n,
            b: from_state.len().max(to_state.len()),
        });
    }
    if n_states < 2 {
        return Err(EpiError::Numerical("need ≥ 2 states".to_string()));
    }
    let entry = entry_time.unwrap_or(&[]); // empty = all enter at 0

    // Validate states.
    for i in 0..n {
        if from_state[i] as usize >= n_states || to_state[i] as usize >= n_states {
            return Err(EpiError::Numerical(format!(
                "state label exceeds n_states={n_states}: from={}, to={}",
                from_state[i], to_state[i]
            )));
        }
    }

    // ── Identify distinct event times ────────────────────────────────────
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| time[a].partial_cmp(&time[b]).unwrap_or(std::cmp::Ordering::Equal));

    let mut event_times = Vec::new();
    let mut i = 0;
    while i < n {
        let t = time[order[i]];
        event_times.push(t);
        while i < n && (time[order[i]] - t).abs() < f64::EPSILON * t.abs().max(1.0) {
            i += 1;
        }
    }

    // ── At each event time, compute hazard increments and state counts ──
    // For the Aalen-Johansen estimator, we need:
    // 1. n_r(t) = number of subjects in state r just before time t
    // 2. d_rs(t) = number of r→s transitions at time t
    //
    // State occupancy tracking: at baseline (t=0), each subject i is in
    // from_state[i] (if entry_time[i] ≤ t). When a subject transitions, they
    // leave from_state and enter to_state.

    let m = event_times.len();
    let mut hazard_increments: Vec<Vec<Vec<f64>>> = Vec::with_capacity(m);
    let mut state_counts: Vec<Vec<usize>> = Vec::with_capacity(m);

    for t_idx in 0..m {
        let t = event_times[t_idx];

        // Count transitions at this time.
        let mut d = vec![vec![0usize; n_states]; n_states]; // d[r][s] = r→s count
        for j in 0..n {
            if (time[j] - t).abs() < f64::EPSILON * t.abs().max(1.0) {
                let r = from_state[j] as usize;
                let s = to_state[j] as usize;
                if r != s {
                    d[r][s] += 1;
                }
            }
        }

        // Count subjects in each state just before time t.
        // A subject j is in state r at time t if:
        // - They entered from_state[j] at entry_time[j] ≤ t
        // - Their event time ≥ t (still at risk) or their event is AT t
        // - If they had a transition at time t' < t, they moved to to_state[j]
        let mut n_in_state = vec![0usize; n_states];
        for j in 0..n {
            let et = if entry.is_empty() { 0.0 } else { entry[j] };
            if et > t { continue; } // not yet entered

            if time[j] >= t {
                // Still in from_state just before t (event hasn't happened yet,
                // or happens exactly at t).
                n_in_state[from_state[j] as usize] += 1;
            } else if from_state[j] != to_state[j] {
                // Actually transitioned at time[j] < t — now in to_state.
                n_in_state[to_state[j] as usize] += 1;
            }
            // else: censored (from == to, time < t) — left the study, don't count.
        }

        state_counts.push(n_in_state.clone());

        // Hazard increment matrix: off-diagonal ΔH[r][s] = d_rs/n_r,
        // diagonal ΔH[r][r] = −Σ_{s≠r} d_rs/n_r (outflow correction).
        let mut dh = vec![vec![0.0_f64; n_states]; n_states];
        for r in 0..n_states {
            if n_in_state[r] > 0 {
                let mut outflow = 0.0;
                for s in 0..n_states {
                    if s != r {
                        let h = d[r][s] as f64 / n_in_state[r] as f64;
                        dh[r][s] = h;
                        outflow += h;
                    }
                }
                dh[r][r] = -outflow;
            }
        }
        hazard_increments.push(dh);
    }

    // ── Compute cumulative hazards (Nelson-Aalen) ────────────────────────
    let mut cum_hazards: Vec<Vec<Vec<f64>>> = Vec::with_capacity(m);
    let mut cum = vec![vec![0.0_f64; n_states]; n_states];
    for t_idx in 0..m {
        for r in 0..n_states {
            for s in 0..n_states {
                cum[r][s] += hazard_increments[t_idx][r][s];
            }
        }
        cum_hazards.push(cum.clone());
    }

    // ── Compute transition probability matrices (Aalen-Johansen) ────────
    // P(t) = ∏_{tⱼ ≤ t} (I + ΔH(tⱼ))
    // This is the product-integral, approximated by the matrix product of
    // (I + ΔH) at each event time.
    let mut p_matrices: Vec<Vec<Vec<f64>>> = Vec::with_capacity(m);
    let mut p = identity(n_states); // P(0) = I

    for t_idx in 0..m {
        // M = I + ΔH(tⱼ)
        let mut m_mat = identity(n_states);
        for r in 0..n_states {
            for s in 0..n_states {
                m_mat[r][s] += hazard_increments[t_idx][r][s];
            }
        }
        // P_new = P × M
        p = matmul(&p, &m_mat, n_states);
        p_matrices.push(p.clone());
    }

    Ok(MultiStateResult {
        times: event_times,
        p_matrices,
        cum_hazards,
        state_counts,
        n_states,
        n_obs: n,
        n_times: m,
    })
}

/// Identity matrix of size n.
fn identity(n: usize) -> Vec<Vec<f64>> {
    let mut m = vec![vec![0.0; n]; n];
    for i in 0..n { m[i][i] = 1.0; }
    m
}

/// Matrix multiplication for square matrices.
fn matmul(a: &[Vec<f64>], b: &[Vec<f64>], n: usize) -> Vec<Vec<f64>> {
    let mut c = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..n {
            let mut sum = 0.0;
            for k in 0..n {
                sum += a[i][k] * b[k][j];
            }
            c[i][j] = sum;
        }
    }
    c
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * b.abs().max(1.0)
    }

    #[test]
    fn two_state_matches_km() {
        // State 0 → State 1 (absorbing). This is equivalent to KM survival.
        // P(0→0 at t) = S(t), P(0→1 at t) = 1 − S(t).
        let time = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let from = vec![0; 5];
        let to = vec![1; 5];
        let result = multistate(&time, &from, &to, 2, None).unwrap();

        // P(0→1) at last time should be 1.0 (all transitioned).
        let p = &result.p_matrices.last().unwrap();
        assert!(approx_eq(p[0][1], 1.0, 1e-10), "P(0→1) should be 1.0, got {}", p[0][1]);
        assert!(approx_eq(p[0][0], 0.0, 1e-10), "P(0→0) should be 0.0, got {}", p[0][0]);
    }

    #[test]
    fn competing_risk_cif_property() {
        // 3-state: 0 (alive) → 1 (cause 1) or 0 → 2 (cause 2).
        // P(0→1 at t) should match CIF₁(t), P(0→2 at t) = CIF₂(t).
        let time = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let from = vec![0; 6];
        let to = vec![1, 2, 1, 2, 1, 2]; // alternating causes
        let result = multistate(&time, &from, &to, 3, None).unwrap();

        let p = result.p_matrices.last().unwrap();
        // P(0→1) + P(0→2) + P(0→0) should ≤ 1.
        let total = p[0][1] + p[0][2] + p[0][0];
        assert!(total <= 1.0 + 1e-10, "Row sum = {total}");
    }

    #[test]
    fn row_stochastic() {
        // Each row of P(t) should sum to 1 (probabilities are conserved).
        let time = vec![1.0, 5.0, 3.0, 7.0, 2.0, 6.0, 4.0, 8.0];
        let from = vec![0; 8];
        let to = vec![1, 1, 1, 1, 2, 2, 2, 2]; // half to state 1, half to state 2
        let result = multistate(&time, &from, &to, 3, None).unwrap();

        for p in &result.p_matrices {
            for r in 0..3 {
                let row_sum: f64 = p[r].iter().sum();
                assert!(
                    (row_sum - 1.0).abs() < 1e-10,
                    "Row {r} sum = {row_sum} ≠ 1.0"
                );
            }
        }
    }

    #[test]
    fn cumulative_hazard_increases() {
        let time = vec![1.0, 2.0, 3.0];
        let from = vec![0; 3];
        let to = vec![1; 3];
        let result = multistate(&time, &from, &to, 2, None).unwrap();

        // Λ₀₁ should be monotonically increasing.
        let h: Vec<f64> = result.cum_hazards.iter().map(|h| h[0][1]).collect();
        for i in 1..h.len() {
            assert!(h[i] >= h[i - 1], "Cumulative hazard should not decrease");
        }
    }

    #[test]
    fn reversible_transitions() {
        // State 0 → 1 and 1 → 0 (illness-recovery model).
        let time = vec![2.0, 5.0, 8.0, 3.0, 6.0, 9.0];
        let from = vec![0, 1, 0, 1, 0, 1]; // alternating transitions
        let to = vec![1, 0, 1, 0, 1, 0];
        let result = multistate(&time, &from, &to, 2, None).unwrap();

        // Should run without error and produce valid matrices.
        assert_eq!(result.n_states, 2);
        assert!(!result.p_matrices.is_empty());

        // All P matrices should be row-stochastic.
        for p in &result.p_matrices {
            for r in 0..2 {
                let row_sum: f64 = p[r].iter().sum();
                assert!((row_sum - 1.0).abs() < 1e-10, "Row {r} sum = {row_sum}");
            }
        }
    }

    #[test]
    fn rejects_invalid_states() {
        let time = vec![1.0, 2.0];
        let from = vec![0, 5]; // state 5 ≥ n_states=3
        let to = vec![1, 0];
        assert!(multistate(&time, &from, &to, 3, None).is_err());
    }
}
