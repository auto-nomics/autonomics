//! ROC curve analysis: AUC (Mann-Whitney U), DeLong test, bootstrap CI.
//!
//! Used in the paper's Figure 6 to compare predictive performance of sleep
//! duration, sleep quality, and combined logistic models for PD.

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use statkit::descriptive;
use statrs::distribution::{ContinuousCDF, Normal};

use crate::error::{EpiError, Result};

/// Result of an ROC/AUC analysis.
#[derive(Debug, Clone)]
pub struct RocResult {
    /// Area under the ROC curve (AUC = Mann-Whitney U statistic / (n_pos * n_neg)).
    pub auc: f64,
    /// Standard error of the AUC (Hanley-McNeil).
    pub se: f64,
    /// 95% CI lower bound (Normal approximation or bootstrap).
    pub ci_lower: f64,
    /// 95% CI upper bound.
    pub ci_upper: f64,
    /// Number of positive cases.
    pub n_pos: usize,
    /// Number of negative cases.
    pub n_neg: usize,
}

/// Compute AUC via the Mann-Whitney U statistic.
///
/// `scores` are the predicted probabilities (or any continuous predictor);
/// `labels` are 0/1 outcomes. AUC = P(score_pos > score_neg) with ties counted
/// as half.
pub fn auc(scores: &[f64], labels: &[u64]) -> Result<RocResult> {
    if scores.len() != labels.len() {
        return Err(EpiError::DimensionMismatch {
            a: scores.len(),
            b: labels.len(),
        });
    }
    let n = scores.len();
    if n == 0 {
        return Err(EpiError::EmptyInput);
    }

    // Rank all scores (average method for ties).
    let ranks = descriptive::rank(scores)?;

    let n_pos: usize = labels.iter().filter(|&&l| l == 1).count();
    let n_neg = n - n_pos;
    if n_pos == 0 || n_neg == 0 {
        return Err(EpiError::Numerical(
            "cannot compute AUC with all-positive or all-negative labels".to_string(),
        ));
    }

    // Sum of ranks for positive cases.
    let rank_sum_pos: f64 = labels
        .iter()
        .zip(&ranks)
        .filter(|(l, _)| **l == 1)
        .map(|(_, r)| *r)
        .sum();

    // Mann-Whitney U = rank_sum_pos − n_pos(n_pos+1)/2.
    let u = rank_sum_pos - n_pos as f64 * (n_pos as f64 + 1.0) / 2.0;
    let auc = u / (n_pos as f64 * n_neg as f64);

    // Hanley-McNeil standard error.
    let q1 = auc / (2.0 - auc);
    let q2 = 2.0 * auc / (1.0 + auc);
    let se = ((auc * (1.0 - auc)
        + (n_pos as f64 - 1.0) * (q1 - auc * auc)
        + (n_neg as f64 - 1.0) * (q2 - auc * auc))
        / (n_pos as f64 * n_neg as f64))
        .sqrt();

    let z975 = 1.959963984540054_f64;
    Ok(RocResult {
        auc,
        se,
        ci_lower: (auc - z975 * se).max(0.0),
        ci_upper: (auc + z975 * se).min(1.0),
        n_pos,
        n_neg,
    })
}

/// Result of a DeLong test comparing two AUCs.
#[derive(Debug, Clone)]
pub struct DelongResult {
    /// AUC of model 1.
    pub auc1: f64,
    /// AUC of model 2.
    pub auc2: f64,
    /// Difference (auc1 − auc2).
    pub delta: f64,
    /// z-statistic.
    pub z: f64,
    /// Two-sided p-value.
    pub p_value: f64,
}

/// DeLong test for comparing two correlated AUCs from the same set of
/// subjects (both models scored the same observations).
///
/// Uses the DeLong et al. (1988) covariance estimation: for each positive
/// subject, compute the "placement value" V₁₀ = fraction of negatives scored
/// below; for each negative, V₀₁ = fraction of positives scored above. The
/// covariance of the two AUC difference is estimated from the joint
/// distribution of these placement values.
pub fn delong_test(scores1: &[f64], scores2: &[f64], labels: &[u64]) -> Result<DelongResult> {
    let n = labels.len();
    if scores1.len() != n || scores2.len() != n {
        return Err(EpiError::DimensionMismatch {
            a: n,
            b: scores1.len(),
        });
    }

    let pos_idx: Vec<usize> = (0..n).filter(|&i| labels[i] == 1).collect();
    let neg_idx: Vec<usize> = (0..n).filter(|&i| labels[i] == 0).collect();
    let n_pos = pos_idx.len();
    let n_neg = neg_idx.len();
    if n_pos == 0 || n_neg == 0 {
        return Err(EpiError::Numerical(
            "DeLong test requires both positive and negative cases".to_string(),
        ));
    }

    // For each model k, compute per-subject structural components:
    // V_k(pos_j) = (1/n_neg) * Σ_neg [I(score_k(pos_j) > score_k(neg_i)) + 0.5*I(tie)]
    // V_k(neg_i) = (1/n_pos) * Σ_pos [I(score_k(pos_j) > score_k(neg_i)) + 0.5*I(tie)]
    let compute_v = |scores: &[f64]| -> (Vec<f64>, Vec<f64>) {
        let mut v_pos = vec![0.0; n_pos];
        let mut v_neg = vec![0.0; n_neg];
        for (pj, &p) in pos_idx.iter().enumerate() {
            for (ni, &ng) in neg_idx.iter().enumerate() {
                let val = pairwise_score(scores[p], scores[ng]);
                v_pos[pj] += val;
                v_neg[ni] += val;
            }
        }
        for v in &mut v_pos {
            *v /= n_neg as f64;
        }
        for v in &mut v_neg {
            *v /= n_pos as f64;
        }
        (v_pos, v_neg)
    };

    let (v1_pos, v1_neg) = compute_v(scores1);
    let (v2_pos, v2_neg) = compute_v(scores2);

    let auc1: f64 = v1_pos.iter().sum::<f64>() / n_pos as f64;
    let auc2: f64 = v2_pos.iter().sum::<f64>() / n_pos as f64;

    // Covariance matrix of (AUC1, AUC2) — DeLong's method.
    // S₁₀ component (positive subjects): Cov contribution from positives.
    let cov_component = |va_pos: &[f64], vb_pos: &[f64], va_neg: &[f64], vb_neg: &[f64]| -> f64 {
        let cov_pos = covariance_mean(va_pos, vb_pos) / n_pos as f64;
        let cov_neg = covariance_mean(va_neg, vb_neg) / n_neg as f64;
        cov_pos + cov_neg
    };

    let var1 = cov_component(&v1_pos, &v1_pos, &v1_neg, &v1_neg);
    let var2 = cov_component(&v2_pos, &v2_pos, &v2_neg, &v2_neg);
    let cov12 = cov_component(&v1_pos, &v2_pos, &v1_neg, &v2_neg);

    let var_diff = var1 + var2 - 2.0 * cov12;
    let delta = auc1 - auc2;
    if var_diff <= 0.0 {
        return Ok(DelongResult {
            auc1,
            auc2,
            delta,
            z: f64::NAN,
            p_value: if delta.abs() < 1e-15 { 1.0 } else { f64::NAN },
        });
    }
    let z = delta / var_diff.sqrt();
    let normal = Normal::new(0.0, 1.0).map_err(|e| EpiError::Numerical(e.to_string()))?;
    let p_value = 2.0 * normal.sf(z.abs());

    Ok(DelongResult {
        auc1,
        auc2,
        delta,
        z,
        p_value,
    })
}

/// Bootstrap 95% CI for the AUC.
///
/// Resamples the paired (score, label) data `n_boot` times with a fixed seed
/// for reproducibility, computing the percentile interval.
pub fn bootstrap_auc_ci(
    scores: &[f64],
    labels: &[u64],
    n_boot: usize,
    seed: u64,
) -> Result<RocResult> {
    let n = scores.len();
    if scores.len() != labels.len() || n == 0 {
        return Err(EpiError::EmptyInput);
    }

    let point = auc(scores, labels)?;
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    use rand::seq::IndexedRandom;

    let mut boot_aucs = Vec::with_capacity(n_boot);
    let idx_pool: Vec<usize> = (0..n).collect();
    for _ in 0..n_boot {
        let mut boot_scores = Vec::with_capacity(n);
        let mut boot_labels = Vec::with_capacity(n);
        for _ in 0..n {
            let i = idx_pool.choose(&mut rng).copied().unwrap();
            boot_scores.push(scores[i]);
            boot_labels.push(labels[i]);
        }
        let n_pos: usize = boot_labels.iter().filter(|&&l| l == 1).count();
        if n_pos == 0 || n_pos == n {
            continue; // skip degenerate resample
        }
        if let Ok(r) = auc(&boot_scores, &boot_labels) {
            boot_aucs.push(r.auc);
        }
    }

    boot_aucs.sort_by(|a, b| a.total_cmp(b));
    let lo = boot_aucs[(0.025 * boot_aucs.len() as f64) as usize];
    let hi = boot_aucs[(0.975 * boot_aucs.len() as f64) as usize - 1];

    Ok(RocResult {
        auc: point.auc,
        se: point.se,
        ci_lower: lo,
        ci_upper: hi,
        n_pos: point.n_pos,
        n_neg: point.n_neg,
    })
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Scoring function for Mann-Whitney: 1.0 if a > b, 0.5 if tie, 0.0 if a < b.
fn pairwise_score(a: f64, b: f64) -> f64 {
    if a > b {
        1.0
    } else if a < b {
        0.0
    } else {
        0.5
    }
}

/// Sample covariance of two slices, divided by n (population form, as DeLong
/// uses the plug-in estimator).
fn covariance_mean(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len() as f64;
    let ma = a.iter().sum::<f64>() / n;
    let mb = b.iter().sum::<f64>() / n;
    a.iter()
        .zip(b)
        .map(|(&x, &y)| (x - ma) * (y - mb))
        .sum::<f64>()
        / n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * b.abs().max(1.0)
    }

    #[test]
    fn perfect_separation_auc_one() {
        // All positives score higher than all negatives.
        let scores = vec![0.1, 0.2, 0.3, 0.8, 0.9, 1.0];
        let labels = vec![0, 0, 0, 1, 1, 1];
        let res = auc(&scores, &labels).unwrap();
        assert!(approx_eq(res.auc, 1.0, 1e-9));
        assert_eq!(res.n_pos, 3);
        assert_eq!(res.n_neg, 3);
    }

    #[test]
    fn random_classifier_auc_half() {
        // Pos (label=1) at scores 1.0 and 4.0; neg at 2.0 and 3.0.
        // U = (0+0) + (1+1) = 2, AUC = 2/(2*2) = 0.5.
        let scores = vec![1.0, 2.0, 3.0, 4.0];
        let labels = vec![1, 0, 0, 1];
        let res = auc(&scores, &labels).unwrap();
        assert!(approx_eq(res.auc, 0.5, 1e-9));
    }

    #[test]
    fn reversed_scores_auc_zero() {
        // All positives score lower than negatives.
        let scores = vec![0.9, 0.8, 0.7, 0.3, 0.2, 0.1];
        let labels = vec![0, 0, 0, 1, 1, 1];
        let res = auc(&scores, &labels).unwrap();
        assert!(approx_eq(res.auc, 0.0, 1e-9));
    }

    #[test]
    fn ties_handled_correctly() {
        // Two tied pairs: score=0.3 for both pos and neg.
        let scores = vec![0.1, 0.3, 0.3, 0.9];
        let labels = vec![0, 0, 1, 1];
        let res = auc(&scores, &labels).unwrap();
        // Manual: pos=0.3 beats neg=0.1 (1.0), ties neg=0.3 (0.5)
        //         pos=0.9 beats both negs (1.0 + 1.0)
        // U = 1.0+0.5+1.0+1.0 = 3.5, AUC = 3.5 / (2*2) = 0.875
        assert!(approx_eq(res.auc, 0.875, 1e-9));
    }

    #[test]
    fn delong_identical_models() {
        // Same scores → delta = 0, p should be large (or NaN with var=0).
        let scores = vec![0.1, 0.3, 0.5, 0.7, 0.9, 0.2];
        let labels = vec![0, 0, 1, 1, 1, 0];
        let res = delong_test(&scores, &scores, &labels).unwrap();
        assert!(res.delta.abs() < 1e-10);
    }

    #[test]
    fn delong_different_models() {
        // Model 1 is clearly better than model 2.
        let s1 = vec![0.1, 0.2, 0.3, 0.8, 0.9, 0.95];
        let s2 = vec![0.4, 0.5, 0.45, 0.55, 0.6, 0.5];
        let labels = vec![0, 0, 0, 1, 1, 1];
        let res = delong_test(&s1, &s2, &labels).unwrap();
        assert!(res.auc1 > res.auc2);
    }

    #[test]
    fn bootstrap_ci_covers_point_estimate() {
        let scores = vec![0.1, 0.2, 0.3, 0.4, 0.6, 0.7, 0.8, 0.9];
        let labels = vec![0, 0, 0, 0, 1, 1, 1, 1];
        let res = bootstrap_auc_ci(&scores, &labels, 500, 42).unwrap();
        assert!(res.ci_lower <= res.auc);
        assert!(res.auc <= res.ci_upper);
    }
}
