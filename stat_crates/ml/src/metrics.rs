//! Evaluation metrics for classification and regression.
//!
//! All functions take `&[f64]` slices; classification metrics expect integer
//! class labels encoded as `f64`. Binarise multi-class as needed upstream.

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
pub fn confusion_matrix(
    y_true: &[f64],
    y_pred: &[f64],
    n_classes: usize,
) -> Result<Vec<usize>> {
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
    let mut pairs: Vec<(f64, f64)> = y_score.iter().zip(y_true.iter()).map(|(&s, &t)| (s, t)).collect();
    pairs.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let pos: Vec<f64> = pairs.iter().filter(|(_, t)| *t == 1.0).map(|(s, _)| *s).collect();
    let neg: Vec<f64> = pairs.iter().filter(|(_, t)| *t == 0.0).map(|(s, _)| *s).collect();
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
// Regression metrics
// ═══════════════════════════════════════════════════════════════════════

/// Mean squared error.
pub fn mse(y_true: &[f64], y_pred: &[f64]) -> Result<f64> {
    check_len(y_true, y_pred)?;
    let n = y_true.len() as f64;
    Ok(y_true.iter().zip(y_pred).map(|(a, b)| (a - b).powi(2)).sum::<f64>() / n)
}

/// Root mean squared error.
pub fn rmse(y_true: &[f64], y_pred: &[f64]) -> Result<f64> {
    Ok(mse(y_true, y_pred)?.sqrt())
}

/// Mean absolute error.
pub fn mae(y_true: &[f64], y_pred: &[f64]) -> Result<f64> {
    check_len(y_true, y_pred)?;
    let n = y_true.len() as f64;
    Ok(y_true.iter().zip(y_pred).map(|(a, b)| (a - b).abs()).sum::<f64>() / n)
}

/// R² (coefficient of determination).
pub fn r2_score(y_true: &[f64], y_pred: &[f64]) -> Result<f64> {
    check_len(y_true, y_pred)?;
    let mean = y_true.iter().sum::<f64>() / y_true.len() as f64;
    let ss_res: f64 = y_true.iter().zip(y_pred).map(|(a, b)| (a - b).powi(2)).sum();
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
        .map(|(a, b)| if a.abs() > 1e-12 { ((a - b) / a).abs() } else { 0.0 })
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
        assert_eq!(accuracy(&[0., 1., 1., 0.], &[0., 1., 0., 0.]).unwrap(), 0.75);
    }

    #[test]
    fn test_prf() {
        let (p, r, f) = precision_recall_f1(&[0., 1., 1., 0., 1.], &[0., 1., 0., 0., 1.], 1.0).unwrap();
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
}
