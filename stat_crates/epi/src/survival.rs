//! Survival analysis: Kaplan-Meier estimator and log-rank test.
//!
//! # Kaplan-Meier
//!
//! Non-parametric estimation of the survival function:
//!
//! ```text
//! Ŝ(t) = ∏_{tᵢ ≤ t} (1 − dᵢ / nᵢ)
//! ```
//!
//! where `dᵢ` is the number of events at time `tᵢ` and `nᵢ` the number at
//! risk. Standard errors use Greenwood's formula:
//!
//! ```text
//! Var(log Ŝ(t)) ≈ Σ_{tᵢ ≤ t} dᵢ / (nᵢ · (nᵢ − dᵢ))
//! ```
//!
//! # Log-rank test
//!
//! Mantel-Cox (a.k.a. log-rank) test comparing survival curves across K
//! groups. At each pooled event time `t`, build a K×2 contingency table
//! (events vs at-risk), compute expected events `E_g` and the covariance
//! matrix `V`. The test statistic `(O − E)′ V⁻¹ (O − E) ~ χ²(K−1)`.

use statrs::distribution::{ChiSquared, ContinuousCDF};

use crate::error::{EpiError, Result};

// ── Kaplan-Meier ───────────────────────────────────────────────────────────

/// Result of a Kaplan-Meier estimation.
#[derive(Debug, Clone)]
pub struct KmResult {
    /// Event times (sorted ascending, only times with ≥ 1 event).
    pub times: Vec<f64>,
    /// Survival probability `Ŝ(t)` at each event time.
    pub survival: Vec<f64>,
    /// Greenwood standard error of `Ŝ(t)`.
    pub std_error: Vec<f64>,
    /// Number at risk just before each event time.
    pub n_at_risk: Vec<usize>,
    /// Number of events at each event time.
    pub n_events: Vec<usize>,
    /// Total censored observations.
    pub n_censored: usize,
    /// Total observations.
    pub n_obs: usize,
}

/// Compute the Kaplan-Meier survival estimator.
///
/// `time` is the observed survival time; `event` is 1 for events, 0 for
/// censored.
pub fn kaplan_meier(time: &[f64], event: &[f64]) -> Result<KmResult> {
    let n = time.len();
    if n == 0 {
        return Err(EpiError::EmptyInput);
    }
    if event.len() != n {
        return Err(EpiError::DimensionMismatch {
            a: n,
            b: event.len(),
        });
    }
    for &e in event {
        if e != 0.0 && e != 1.0 {
            return Err(EpiError::Numerical(format!(
                "event must be 0 or 1, got {e}"
            )));
        }
    }

    // Sort indices by time ascending.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        time[a]
            .partial_cmp(&time[b])
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Identify distinct event times and compute n_at_risk / n_events.
    let mut times = Vec::new();
    let mut n_at_risk_arr = Vec::new();
    let mut n_events_arr = Vec::new();

    let n_censored = event.iter().filter(|&&e| e == 0.0).count();

    // Process from start; at each distinct time, count events and at-risk.
    let mut i = 0;
    while i < n {
        let t = time[order[i]];
        let mut d = 0usize; // events at this time
        let mut group_end = i;
        while group_end < n && (time[order[group_end]] - t).abs() < f64::EPSILON * t.abs().max(1.0)
        {
            if event[order[group_end]] == 1.0 {
                d += 1;
            }
            group_end += 1;
        }
        // n_at_risk = everyone at or after this time = n - i
        let n_risk = n - i;
        if d > 0 {
            times.push(t);
            n_at_risk_arr.push(n_risk);
            n_events_arr.push(d);
        }
        i = group_end;
    }

    // Compute survival and Greenwood SE.
    let m = times.len();
    let mut survival = Vec::with_capacity(m);
    let mut std_errors = Vec::with_capacity(m);
    let mut s = 1.0_f64;
    let mut cum_var = 0.0_f64;

    for k in 0..m {
        let n_k = n_at_risk_arr[k] as f64;
        let d_k = n_events_arr[k] as f64;
        s *= 1.0 - d_k / n_k;
        survival.push(s);
        if n_k > d_k {
            cum_var += d_k / (n_k * (n_k - d_k));
        }
        let se = cum_var.sqrt();
        std_errors.push(if se.is_finite() { se } else { f64::NAN });
    }

    Ok(KmResult {
        times,
        survival,
        std_error: std_errors,
        n_at_risk: n_at_risk_arr,
        n_events: n_events_arr,
        n_censored,
        n_obs: n,
    })
}

// ── Log-rank test ──────────────────────────────────────────────────────────

/// Result of a log-rank (Mantel-Cox) test.
#[derive(Debug, Clone)]
pub struct LogRankResult {
    /// χ² test statistic.
    pub chi_squared: f64,
    /// Degrees of freedom (K−1).
    pub df: usize,
    /// p-value from the χ² distribution.
    pub p_value: f64,
    /// Number of groups.
    pub n_groups: usize,
    /// Observed events in each group.
    pub observed: Vec<f64>,
    /// Expected events in each group.
    pub expected: Vec<f64>,
    /// O − E for each group.
    pub o_minus_e: Vec<f64>,
}

/// Mantel-Cox log-rank test comparing survival curves across K groups.
///
/// `time`, `event` are pooled survival data; `group` is a group label (0, 1,
/// …, K−1) for each observation. Groups need not be contiguous.
pub fn log_rank_test(time: &[f64], event: &[f64], group: &[u64]) -> Result<LogRankResult> {
    let n = time.len();
    if n == 0 {
        return Err(EpiError::EmptyInput);
    }
    if event.len() != n || group.len() != n {
        return Err(EpiError::DimensionMismatch {
            a: n,
            b: event.len().max(group.len()),
        });
    }

    let k = *group.iter().max().unwrap_or(&0) as usize + 1;
    if k < 2 {
        return Err(EpiError::Numerical(
            "log-rank test requires ≥ 2 groups".to_string(),
        ));
    }

    // Sort by time ascending.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        time[a]
            .partial_cmp(&time[b])
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Accumulate O_g, E_g, and V (k×k covariance of O−E).
    let mut observed = vec![0.0_f64; k];
    let mut expected = vec![0.0_f64; k];
    let mut variance = vec![vec![0.0_f64; k]; k]; // (k×k) covariance matrix

    let mut i = 0;
    while i < n {
        let t = time[order[i]];

        // Collect all observations at time t.
        let mut group_end = i;
        while group_end < n && (time[order[group_end]] - t).abs() < f64::EPSILON * t.abs().max(1.0)
        {
            group_end += 1;
        }

        // Events per group at time t.
        let mut d_events = vec![0usize; k];
        for &idx in &order[i..group_end] {
            if event[idx] == 1.0 {
                d_events[group[idx] as usize] += 1;
            }
        }

        // At-risk per group: ALL subjects with time ≥ t (O(n) scan).
        let n_at_risk: Vec<usize> = (0..k)
            .map(|g| {
                (0..n)
                    .filter(|&j| time[j] >= t && group[j] as usize == g)
                    .count()
            })
            .collect();

        let n_total: usize = n_at_risk.iter().sum();
        let d_total: usize = d_events.iter().sum();

        if d_total > 0 {
            for g in 0..k {
                // Observed: always count.
                observed[g] += d_events[g] as f64;

                if n_total > 0 {
                    // Expected: E_g = d_total * n_g / N.
                    let e_g = d_total as f64 * n_at_risk[g] as f64 / n_total as f64;
                    expected[g] += e_g;

                    // Variance/covariance only when n > 1 (needs n−1 denominator).
                    if n_total > 1 {
                        let factor =
                            d_total as f64 * (n_total - d_total) as f64 / ((n_total - 1) as f64);
                        for h in 0..k {
                            let delta = if g == h { 1.0 } else { 0.0 };
                            let cov_gh = factor
                                * (delta * n_at_risk[g] as f64 / n_total as f64
                                    - n_at_risk[g] as f64 * n_at_risk[h] as f64
                                        / (n_total as f64).powi(2));
                            variance[g][h] += cov_gh;
                        }
                    }
                }
            }
        }

        i = group_end;
    }

    // Compute test statistic: (O − E)' V⁻¹ (O − E).
    // For 2 groups, V is 1×1 and the formula simplifies.
    let ome: Vec<f64> = (0..k).map(|g| observed[g] - expected[g]).collect();

    // Use the (K−1) × (K−1) submatrix (last group is dependent).
    // Actually, the standard approach is to use the full (O−E) vector with
    // the generalized inverse. For simplicity, we use the first (K−1) groups.
    let kk = k - 1;
    let v_sub: Vec<Vec<f64>> = (0..kk)
        .map(|g| (0..kk).map(|h| variance[g][h]).collect())
        .collect();
    let ome_sub: Vec<f64> = (0..kk).map(|g| ome[g]).collect();

    // Solve (O−E)' V⁻¹ (O−E) via Cholesky.
    // For k=2, this is just ome[0]² / variance[0][0].
    let chi2 = if kk == 1 {
        if variance[0][0] > 0.0 {
            ome_sub[0].powi(2) / variance[0][0]
        } else {
            0.0
        }
    } else {
        // Manual matrix solve for (K−1)×(K−1) system.
        // chi² = (O−E)' V⁻¹ (O−E)
        // Solve V·x = (O−E), then chi² = (O−E)'·x
        let v_inv = invert_matrix(&v_sub)?;
        let mut chi2_val = 0.0;
        for g in 0..kk {
            let mut x_g = 0.0;
            for h in 0..kk {
                x_g += v_inv[g][h] * ome_sub[h];
            }
            chi2_val += ome_sub[g] * x_g;
        }
        chi2_val
    };

    let df = kk;
    let p_value = if df > 0 {
        let dist = ChiSquared::new(df as f64)
            .map_err(|e| EpiError::Numerical(format!("ChiSquared: {e}")))?;
        1.0 - dist.cdf(chi2)
    } else {
        f64::NAN
    };

    Ok(LogRankResult {
        chi_squared: chi2,
        df,
        p_value,
        n_groups: k,
        observed,
        expected,
        o_minus_e: ome,
    })
}

/// Invert a small matrix via Gauss-Jordan elimination.
fn invert_matrix(a: &[Vec<f64>]) -> Result<Vec<Vec<f64>>> {
    let n = a.len();
    let mut m: Vec<Vec<f64>> = (0..n)
        .map(|i| {
            let mut row = a[i].clone();
            row.extend((0..n).map(|j| if i == j { 1.0 } else { 0.0 }));
            row
        })
        .collect();
    const TINY: f64 = 1.0e-300;

    for col in 0..n {
        let mut piv = col;
        for r in (col + 1)..n {
            if m[r][col].abs() > m[piv][col].abs() {
                piv = r;
            }
        }
        if m[piv][col].abs() <= TINY {
            return Err(EpiError::Numerical("singular variance matrix".to_string()));
        }
        if piv != col {
            m.swap(col, piv);
        }
        let diag = m[col][col];
        for elem in m[col].iter_mut() {
            *elem /= diag;
        }
        for r in 0..n {
            if r == col {
                continue;
            }
            let factor = m[r][col];
            if factor == 0.0 {
                continue;
            }
            let (lo, hi) = m.split_at_mut(r.max(col) + 1);
            let (row_r, row_col) = if r < col {
                (&mut lo[r], &hi[col])
            } else {
                (&mut hi[0], &lo[col])
            };
            for (rr, cc) in row_r.iter_mut().zip(row_col.iter()) {
                *rr -= factor * cc;
            }
        }
    }
    Ok(m.into_iter().map(|row| row[n..].to_vec()).collect())
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * b.abs().max(1.0)
    }

    #[test]
    fn km_simple_no_censoring() {
        // All events, no censoring.
        // Times: 1, 2, 3, 4, 5. S(t) should decrease to 0.
        let time = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let event = vec![1.0; 5];
        let km = kaplan_meier(&time, &event).unwrap();

        assert_eq!(km.times.len(), 5);
        assert_eq!(km.n_events, vec![1, 1, 1, 1, 1]);
        assert_eq!(km.n_at_risk, vec![5, 4, 3, 2, 1]);
        // S(1) = 4/5 = 0.8, S(2) = 0.8 * 3/4 = 0.6, ...
        assert!(approx_eq(km.survival[0], 0.8, 1e-10));
        assert!(approx_eq(km.survival[4], 0.0, 1e-10));
        assert_eq!(km.n_censored, 0);
    }

    #[test]
    fn km_with_censoring() {
        let time = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let event = vec![1.0, 0.0, 1.0, 0.0, 1.0];
        let km = kaplan_meier(&time, &event).unwrap();

        assert_eq!(km.n_censored, 2);
        // Event times: 1, 3, 5 (censored at 2, 4 are skipped).
        assert_eq!(km.times.len(), 3);
        // S(1) = 1 - 1/5 = 0.8
        assert!(approx_eq(km.survival[0], 0.8, 1e-10));
        // S(3) = 0.8 * (1 - 1/3) = 0.8 * 2/3 ≈ 0.5333
        assert!(approx_eq(km.survival[1], 0.8 * 2.0 / 3.0, 1e-10));
    }

    #[test]
    fn km_tied_event_times() {
        let time = vec![1.0, 1.0, 2.0, 2.0, 3.0];
        let event = vec![1.0, 1.0, 1.0, 0.0, 1.0];
        let km = kaplan_meier(&time, &event).unwrap();

        // Event times: 1 (2 events), 2 (1 event), 3 (1 event).
        assert_eq!(km.times, vec![1.0, 2.0, 3.0]);
        assert_eq!(km.n_events, vec![2, 1, 1]);
        // At t=1: 5 at risk. At t=2: 3 (positions 2,3,4). At t=3: 1 (position 4).
        assert_eq!(km.n_at_risk, vec![5, 3, 1]);
    }

    #[test]
    fn log_rank_no_difference() {
        // Two groups with interleaved survival times → no significant difference.
        let n = 20;
        let time: Vec<f64> = (1..=n).map(|i| i as f64).collect();
        let event: Vec<f64> = vec![1.0; n];
        let group: Vec<u64> = (0..n).map(|i| (i % 2) as u64).collect();
        let result = log_rank_test(&time, &event, &group).unwrap();

        assert_eq!(result.df, 1);
        assert!(
            result.p_value > 0.05,
            "should be non-significant: p={}",
            result.p_value
        );
    }

    #[test]
    fn log_rank_clear_difference() {
        // Group 0 has early events, group 1 has late events.
        let time = vec![
            1.0, 2.0, 3.0, 4.0, 5.0, // group 0: early events
            20.0, 21.0, 22.0, 23.0, 24.0, // group 1: late events
        ];
        let event: Vec<f64> = vec![1.0; 10];
        let group: Vec<u64> = vec![0; 5].into_iter().chain(vec![1u64; 5]).collect();
        let result = log_rank_test(&time, &event, &group).unwrap();

        assert!(
            result.chi_squared > 3.84,
            "should be significant: χ²={}",
            result.chi_squared
        );
        assert!(
            result.p_value < 0.05,
            "should be significant: p={}",
            result.p_value
        );
    }

    #[test]
    fn log_rank_observed_expected() {
        let time = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let event: Vec<f64> = vec![1.0; 6];
        let group: Vec<u64> = vec![0, 0, 0, 1, 1, 1];
        let result = log_rank_test(&time, &event, &group).unwrap();

        // With identical group sizes and evenly spaced events, O ≈ E.
        assert_eq!(result.n_groups, 2);
        assert!(result.observed[0] + result.observed[1] == 6.0);
        assert!(result.expected[0] + result.expected[1] == 6.0);
    }

    #[test]
    fn rejects_single_group() {
        let time = vec![1.0, 2.0, 3.0];
        let event = vec![1.0, 1.0, 1.0];
        let group = vec![0u64; 3];
        assert!(log_rank_test(&time, &event, &group).is_err());
    }
}
