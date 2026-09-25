//! Evaluation metrics for classification and regression.
//!
//! All functions take `&[f64]` slices; classification metrics expect integer
//! class labels encoded as `f64`. Binarise multi-class as needed upstream.
//! The multiclass section below is the one exception: it takes `&[usize]`
//! labels with per-sample probability rows (`&[Vec<f64>]`).

use thiserror::Error;

#[derive(Debug, Error)]
pub enum MetricsError {
    #[error("length mismatch: y_true has {n_true}, y_pred has {n_pred}")]
    LengthMismatch { n_true: usize, n_pred: usize },
    #[error("empty input")]
    Empty,
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, MetricsError>;

fn check_len(y_true: &[f64], y_pred: &[f64]) -> Result<()> {
    if y_true.is_empty() {
        return Err(MetricsError::Empty);
    }
    if y_true.len() != y_pred.len() {
        return Err(MetricsError::LengthMismatch {
            n_true: y_true.len(),
            n_pred: y_pred.len(),
        });
    }
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════
// Classification metrics
// ═══════════════════════════════════════════════════════════════════════

/// Accuracy: fraction of correctly classified samples.
pub fn accuracy(y_true: &[f64], y_pred: &[f64]) -> Result<f64> {
    check_len(y_true, y_pred)?;
    let n = y_true.len();
    let correct = y_true.iter().zip(y_pred).filter(|(a, b)| a == b).count();
    Ok(correct as f64 / n as f64)
}

/// Confusion matrix for integer class labels `[0, n_classes)`.
///
/// Returns `cm[actual][predicted]` as a flattened row-major `Vec<usize>`.
pub fn confusion_matrix(y_true: &[f64], y_pred: &[f64], n_classes: usize) -> Result<Vec<usize>> {
    check_len(y_true, y_pred)?;
    let mut cm = vec![0usize; n_classes * n_classes];
    for (&a, &p) in y_true.iter().zip(y_pred) {
        let a = a as usize;
        let p = p as usize;
        if a < n_classes && p < n_classes {
            cm[a * n_classes + p] += 1;
        }
    }
    Ok(cm)
}

/// Precision, recall, F1 for binary classification (positive class = 1).
///
/// Returns `(precision, recall, f1)`.
pub fn precision_recall_f1(
    y_true: &[f64],
    y_pred: &[f64],
    positive: f64,
) -> Result<(f64, f64, f64)> {
    check_len(y_true, y_pred)?;
    let mut tp = 0u64;
    let mut fp = 0u64;
    let mut fn_ = 0u64;
    for (&a, &p) in y_true.iter().zip(y_pred) {
        if p == positive && a == positive {
            tp += 1;
        } else if p == positive && a != positive {
            fp += 1;
        } else if p != positive && a == positive {
            fn_ += 1;
        }
    }
    let precision = if tp + fp > 0 {
        tp as f64 / (tp + fp) as f64
    } else {
        0.0
    };
    let recall = if tp + fn_ > 0 {
        tp as f64 / (tp + fn_) as f64
    } else {
        0.0
    };
    let f1 = if precision + recall > 0.0 {
        2.0 * precision * recall / (precision + recall)
    } else {
        0.0
    };
    Ok((precision, recall, f1))
}

/// Macro-averaged precision/recall/F1 across all classes `[0, n_classes)`.
pub fn macro_precision_recall_f1(
    y_true: &[f64],
    y_pred: &[f64],
    n_classes: usize,
) -> Result<(f64, f64, f64)> {
    check_len(y_true, y_pred)?;
    let cm = confusion_matrix(y_true, y_pred, n_classes)?;
    let mut p_sum = 0.0;
    let mut r_sum = 0.0;
    let mut f_sum = 0.0;
    for c in 0..n_classes {
        let tp = cm[c * n_classes + c];
        let fp: u64 = (0..n_classes)
            .filter(|&i| i != c)
            .map(|i| cm[i * n_classes + c] as u64)
            .sum();
        let fn_: u64 = (0..n_classes)
            .filter(|&j| j != c)
            .map(|j| cm[c * n_classes + j] as u64)
            .sum();
        let precision = if tp as u64 + fp > 0 {
            tp as f64 / (tp as u64 + fp) as f64
        } else {
            0.0
        };
        let recall = if tp as u64 + fn_ > 0 {
            tp as f64 / (tp as u64 + fn_) as f64
        } else {
            0.0
        };
        let f1 = if precision + recall > 0.0 {
            2.0 * precision * recall / (precision + recall)
        } else {
            0.0
        };
        p_sum += precision;
        r_sum += recall;
        f_sum += f1;
    }
    let n = n_classes as f64;
    Ok((p_sum / n, r_sum / n, f_sum / n))
}

/// ROC AUC via the Mann-Whitney U statistic (trapezoidal rule).
///
/// `y_score` is the predicted probability/score for the positive class.
pub fn roc_auc(y_true: &[f64], y_score: &[f64]) -> Result<f64> {
    check_len(y_true, y_score)?;
    // Mann-Whitney U → AUC
    let mut pairs: Vec<(f64, f64)> = y_score
        .iter()
        .zip(y_true.iter())
        .map(|(&s, &t)| (s, t))
        .collect();
    pairs.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let pos: Vec<f64> = pairs
        .iter()
        .filter(|(_, t)| *t == 1.0)
        .map(|(s, _)| *s)
        .collect();
    let neg: Vec<f64> = pairs
        .iter()
        .filter(|(_, t)| *t == 0.0)
        .map(|(s, _)| *s)
        .collect();
    if pos.is_empty() || neg.is_empty() {
        return Err(MetricsError::Other(
            "ROC AUC requires both classes present".into(),
        ));
    }
    let mut rank_sum_pos = 0.0;
    for &sp in &pos {
        for &sn in &neg {
            if sp > sn {
                rank_sum_pos += 1.0;
            } else if sp == sn {
                rank_sum_pos += 0.5;
            }
        }
    }
    Ok(rank_sum_pos / (pos.len() * neg.len()) as f64)
}

// ═══════════════════════════════════════════════════════════════════════
// Multiclass metrics (probability-based)
// ═══════════════════════════════════════════════════════════════════════

fn check_multiclass(y_true: &[usize], probs: &[Vec<f64>], k: usize) -> Result<()> {
    if y_true.is_empty() || probs.is_empty() {
        return Err(MetricsError::Empty);
    }
    if y_true.len() != probs.len() {
        return Err(MetricsError::LengthMismatch {
            n_true: y_true.len(),
            n_pred: probs.len(),
        });
    }
    if k < 2 {
        return Err(MetricsError::Other("multiclass metrics need k >= 2".into()));
    }
    for (i, p) in probs.iter().enumerate() {
        if p.len() < k {
            return Err(MetricsError::Other(format!(
                "probs[{i}] has {} entries, need >= {k}",
                p.len()
            )));
        }
    }
    for &y in y_true {
        if y >= k {
            return Err(MetricsError::Other(format!(
                "label {y} out of range 0..{k}"
            )));
        }
    }
    Ok(())
}

/// One-vs-rest ROC AUC for each of `k` classes.
///
/// `probs[i]` holds sample `i`'s class probabilities (length >= `k`; rows
/// need not sum to 1 — each column is ranked independently via
/// [`roc_auc`]).  A class with no positives or no negatives in `y_true`
/// yields `f64::NAN` in its slot; macro averaging skips NAN entries.
pub fn ovr_auc(y_true: &[usize], probs: &[Vec<f64>], k: usize) -> Result<Vec<f64>> {
    check_multiclass(y_true, probs, k)?;
    let aucs: Vec<f64> = (0..k)
        .map(|c| {
            let y_bin: Vec<f64> = y_true
                .iter()
                .map(|&y| if y == c { 1.0 } else { 0.0 })
                .collect();
            let score: Vec<f64> = probs.iter().map(|p| p[c]).collect();
            // a one-sided class (no positives or no negatives) is not
            // scorable: record NAN, macro averaging skips it
            roc_auc(&y_bin, &score).unwrap_or(f64::NAN)
        })
        .collect();
    Ok(aucs)
}

/// Macro-averaged one-vs-rest AUC (NAN classes skipped).
pub fn macro_ovr_auc(y_true: &[usize], probs: &[Vec<f64>], k: usize) -> Result<f64> {
    let aucs = ovr_auc(y_true, probs, k)?;
    let scored: Vec<f64> = aucs.into_iter().filter(|a| !a.is_nan()).collect();
    if scored.is_empty() {
        return Err(MetricsError::Other(
            "no class is scorable: each class needs positives and negatives".into(),
        ));
    }
    Ok(scored.iter().sum::<f64>() / scored.len() as f64)
}

/// Multiclass Brier score: mean over samples of sum_k (p_ik - 1{y_i=k})^2.
pub fn multiclass_brier(y_true: &[usize], probs: &[Vec<f64>], k: usize) -> Result<f64> {
    check_multiclass(y_true, probs, k)?;
    let n = y_true.len() as f64;
    let total: f64 = y_true
        .iter()
        .zip(probs)
        .map(|(&y, p)| {
            (0..k)
                .map(|c| {
                    let t = if c == y { 1.0 } else { 0.0 };
                    (p[c] - t).powi(2)
                })
                .sum::<f64>()
        })
        .sum();
    Ok(total / n)
}

/// Multiclass log loss: -(1/N) sum_i log(clamp(p_{i,y_i}, 1e-15)).
pub fn multiclass_log_loss(y_true: &[usize], probs: &[Vec<f64>], k: usize) -> Result<f64> {
    check_multiclass(y_true, probs, k)?;
    let n = y_true.len() as f64;
    let total: f64 = y_true
        .iter()
        .zip(probs)
        .map(|(&y, p)| -p[y].clamp(1e-15, 1.0).ln())
        .sum();
    Ok(total / n)
}

/// Balanced accuracy from a row-major confusion matrix (`cm[actual*k + predicted]`).
///
/// Classes with no samples (empty rows) are skipped.
pub fn balanced_accuracy(cm: &[usize], k: usize) -> Result<f64> {
    if cm.len() != k * k {
        return Err(MetricsError::Other(format!(
            "confusion matrix for k={k} needs {} cells, got {}",
            k * k,
            cm.len()
        )));
    }
    let recalls: Vec<f64> = (0..k)
        .filter(|&c| (0..k).map(|j| cm[c * k + j]).sum::<usize>() > 0)
        .map(|c| {
            let row: usize = (0..k).map(|j| cm[c * k + j]).sum();
            cm[c * k + c] as f64 / row as f64
        })
        .collect();
    if recalls.is_empty() {
        return Err(MetricsError::Other("confusion matrix is all zeros".into()));
    }
    Ok(recalls.iter().sum::<f64>() / recalls.len() as f64)
}

/// A single calibration bin: mean predicted probability vs observed rate.
#[derive(Debug, Clone, PartialEq)]
pub struct CalibrationBin {
    pub bin_index: usize,
    pub n: usize,
    pub mean_pred: f64,
    pub obs_rate: f64,
}

/// One-vs-rest calibration curve for a single class.
///
/// Bins predicted probabilities into `n_bins` equal-width bins (p=1 falls
/// into the last bin) and reports the observed positive rate per bin, plus
/// the calibration slope/intercept from a univariate logistic fit of the
/// outcome on the logit of the predicted probability (probabilities
/// clamped to [1e-6, 1-1e-6]).  `slope`/`intercept` stay NAN when the
/// probabilities are constant (degenerate fit).
#[derive(Debug, Clone, PartialEq)]
pub struct CalibrationCurve {
    pub bins: Vec<CalibrationBin>,
    pub slope: f64,
    pub intercept: f64,
}

pub fn class_calibration(y_bin: &[bool], p: &[f64], n_bins: usize) -> Result<CalibrationCurve> {
    if y_bin.is_empty() || p.is_empty() {
        return Err(MetricsError::Empty);
    }
    if y_bin.len() != p.len() {
        return Err(MetricsError::LengthMismatch {
            n_true: y_bin.len(),
            n_pred: p.len(),
        });
    }
    if n_bins < 2 {
        return Err(MetricsError::Other("n_bins must be >= 2".into()));
    }
    for &v in p {
        if !(0.0..=1.0).contains(&v) {
            return Err(MetricsError::Other(format!(
                "probability {v} outside [0,1]"
            )));
        }
    }

    let mut n = vec![0usize; n_bins];
    let mut sum_p = vec![0.0f64; n_bins];
    let mut sum_y = vec![0usize; n_bins];
    for (&y, &v) in y_bin.iter().zip(p) {
        let b = ((v * n_bins as f64).floor() as usize).min(n_bins - 1);
        n[b] += 1;
        sum_p[b] += v;
        sum_y[b] += y as usize;
    }
    let bins: Vec<CalibrationBin> = (0..n_bins)
        .filter(|&b| n[b] > 0)
        .map(|b| CalibrationBin {
            bin_index: b,
            n: n[b],
            mean_pred: sum_p[b] / n[b] as f64,
            obs_rate: sum_y[b] as f64 / n[b] as f64,
        })
        .collect();

    // Univariate logistic regression y ~ a + b*logit(p), Newton-Raphson.
    let eps = 1e-6f64;
    let xs: Vec<f64> = p
        .iter()
        .map(|&v| {
            let c = v.clamp(eps, 1.0 - eps);
            (c / (1.0 - c)).ln()
        })
        .collect();
    let mut a = 0.0f64;
    let mut b = 0.0f64;
    let mut ok = false;
    for _ in 0..25 {
        let mut g = [0.0f64; 2]; // gradient of the log-likelihood
        let mut h = [[0.0f64; 2]; 2]; // negative Hessian (Fisher info)
        for (&x, &y) in xs.iter().zip(y_bin) {
            let eta = a + b * x;
            let mu = 1.0 / (1.0 + (-eta).exp());
            let w = mu * (1.0 - mu) + 1e-12;
            let r = if y { 1.0 } else { 0.0 } - mu;
            g[0] += r;
            g[1] += r * x;
            h[0][0] += w;
            h[0][1] += w * x;
            h[1][0] += w * x;
            h[1][1] += w * x * x;
        }
        let det = h[0][0] * h[1][1] - h[0][1] * h[1][0];
        if det.abs() < 1e-12 {
            break; // constant p: leave slope/intercept NAN
        }
        let da = (h[1][1] * g[0] - h[0][1] * g[1]) / det;
        let db = (h[0][0] * g[1] - h[1][0] * g[0]) / det;
        a += da;
        b += db;
        if da.abs() < 1e-10 && db.abs() < 1e-10 {
            ok = true;
            break;
        }
        ok = true;
    }
    let (slope, intercept) = if ok { (b, a) } else { (f64::NAN, f64::NAN) };
    Ok(CalibrationCurve {
        bins,
        slope,
        intercept,
    })
}

/// Cluster (group) bootstrap: resample entire clusters with replacement.
///
/// `clusters[i]` is the group id of sample `i` (e.g. a patient id encoded
/// as u64).  Each of `n_boot` replicates draws `n_clusters` cluster ids
/// with replacement, concatenates their member row indices in original
/// row order, and evaluates `stat` on that index vector.  Deterministic
/// for a given `seed` (ChaCha8).  The caller turns the replicates into
/// percentile intervals.
pub fn cluster_bootstrap<T>(
    stat: impl Fn(&[usize]) -> T,
    clusters: &[u64],
    n_boot: usize,
    seed: u64,
) -> Result<Vec<T>> {
    use rand::Rng;
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    if clusters.is_empty() {
        return Err(MetricsError::Empty);
    }
    if n_boot == 0 {
        return Err(MetricsError::Other("n_boot must be >= 1".into()));
    }
    let unique: Vec<u64> = clusters
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<u64>>()
        .into_iter()
        .collect();
    let members: std::collections::HashMap<u64, Vec<usize>> = {
        let mut m: std::collections::HashMap<u64, Vec<usize>> = Default::default();
        for (i, &g) in clusters.iter().enumerate() {
            m.entry(g).or_default().push(i);
        }
        m
    };

    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let n_groups = unique.len();
    let mut out = Vec::with_capacity(n_boot);
    for _ in 0..n_boot {
        let mut idx: Vec<usize> = Vec::with_capacity(clusters.len());
        for _ in 0..n_groups {
            let draw = unique[rng.random_range(0..n_groups)];
            idx.extend_from_slice(&members[&draw]);
        }
        idx.sort_unstable();
        out.push(stat(&idx));
    }
    Ok(out)
}

// ═══════════════════════════════════════════════════════════════════════
// Regression metrics
// ═══════════════════════════════════════════════════════════════════════

/// Mean squared error.
pub fn mse(y_true: &[f64], y_pred: &[f64]) -> Result<f64> {
    check_len(y_true, y_pred)?;
    let n = y_true.len() as f64;
    Ok(y_true
        .iter()
        .zip(y_pred)
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f64>()
        / n)
}

/// Root mean squared error.
pub fn rmse(y_true: &[f64], y_pred: &[f64]) -> Result<f64> {
    Ok(mse(y_true, y_pred)?.sqrt())
}

/// Mean absolute error.
pub fn mae(y_true: &[f64], y_pred: &[f64]) -> Result<f64> {
    check_len(y_true, y_pred)?;
    let n = y_true.len() as f64;
    Ok(y_true
        .iter()
        .zip(y_pred)
        .map(|(a, b)| (a - b).abs())
        .sum::<f64>()
        / n)
}

/// R² (coefficient of determination).
pub fn r2_score(y_true: &[f64], y_pred: &[f64]) -> Result<f64> {
    check_len(y_true, y_pred)?;
    let mean = y_true.iter().sum::<f64>() / y_true.len() as f64;
    let ss_res: f64 = y_true
        .iter()
        .zip(y_pred)
        .map(|(a, b)| (a - b).powi(2))
        .sum();
    let ss_tot: f64 = y_true.iter().map(|a| (a - mean).powi(2)).sum();
    if ss_tot == 0.0 {
        return Ok(0.0);
    }
    Ok(1.0 - ss_res / ss_tot)
}

/// Mean absolute percentage error (as fraction, not %).
pub fn mape(y_true: &[f64], y_pred: &[f64]) -> Result<f64> {
    check_len(y_true, y_pred)?;
    let n = y_true.len() as f64;
    let sum: f64 = y_true
        .iter()
        .zip(y_pred)
        .map(|(a, b)| {
            if a.abs() > 1e-12 {
                ((a - b) / a).abs()
            } else {
                0.0
            }
        })
        .sum();
    Ok(sum / n)
}

/// Explained variance score.
pub fn explained_variance(y_true: &[f64], y_pred: &[f64]) -> Result<f64> {
    check_len(y_true, y_pred)?;
    let mean_true: f64 = y_true.iter().sum::<f64>() / y_true.len() as f64;
    let mean_pred: f64 = y_pred.iter().sum::<f64>() / y_pred.len() as f64;
    let numerator: f64 = y_true
        .iter()
        .zip(y_pred)
        .map(|(a, b)| (a - mean_true - (b - mean_pred)).powi(2))
        .sum();
    let denominator: f64 = y_true.iter().map(|a| (a - mean_true).powi(2)).sum();
    if denominator == 0.0 {
        return Ok(0.0);
    }
    Ok(1.0 - numerator / denominator)
}

/// Max error.
pub fn max_error(y_true: &[f64], y_pred: &[f64]) -> Result<f64> {
    check_len(y_true, y_pred)?;
    Ok(y_true
        .iter()
        .zip(y_pred)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f64, f64::max))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_accuracy() {
        assert_eq!(
            accuracy(&[0., 1., 1., 0.], &[0., 1., 0., 0.]).unwrap(),
            0.75
        );
    }

    #[test]
    fn test_prf() {
        let (p, r, f) =
            precision_recall_f1(&[0., 1., 1., 0., 1.], &[0., 1., 0., 0., 1.], 1.0).unwrap();
        assert!((p - 1.0).abs() < 1e-10);
        assert!((r - 2.0 / 3.0).abs() < 1e-10);
        assert!((f - 0.8).abs() < 1e-10);
    }

    #[test]
    fn test_mse_r2() {
        let y_true = vec![3.0, -0.5, 2.0, 7.0];
        let y_pred = vec![2.5, 0.0, 2.0, 8.0];
        let m = mse(&y_true, &y_pred).unwrap();
        assert!((m - 0.375).abs() < 1e-10);
        let r2 = r2_score(&y_true, &y_pred).unwrap();
        assert!((r2 - 0.9486081370449679).abs() < 1e-6);
    }

    #[test]
    fn test_roc_auc() {
        let y_true = vec![0.0, 0.0, 1.0, 1.0];
        let y_score = vec![0.1, 0.4, 0.35, 0.8];
        let auc = roc_auc(&y_true, &y_score).unwrap();
        assert!((auc - 0.75).abs() < 1e-10);
    }

    #[test]
    fn test_confusion_matrix() {
        let cm = confusion_matrix(&[0., 1., 2., 0., 1.], &[0., 1., 2., 1., 1.], 3).unwrap();
        // cm[actual * 3 + predicted]
        assert_eq!(cm[0], 1); // actual=0 pred=0
        assert_eq!(cm[1], 1); // actual=0 pred=1
        assert_eq!(cm[4], 2); // actual=1 pred=1
        assert_eq!(cm[8], 1); // actual=2 pred=2
    }

    #[test]
    fn test_macro_prf() {
        let y_true = vec![0., 0., 1., 1., 2., 2.];
        let y_pred = vec![0., 0., 1., 1., 2., 2.];
        let (p, r, f) = macro_precision_recall_f1(&y_true, &y_pred, 3).unwrap();
        assert!((p - 1.0).abs() < 1e-10);
        assert!((r - 1.0).abs() < 1e-10);
        assert!((f - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_ovr_auc_perfect() {
        let y = vec![0, 1, 2];
        let probs = vec![
            vec![0.8, 0.1, 0.1],
            vec![0.1, 0.8, 0.1],
            vec![0.1, 0.1, 0.8],
        ];
        let aucs = ovr_auc(&y, &probs, 3).unwrap();
        for a in aucs {
            assert!((a - 1.0).abs() < 1e-10);
        }
        assert!((macro_ovr_auc(&y, &probs, 3).unwrap() - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_ovr_auc_hand_computed() {
        // class 0: pos scores [0.9, 0.3] vs neg scores [0.6, 0.2]
        // pairs: (0.9>0.6)=1 (0.9>0.2)=1 (0.3>0.6)=0 (0.3>0.2)=1 -> 3/4
        let y = vec![0, 0, 1, 2];
        let probs = vec![
            vec![0.9, 0.05, 0.05],
            vec![0.3, 0.3, 0.4],
            vec![0.6, 0.3, 0.1],
            vec![0.2, 0.2, 0.6],
        ];
        let aucs = ovr_auc(&y, &probs, 3).unwrap();
        assert!((aucs[0] - 0.75).abs() < 1e-10);
    }

    #[test]
    fn test_ovr_auc_absent_class_nan_macro_skips() {
        let y = vec![0, 1, 0]; // class 2 never occurs
        let probs = vec![
            vec![0.7, 0.2, 0.1],
            vec![0.2, 0.7, 0.1],
            vec![0.6, 0.3, 0.1],
        ];
        let aucs = ovr_auc(&y, &probs, 3).unwrap();
        assert!(aucs[2].is_nan());
        let macro_auc = macro_ovr_auc(&y, &probs, 3).unwrap();
        let expected = (aucs[0] + aucs[1]) / 2.0;
        assert!((macro_auc - expected).abs() < 1e-10);
    }

    #[test]
    fn test_multiclass_brier_log_loss() {
        let y = vec![0];
        let perfect = vec![vec![1.0, 0.0, 0.0]];
        assert!(multiclass_brier(&y, &perfect, 3).unwrap().abs() < 1e-10);
        let half = vec![vec![0.0, 0.5, 0.5]];
        assert!((multiclass_brier(&y, &half, 3).unwrap() - 1.5).abs() < 1e-10);

        let p = vec![vec![0.5, 0.25, 0.25]];
        assert!((multiclass_log_loss(&y, &p, 3).unwrap() - 2.0f64.ln()).abs() < 1e-10);
    }

    #[test]
    fn test_balanced_accuracy() {
        // recalls 2/3 and 1 -> mean 5/6
        let cm = vec![2, 1, 0, 3];
        assert!((balanced_accuracy(&cm, 2).unwrap() - 5.0 / 6.0).abs() < 1e-10);
        // only an all-zero matrix has no class with samples
        assert!(balanced_accuracy(&[0, 0, 0, 0], 2).is_err());
    }

    #[test]
    fn test_calibration_bins() {
        let p = vec![0.05, 0.15, 0.25];
        let y = vec![false, false, true];
        let curve = class_calibration(&y, &p, 10).unwrap();
        assert_eq!(curve.bins.len(), 3);
        assert_eq!(curve.bins[0].bin_index, 0);
        assert_eq!(curve.bins[2].bin_index, 2);
        assert_eq!(curve.bins[2].obs_rate, 1.0);
        assert!(curve.bins.iter().all(|b| b.n == 1));
    }

    #[test]
    fn test_calibration_slope_recovers_identity() {
        // Bernoulli outcomes drawn with success probability p (ChaCha8,
        // fixed seed): the data is calibrated by construction and not
        // separable, so the logistic slope recovers ~1, intercept ~0.
        // p itself comes from a golden-ratio low-discrepancy sequence to
        // cover (0.05, 0.95) evenly.
        use rand::Rng;
        use rand::SeedableRng;
        use rand_chacha::ChaCha8Rng;
        let p: Vec<f64> = (0..400)
            .map(|i| 0.05 + 0.9 * ((i as f64 * 0.6180339887498949) % 1.0))
            .collect();
        let mut rng = ChaCha8Rng::seed_from_u64(42);
        let y: Vec<bool> = p.iter().map(|&q| rng.random_bool(q)).collect();
        let curve = class_calibration(&y, &p, 10).unwrap();
        assert!((0.6..1.6).contains(&curve.slope), "slope {}", curve.slope);
        assert!(curve.intercept.abs() < 0.5, "intercept {}", curve.intercept);
    }

    #[test]
    fn test_calibration_constant_p_degenerate() {
        let curve = class_calibration(&[true, false], &[0.5, 0.5], 10).unwrap();
        assert!(curve.slope.is_nan());
        assert!(curve.intercept.is_nan());
    }

    #[test]
    fn test_cluster_bootstrap_deterministic() {
        let clusters = [1u64, 1, 2, 2, 3];
        let a =
            cluster_bootstrap(|idx: &[usize]| idx.iter().sum::<usize>(), &clusters, 20, 7).unwrap();
        let b =
            cluster_bootstrap(|idx: &[usize]| idx.iter().sum::<usize>(), &clusters, 20, 7).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn test_cluster_bootstrap_groups_intact() {
        let clusters = [5u64, 5, 7, 7, 9];
        let sets: Vec<Vec<usize>> =
            cluster_bootstrap(|idx: &[usize]| idx.to_vec(), &clusters, 50, 1).unwrap();
        for idx in &sets {
            let contains = |i: usize| idx.binary_search(&i).is_ok();
            for pair in [(0usize, 1usize), (2usize, 3usize)] {
                assert_eq!(contains(pair.0), contains(pair.1));
            }
        }
    }
}
