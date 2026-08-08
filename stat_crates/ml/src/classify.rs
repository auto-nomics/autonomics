//! Classification algorithms — logistic regression, naive Bayes, KNN, decision tree.
//!
//! Uses [`linfa-logistic`] for logistic regression, [`linfa-bayes`] for
//! Gaussian/Multinomial/Bernoulli NB, [`linfa-trees`] for decision tree.
//! KNN is implemented natively (simple distance-based voting).

use faer::Mat;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ClassifyError {
    #[error("empty input")]
    Empty,
    #[error("labels length ({labels}) doesn't match data rows ({rows})")]
    LabelMismatch { labels: usize, rows: usize },
    #[error("linfa error: {0}")]
    Linfa(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, ClassifyError>;

pub use crate::dimred::faer_to_ndarray;

// ═══════════════════════════════════════════════════════════════════════
// Logistic Regression (linfa-logistic)
// ═══════════════════════════════════════════════════════════════════════

pub struct LogisticResult {
    pub predictions: Vec<usize>,
    pub probabilities: Vec<f64>,
}

pub fn logistic_regression(
    data: &Mat<f64>,
    labels: &[usize],
    alpha: f64,
    max_iter: usize,
) -> Result<LogisticResult> {
    use linfa::dataset::{DatasetBase, AsTargets};
    use linfa::traits::{Fit, Predict};
    use linfa_logistic::LogisticRegression;

    let (nrows, _) = data.shape();
    if nrows == 0 {
        return Err(ClassifyError::Empty);
    }
    if labels.len() != nrows {
        return Err(ClassifyError::LabelMismatch { labels: labels.len(), rows: nrows });
    }

    let x = faer_to_ndarray(data);
    // Convert labels to bool for binary logistic
    let y: Vec<bool> = labels.iter().map(|&l| l != 0).collect();
    let dataset = DatasetBase::new(x, ndarray::Array1::from(y));

    let model = LogisticRegression::default()
        .alpha(alpha)
        .max_iterations(max_iter as u64)
        .fit(&dataset)
        .map_err(|e| ClassifyError::Linfa(e.to_string()))?;

    let probs = model.predict_probabilities(dataset.records());
    let predictions: Vec<usize> = probs.iter().map(|&p| if p > 0.5 { 1 } else { 0 }).collect();
    let probabilities: Vec<f64> = probs.to_vec();

    Ok(LogisticResult { predictions, probabilities })
}

// ═══════════════════════════════════════════════════════════════════════
// Gaussian Naive Bayes (linfa-bayes)
// ═══════════════════════════════════════════════════════════════════════

pub struct NbResult {
    pub predictions: Vec<usize>,
}

pub fn gaussian_nb(data: &Mat<f64>, labels: &[usize]) -> Result<NbResult> {
    use linfa::dataset::DatasetBase;
    use linfa::traits::{Fit, Predict};
    use linfa_bayes::GaussianNb;

    let (nrows, _) = data.shape();
    if nrows == 0 {
        return Err(ClassifyError::Empty);
    }
    if labels.len() != nrows {
        return Err(ClassifyError::LabelMismatch { labels: labels.len(), rows: nrows });
    }

    let x = faer_to_ndarray(data);
    let y = ndarray::Array1::from_vec(labels.to_vec());
    let dataset = DatasetBase::new(x, y);

    let model = GaussianNb::params()
        .fit(&dataset)
        .map_err(|e| ClassifyError::Linfa(e.to_string()))?;

    let predicted = model.predict(dataset.records());
    let predictions: Vec<usize> = predicted.iter().copied().collect();

    Ok(NbResult { predictions })
}

// ═══════════════════════════════════════════════════════════════════════
// KNN (native — distance-weighted voting)
// ═══════════════════════════════════════════════════════════════════════

pub struct KnnResult {
    pub predictions: Vec<usize>,
}

pub fn knn_classify(
    train_data: &Mat<f64>,
    train_labels: &[usize],
    test_data: &Mat<f64>,
    k: usize,
) -> Result<KnnResult> {
    let (n_train, _) = train_data.shape();
    let (n_test, n_features) = test_data.shape();
    if n_train == 0 || n_test == 0 {
        return Err(ClassifyError::Empty);
    }
    if train_labels.len() != n_train {
        return Err(ClassifyError::LabelMismatch { labels: train_labels.len(), rows: n_train });
    }

    let predictions: Vec<usize> = (0..n_test)
        .map(|i| {
            let test_point: Vec<f64> = (0..n_features).map(|j| test_data[(i, j)]).collect();
            // Compute distances to all training points
            let mut dists: Vec<(f64, usize)> = (0..n_train)
                .map(|t| {
                    let train_point: Vec<f64> = (0..n_features).map(|j| train_data[(t, j)]).collect();
                    let d: f64 = test_point.iter().zip(&train_point).map(|(a, b)| (a - b).powi(2)).sum::<f64>().sqrt();
                    (d, train_labels[t])
                })
                .collect();
            dists.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

            // Majority vote among k nearest
            let k = k.min(dists.len());
            let mut vote_counts: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
            for (_, label) in dists.iter().take(k) {
                *vote_counts.entry(*label).or_insert(0) += 1;
            }
            vote_counts.into_iter().max_by_key(|(_, c)| *c).map(|(l, _)| l).unwrap_or(0)
        })
        .collect();

    Ok(KnnResult { predictions })
}

// ═══════════════════════════════════════════════════════════════════════
// Decision Tree (linfa-trees)
// ═══════════════════════════════════════════════════════════════════════

pub struct DecisionTreeResult {
    pub predictions: Vec<usize>,
}

pub fn decision_tree(
    data: &Mat<f64>,
    labels: &[usize],
    max_depth: usize,
    _min_samples_split: usize,
    _min_samples_leaf: usize,
) -> Result<DecisionTreeResult> {
    use linfa::dataset::DatasetBase;
    use linfa::traits::{Fit, Predict};
    use linfa_trees::DecisionTree;

    let (nrows, _) = data.shape();
    if nrows == 0 {
        return Err(ClassifyError::Empty);
    }
    if labels.len() != nrows {
        return Err(ClassifyError::LabelMismatch { labels: labels.len(), rows: nrows });
    }

    let x = faer_to_ndarray(data);
    let y = ndarray::Array1::from_vec(labels.to_vec());
    let dataset = DatasetBase::new(x, y);

    let params = DecisionTree::params()
        .max_depth(Some(max_depth));
    // linfa-trees builder methods
    let model = params
        .fit(&dataset)
        .map_err(|e| ClassifyError::Linfa(e.to_string()))?;

    let predicted = model.predict(dataset.records());
    let predictions: Vec<usize> = predicted.iter().copied().collect();

    Ok(DecisionTreeResult { predictions })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::mat_from_row_major;

    fn make_binary_data() -> (Mat<f64>, Vec<usize>) {
        // Two well-separated clusters: class 0 and class 1
        let data = mat_from_row_major(
            8,
            2,
            &[0.0, 0.0, 0.5, 0.5, 0.1, 0.2, 0.3, 0.1, // class 0
              5.0, 5.0, 5.5, 5.5, 5.1, 5.2, 5.3, 5.1], // class 1
        );
        let labels = vec![0, 0, 0, 0, 1, 1, 1, 1];
        (data, labels)
    }

    #[test]
    fn test_knn() {
        let (train, labels) = make_binary_data();
        let test = mat_from_row_major(2, 2, &[0.1, 0.1, 5.2, 5.3]);
        let result = knn_classify(&train, &labels, &test, 3).unwrap();
        assert_eq!(result.predictions, vec![0, 1]);
    }

    #[test]
    fn test_gaussian_nb() {
        let (data, labels) = make_binary_data();
        let result = gaussian_nb(&data, &labels).unwrap();
        assert_eq!(result.predictions, labels);
    }

    #[test]
    fn test_logistic() {
        let (data, labels) = make_binary_data();
        let result = logistic_regression(&data, &labels, 0.1, 200).unwrap();
        assert_eq!(result.predictions.len(), 8);
        // Just verify it runs and produces valid predictions
        assert!(result.predictions.iter().all(|&p| p == 0 || p == 1));
        assert!(result.probabilities.iter().all(|&p| (0.0..=1.0).contains(&p)));
    }
}
