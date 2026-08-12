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
    use linfa::dataset::{AsTargets, DatasetBase};
    use linfa::traits::{Fit, Predict};
    use linfa_logistic::LogisticRegression;

    let (nrows, _) = data.shape();
    if nrows == 0 {
        return Err(ClassifyError::Empty);
    }
    if labels.len() != nrows {
        return Err(ClassifyError::LabelMismatch {
            labels: labels.len(),
            rows: nrows,
        });
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

    let raw_probs = model.predict_probabilities(dataset.records());

    // linfa-logistic's `predict_probabilities` returns P(positive_class), where
    // "positive_class" is the MORE FREQUENT label in the training data (not
    // necessarily label=1).  When the data is imbalanced (e.g. 18% positive),
    // the more frequent class is `false` (label 0), so raw_probs = P(label=0).
    // We must flip to P(label=1) when the model's positive class is `false`.
    let pos_is_true = model.labels().pos.class; // bool: true = label 1
    let probabilities: Vec<f64> = if pos_is_true {
        raw_probs.to_vec()
    } else {
        raw_probs.iter().map(|&p| 1.0 - p).collect()
    };
    let predictions: Vec<usize> = probabilities
        .iter()
        .map(|&p| if p > 0.5 { 1 } else { 0 })
        .collect();

    Ok(LogisticResult {
        predictions,
        probabilities,
    })
}

// ═══════════════════════════════════════════════════════════════════════
// Gaussian Naive Bayes (linfa-bayes)
// ═══════════════════════════════════════════════════════════════════════

pub struct NbResult {
    pub predictions: Vec<usize>,
    pub probabilities: Vec<f64>,
}

pub fn gaussian_nb(data: &Mat<f64>, labels: &[usize]) -> Result<NbResult> {
    use linfa::dataset::DatasetBase;
    use linfa::traits::{Fit, Predict};
    use linfa_bayes::{GaussianNb, NaiveBayes};

    let (nrows, _) = data.shape();
    if nrows == 0 {
        return Err(ClassifyError::Empty);
    }
    if labels.len() != nrows {
        return Err(ClassifyError::LabelMismatch {
            labels: labels.len(),
            rows: nrows,
        });
    }

    let x = faer_to_ndarray(data);
    let y = ndarray::Array1::from_vec(labels.to_vec());
    let dataset = DatasetBase::new(x, y);

    let model = GaussianNb::params()
        .fit(&dataset)
        .map_err(|e| ClassifyError::Linfa(e.to_string()))?;

    let predicted = model.predict(dataset.records());
    let predictions: Vec<usize> = predicted.iter().copied().collect();

    // Compute posterior probabilities P(class=1|x) via predict_proba
    let (proba_matrix, classes) = model.predict_proba(dataset.records().view());
    // classes is sorted; find column for class 1
    let class1_col = classes
        .iter()
        .position(|&c| *c == 1usize)
        .unwrap_or_else(|| classes.len().saturating_sub(1));
    let probabilities: Vec<f64> = proba_matrix.column(class1_col).iter().copied().collect();

    Ok(NbResult {
        predictions,
        probabilities,
    })
}

// ═══════════════════════════════════════════════════════════════════════
// KNN (native — distance-weighted voting)
// ═══════════════════════════════════════════════════════════════════════

pub struct KnnResult {
    pub predictions: Vec<usize>,
    pub probabilities: Vec<f64>,
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
        return Err(ClassifyError::LabelMismatch {
            labels: train_labels.len(),
            rows: n_train,
        });
    }

    let mut predictions = Vec::with_capacity(n_test);
    let mut probabilities = Vec::with_capacity(n_test);

    for i in 0..n_test {
        let test_point: Vec<f64> = (0..n_features).map(|j| test_data[(i, j)]).collect();
        // Compute distances to all training points
        let mut dists: Vec<(f64, usize)> = (0..n_train)
            .map(|t| {
                let train_point: Vec<f64> =
                    (0..n_features).map(|j| train_data[(t, j)]).collect();
                let d: f64 = test_point
                    .iter()
                    .zip(&train_point)
                    .map(|(a, b)| (a - b).powi(2))
                    .sum::<f64>()
                    .sqrt();
                (d, train_labels[t])
            })
            .collect();
        dists.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        // Majority vote among k nearest
        let kk = k.min(dists.len());
        let mut vote_counts: std::collections::HashMap<usize, usize> =
            std::collections::HashMap::new();
        for (_, label) in dists.iter().take(kk) {
            *vote_counts.entry(*label).or_insert(0) += 1;
        }
        let pred = vote_counts
            .into_iter()
            .max_by_key(|(_, c)| *c)
            .map(|(l, _)| l)
            .unwrap_or(0);
        predictions.push(pred);

        // Probability = fraction of class-1 neighbors among k nearest
        let n_pos: usize = dists.iter().take(kk).filter(|(_, l)| *l != 0).count();
        probabilities.push(n_pos as f64 / kk as f64);
    }

    Ok(KnnResult {
        predictions,
        probabilities,
    })
}

// ═══════════════════════════════════════════════════════════════════════
// Decision Tree (linfa-trees)
// ═══════════════════════════════════════════════════════════════════════

pub struct DecisionTreeResult {
    pub predictions: Vec<usize>,
    pub probabilities: Vec<f64>,
}

/// Traverse the tree to the leaf for a single sample.
fn dt_traverse_to_leaf<'a>(
    sample: &[f64],
    node: &'a linfa_trees::TreeNode<f64, usize>,
) -> &'a linfa_trees::TreeNode<f64, usize> {
    if node.is_leaf() {
        node
    } else {
        let (feat, threshold, _) = node.split();
        let children = node.children();
        // linfa-trees convention: feature < threshold → left, else → right
        if sample[feat] < threshold {
            dt_traverse_to_leaf(sample, children[0].as_ref().unwrap())
        } else {
            dt_traverse_to_leaf(sample, children[1].as_ref().unwrap())
        }
    }
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

    let (nrows, ncols) = data.shape();
    if nrows == 0 {
        return Err(ClassifyError::Empty);
    }
    if labels.len() != nrows {
        return Err(ClassifyError::LabelMismatch {
            labels: labels.len(),
            rows: nrows,
        });
    }

    let x = faer_to_ndarray(data);
    let y = ndarray::Array1::from_vec(labels.to_vec());
    let dataset = DatasetBase::new(x, y);

    let params = DecisionTree::params().max_depth(Some(max_depth));
    // linfa-trees builder methods
    let model = params
        .fit(&dataset)
        .map_err(|e| ClassifyError::Linfa(e.to_string()))?;

    let predicted = model.predict(dataset.records());
    let predictions: Vec<usize> = predicted.iter().copied().collect();

    // Compute leaf-level class proportions as probability estimates.
    // For each training sample, traverse to its leaf and record class counts;
    // then for each sample's leaf, P(class=1|leaf) = n_pos / (n_pos + n_neg).
    let mut leaf_stats: std::collections::HashMap<usize, (usize, usize)> =
        std::collections::HashMap::new();
    for i in 0..nrows {
        let sample: Vec<f64> = (0..ncols).map(|j| data[(i, j)]).collect();
        let leaf = dt_traverse_to_leaf(&sample, model.root_node());
        let key = leaf as *const _ as usize;
        let entry = leaf_stats.entry(key).or_insert((0, 0));
        if labels[i] != 0 {
            entry.1 += 1;
        } else {
            entry.0 += 1;
        }
    }

    let probabilities: Vec<f64> = (0..nrows)
        .map(|i| {
            let sample: Vec<f64> = (0..ncols).map(|j| data[(i, j)]).collect();
            let leaf = dt_traverse_to_leaf(&sample, model.root_node());
            let key = leaf as *const _ as usize;
            let (n_neg, n_pos) = leaf_stats.get(&key).copied().unwrap_or((0, 0));
            let total = n_neg + n_pos;
            if total == 0 {
                0.5
            } else {
                n_pos as f64 / total as f64
            }
        })
        .collect();

    Ok(DecisionTreeResult {
        predictions,
        probabilities,
    })
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
            &[
                0.0, 0.0, 0.5, 0.5, 0.1, 0.2, 0.3, 0.1, // class 0
                5.0, 5.0, 5.5, 5.5, 5.1, 5.2, 5.3, 5.1,
            ], // class 1
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
        assert_eq!(result.probabilities.len(), 2);
        assert!(result.probabilities[0] < 0.5); // near class 0 cluster
        assert!(result.probabilities[1] > 0.5); // near class 1 cluster
    }

    #[test]
    fn test_gaussian_nb() {
        let (data, labels) = make_binary_data();
        let result = gaussian_nb(&data, &labels).unwrap();
        assert_eq!(result.predictions, labels);
        assert_eq!(result.probabilities.len(), labels.len());
        assert!(result.probabilities.iter().all(|&p| (0.0..=1.0).contains(&p)));
        // Verify probability direction: class-1 samples should have higher P
        let mean_pos: f64 =
            result.probabilities[4..].iter().sum::<f64>() / 4.0;
        let mean_neg: f64 =
            result.probabilities[..4].iter().sum::<f64>() / 4.0;
        assert!(mean_pos > mean_neg, "P(class=1) should be higher for class-1 samples");
    }

    #[test]
    fn test_logistic() {
        let (data, labels) = make_binary_data();
        let result = logistic_regression(&data, &labels, 0.1, 200).unwrap();
        assert_eq!(result.predictions.len(), 8);
        assert!(result.predictions.iter().all(|&p| p == 0 || p == 1));
        assert!(
            result
                .probabilities
                .iter()
                .all(|&p| (0.0..=1.0).contains(&p))
        );
        // Verify probability direction: class-1 samples should have higher P
        let mean_pos: f64 =
            result.probabilities[4..].iter().sum::<f64>() / 4.0;
        let mean_neg: f64 =
            result.probabilities[..4].iter().sum::<f64>() / 4.0;
        assert!(
            mean_pos > mean_neg,
            "P(class=1) should be higher for class-1 samples, got pos={mean_pos} neg={mean_neg}"
        );
    }

    #[test]
    fn test_logistic_imbalanced() {
        // Imbalanced dataset: 7 class-0, 3 class-1 (30% positive rate).
        // This is the key regression test: linfa-logistic assigns the *more
        // frequent* class as "positive", so without the fix,
        // predict_probabilities returns P(class=0) and both predictions and
        // probabilities are inverted.
        let data = mat_from_row_major(
            10,
            2,
            &[
                0.0, 0.0, 0.5, 0.5, 0.1, 0.2, 0.3, 0.1, // 4 × class 0
                1.0, 1.0, 1.5, 1.5, 1.1, 1.2, 1.3, 1.1, // 3 × class 0
                8.0, 8.0, 9.0, 9.0, 8.5, 8.5,           // 3 × class 1 (far)
            ],
        );
        let labels = vec![0, 0, 0, 0, 0, 0, 0, 1, 1, 1];
        let result = logistic_regression(&data, &labels, 0.1, 200).unwrap();

        // Class-1 samples should get higher P(class=1) than class-0 samples
        let mean_pos: f64 = result.probabilities[7..].iter().sum::<f64>() / 3.0;
        let mean_neg: f64 = result.probabilities[..7].iter().sum::<f64>() / 7.0;
        assert!(
            mean_pos > mean_neg,
            "P(class=1) should be higher for class-1 samples, got pos={mean_pos} neg={mean_neg}"
        );
        // With the fix, at least the highest-P class-1 sample should be predicted 1
        let max_pos_prob = result.probabilities[7..].iter().cloned().fold(0.0f64, f64::max);
        assert!(
            max_pos_prob > 0.5,
            "at least one class-1 sample should have P > 0.5, got max={max_pos_prob}"
        );
    }
}
