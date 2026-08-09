//! SVM + Ensemble learning — SVM classification, AdaBoost.
//!
//! Uses [`linfa-svm`] for SVM and [`linfa-ensemble`] for AdaBoost.

use faer::Mat;
use thiserror::Error;

use crate::dimred::faer_to_ndarray;

#[derive(Debug, Error)]
pub enum SvmEnsembleError {
    #[error("empty input")]
    Empty,
    #[error("linfa error: {0}")]
    Linfa(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, SvmEnsembleError>;

// ═══════════════════════════════════════════════════════════════════════
// SVM Classification (linfa-svm)
// ═══════════════════════════════════════════════════════════════════════

pub struct SvmResult {
    pub predictions: Vec<usize>,
}

pub fn svm_classify(data: &Mat<f64>, labels: &[usize], kernel: &str, c: f64) -> Result<SvmResult> {
    use linfa::dataset::DatasetBase;
    use linfa::traits::{Fit, Predict};
    use linfa_svm::Svm;

    let (nrows, _) = data.shape();
    if nrows == 0 {
        return Err(SvmEnsembleError::Empty);
    }

    let x = faer_to_ndarray(data);
    let y: Vec<bool> = labels.iter().map(|&l| l != 0).collect();
    let dataset = DatasetBase::new(x, ndarray::Array1::from(y));

    let mut params = Svm::<_, bool>::params().pos_neg_weights(1.0, 1.0);
    match kernel {
        "linear" => {
            params = params.linear_kernel();
        }
        "rbf" | "gaussian" => {
            params = params.gaussian_kernel(c);
        }
        "poly" | "polynomial" => {
            params = params.polynomial_kernel(c, 3.0);
        }
        _ => {
            params = params.gaussian_kernel(c);
        }
    }

    let model = params
        .fit(&dataset)
        .map_err(|e| SvmEnsembleError::Linfa(e.to_string()))?;

    let predicted = model.predict(dataset.records());
    let predictions: Vec<usize> = predicted.iter().map(|&p| if p { 1 } else { 0 }).collect();

    Ok(SvmResult { predictions })
}

// ═══════════════════════════════════════════════════════════════════════
// AdaBoost (linfa-ensemble)
// ═══════════════════════════════════════════════════════════════════════

pub fn adaboost(
    data: &Mat<f64>,
    labels: &[usize],
    n_estimators: usize,
    learning_rate: f64,
) -> Result<SvmResult> {
    // AdaBoost via linfa-ensemble requires complex trait bounds.
    // For now, implement a simple AdaBoost.M1 with decision stumps natively.
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    let (nrows, ncols) = data.shape();
    if nrows == 0 {
        return Err(SvmEnsembleError::Empty);
    }

    let _rng = ChaCha8Rng::seed_from_u64(42);
    let y: Vec<f64> = labels
        .iter()
        .map(|&l| if l != 0 { 1.0 } else { -1.0 })
        .collect();

    // Initialize sample weights uniformly
    let mut weights = vec![1.0 / nrows as f64; nrows];

    // Store weak learners: (feature, threshold, direction, alpha)
    let mut weak_learners: Vec<(usize, f64, f64, f64)> = Vec::new();

    for _ in 0..n_estimators {
        // Find best decision stump
        let mut best_err = f64::INFINITY;
        let mut best_stump = (0usize, 0.0, 1.0);

        for j in 0..ncols {
            let col: Vec<f64> = (0..nrows).map(|i| data[(i, j)]).collect();
            let mut sorted = col.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            sorted.dedup();

            for win in sorted.windows(2) {
                let threshold = (win[0] + win[1]) / 2.0;
                for &direction in &[1.0, -1.0] {
                    let err: f64 = (0..nrows)
                        .filter(|&i| {
                            let pred = if col[i] * direction > threshold * direction {
                                1.0
                            } else {
                                -1.0
                            };
                            pred != y[i]
                        })
                        .map(|i| weights[i])
                        .sum();
                    if err < best_err {
                        best_err = err;
                        best_stump = (j, threshold, direction);
                    }
                }
            }
        }

        if best_err >= 0.5 {
            break;
        }

        let eps = 1e-10;
        let alpha = 0.5 * ((1.0 - best_err) / (best_err + eps)).ln();
        weak_learners.push((best_stump.0, best_stump.1, best_stump.2, alpha));

        // Update weights
        let (feat, thresh, dir) = best_stump;
        let col: Vec<f64> = (0..nrows).map(|i| data[(i, feat)]).collect();
        for i in 0..nrows {
            let pred = if col[i] * dir > thresh * dir {
                1.0
            } else {
                -1.0
            };
            if pred != y[i] {
                weights[i] *= (alpha).exp();
            } else {
                weights[i] *= (-alpha).exp();
            }
        }
        // Normalize
        let sum: f64 = weights.iter().sum();
        for w in &mut weights {
            *w /= sum;
        }
    }

    // Predict using weighted ensemble
    let predictions: Vec<usize> = (0..nrows)
        .map(|i| {
            let mut score = 0.0;
            for &(feat, thresh, dir, alpha) in &weak_learners {
                let val = data[(i, feat)];
                let pred = if val * dir > thresh * dir { 1.0 } else { -1.0 };
                score += alpha * pred;
            }
            if score > 0.0 { 1 } else { 0 }
        })
        .collect();

    let _ = learning_rate; // TODO: apply learning_rate scaling
    Ok(SvmResult { predictions })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::mat_from_row_major;

    #[test]
    fn test_svm() {
        let data = mat_from_row_major(
            8,
            2,
            &[
                0.0, 0.0, 0.5, 0.5, 0.1, 0.2, 0.3, 0.1, 5.0, 5.0, 5.5, 5.5, 5.1, 5.2, 5.3, 5.1,
            ],
        );
        let labels = vec![0, 0, 0, 0, 1, 1, 1, 1];
        let result = svm_classify(&data, &labels, "linear", 1.0).unwrap();
        assert_eq!(result.predictions.len(), 8);
    }

    #[test]
    fn test_adaboost() {
        let data = mat_from_row_major(
            8,
            2,
            &[
                0.0, 0.0, 0.5, 0.5, 0.1, 0.2, 0.3, 0.1, 5.0, 5.0, 5.5, 5.5, 5.1, 5.2, 5.3, 5.1,
            ],
        );
        let labels = vec![0, 0, 0, 0, 1, 1, 1, 1];
        let result = adaboost(&data, &labels, 10, 1.0).unwrap();
        assert_eq!(result.predictions.len(), 8);
    }
}
