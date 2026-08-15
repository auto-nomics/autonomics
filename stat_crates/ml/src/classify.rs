//! Classification algorithms — logistic regression, naive Bayes, KNN, decision tree.
//!
//! Uses [`linfa-logistic`] for logistic regression and [`linfa-bayes`] for
//! Gaussian/Multinomial/Bernoulli NB. Decision trees are implemented natively.
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
    logistic_fit_predict(data, labels, data, alpha, max_iter)
}

/// Train logistic regression on `train_data`/`train_labels` and predict on
/// `test_data`.  This enables proper train:test separation.
pub fn logistic_fit_predict(
    train_data: &Mat<f64>,
    train_labels: &[usize],
    test_data: &Mat<f64>,
    alpha: f64,
    max_iter: usize,
) -> Result<LogisticResult> {
    use linfa::dataset::{AsTargets, DatasetBase};
    use linfa::traits::{Fit, Predict};
    use linfa_logistic::LogisticRegression;

    let (nrows, _) = train_data.shape();
    if nrows == 0 {
        return Err(ClassifyError::Empty);
    }
    if train_labels.len() != nrows {
        return Err(ClassifyError::LabelMismatch {
            labels: train_labels.len(),
            rows: nrows,
        });
    }

    let x = faer_to_ndarray(train_data);
    // Convert labels to bool for binary logistic
    let y: Vec<bool> = train_labels.iter().map(|&l| l != 0).collect();
    let dataset = DatasetBase::new(x, ndarray::Array1::from(y));

    let model = LogisticRegression::default()
        .alpha(alpha)
        .max_iterations(max_iter as u64)
        .fit(&dataset)
        .map_err(|e| ClassifyError::Linfa(e.to_string()))?;

    let x_test = faer_to_ndarray(test_data);
    let raw_probs = model.predict_probabilities(&x_test);

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
    gaussian_nb_fit_predict(data, labels, data)
}

/// Train Gaussian NB on `train_data`/`train_labels` and predict on `test_data`.
pub fn gaussian_nb_fit_predict(
    train_data: &Mat<f64>,
    train_labels: &[usize],
    test_data: &Mat<f64>,
) -> Result<NbResult> {
    use linfa::dataset::DatasetBase;
    use linfa::traits::{Fit, Predict};
    use linfa_bayes::{GaussianNb, NaiveBayes};

    let (nrows, _) = train_data.shape();
    if nrows == 0 {
        return Err(ClassifyError::Empty);
    }
    if train_labels.len() != nrows {
        return Err(ClassifyError::LabelMismatch {
            labels: train_labels.len(),
            rows: nrows,
        });
    }

    let x = faer_to_ndarray(train_data);
    let y = ndarray::Array1::from_vec(train_labels.to_vec());
    let dataset = DatasetBase::new(x, y);

    let model = GaussianNb::params()
        .fit(&dataset)
        .map_err(|e| ClassifyError::Linfa(e.to_string()))?;

    let x_test = faer_to_ndarray(test_data);
    let predicted = model.predict(&x_test);
    let predictions: Vec<usize> = predicted.iter().copied().collect();

    // Compute posterior probabilities P(class=1|x) via predict_proba
    let (proba_matrix, classes) = model.predict_proba(x_test.view());
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
                let train_point: Vec<f64> = (0..n_features).map(|j| train_data[(t, j)]).collect();
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
// Decision Tree (CART)
// ═══════════════════════════════════════════════════════════════════════

pub struct DecisionTreeResult {
    pub predictions: Vec<usize>,
    pub probabilities: Vec<f64>,
}

#[derive(Debug)]
enum DecisionTreeModel {
    Leaf {
        prediction: usize,
        class_1_count: usize,
        total: usize,
    },
    Split {
        feature: usize,
        threshold: f64,
        left: Box<DecisionTreeModel>,
        right: Box<DecisionTreeModel>,
    },
}

#[derive(Debug)]
struct LeafSummary {
    prediction: usize,
    class_1_count: usize,
    total: usize,
}

fn count_labels(labels: &[usize], indices: &[usize]) -> std::collections::HashMap<usize, usize> {
    let mut counts = std::collections::HashMap::new();
    for &index in indices {
        *counts.entry(labels[index]).or_insert(0) += 1;
    }
    counts
}

fn gini_impurity(counts: &std::collections::HashMap<usize, usize>) -> f64 {
    let total: usize = counts.values().sum();
    if total == 0 {
        return 0.0;
    }
    let total = total as f64;
    1.0 - counts
        .values()
        .map(|&count| {
            let probability = count as f64 / total;
            probability * probability
        })
        .sum::<f64>()
}

fn leaf_summary(labels: &[usize], indices: &[usize]) -> LeafSummary {
    let counts = count_labels(labels, indices);
    let prediction = counts
        .iter()
        .max_by(|(left_label, left_count), (right_label, right_count)| {
            right_count
                .cmp(left_count)
                .then_with(|| left_label.cmp(right_label))
        })
        .map(|(&label, _)| label)
        .unwrap_or(0);
    LeafSummary {
        prediction,
        class_1_count: counts.get(&1).copied().unwrap_or(0),
        total: indices.len(),
    }
}

fn leaf_from_summary(summary: LeafSummary) -> DecisionTreeModel {
    DecisionTreeModel::Leaf {
        prediction: summary.prediction,
        class_1_count: summary.class_1_count,
        total: summary.total,
    }
}

fn split_threshold(left_value: f64, right_value: f64) -> f64 {
    let midpoint = left_value + (right_value - left_value) / 2.0;
    if midpoint > left_value && midpoint < right_value {
        midpoint
    } else {
        left_value
    }
}

fn fit_decision_tree(
    data: &Mat<f64>,
    labels: &[usize],
    indices: Vec<usize>,
    depth: usize,
    max_depth: usize,
    min_samples_split: usize,
    min_samples_leaf: usize,
) -> DecisionTreeModel {
    let parent_counts = count_labels(labels, &indices);
    let parent_impurity = gini_impurity(&parent_counts);
    let summary = leaf_summary(labels, &indices);
    let min_samples_split = min_samples_split.max(2);
    let min_samples_leaf = min_samples_leaf.max(1);

    if indices.len() < min_samples_split || depth >= max_depth || parent_impurity <= f64::EPSILON {
        return leaf_from_summary(summary);
    }

    let (_, n_features) = data.shape();
    let mut best: Option<(usize, f64, f64)> = None;

    for feature in 0..n_features {
        let mut sorted_indices = indices.clone();
        sorted_indices.sort_by(|&left, &right| {
            data[(left, feature)]
                .partial_cmp(&data[(right, feature)])
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut left_counts = std::collections::HashMap::new();
        let mut right_counts = parent_counts.clone();
        let total = sorted_indices.len();

        for boundary in 0..total - 1 {
            let moved_index = sorted_indices[boundary];
            let label = labels[moved_index];
            *left_counts.entry(label).or_insert(0) += 1;

            if let Some(count) = right_counts.get_mut(&label) {
                *count -= 1;
                if *count == 0 {
                    right_counts.remove(&label);
                }
            }

            let left_value = data[(moved_index, feature)];
            let right_value = data[(sorted_indices[boundary + 1], feature)];
            if left_value == right_value {
                continue;
            }

            let left_n = boundary + 1;
            let right_n = total - left_n;
            if left_n < min_samples_leaf || right_n < min_samples_leaf {
                continue;
            }

            let weighted_impurity = (left_n as f64 * gini_impurity(&left_counts)
                + right_n as f64 * gini_impurity(&right_counts))
                / total as f64;

            best = match best {
                Some((_, _, best_score)) if best_score <= weighted_impurity => best,
                _ => Some((
                    feature,
                    split_threshold(left_value, right_value),
                    weighted_impurity,
                )),
            };
        }
    }

    // Accept zero-gain splits; their children may expose a useful split that
    // the root's immediate score cannot see (for example, XOR).
    if let Some((_, _, weighted_impurity)) = best {
        if weighted_impurity > parent_impurity + 1e-12 {
            best = None;
        }
    }

    let Some((feature, threshold, _)) = best else {
        return leaf_from_summary(summary);
    };

    let left_indices: Vec<usize> = indices
        .iter()
        .copied()
        .filter(|&index| data[(index, feature)] <= threshold)
        .collect();
    let right_indices: Vec<usize> = indices
        .iter()
        .copied()
        .filter(|&index| data[(index, feature)] > threshold)
        .collect();

    DecisionTreeModel::Split {
        feature,
        threshold,
        left: Box::new(fit_decision_tree(
            data,
            labels,
            left_indices,
            depth + 1,
            max_depth,
            min_samples_split,
            min_samples_leaf,
        )),
        right: Box::new(fit_decision_tree(
            data,
            labels,
            right_indices,
            depth + 1,
            max_depth,
            min_samples_split,
            min_samples_leaf,
        )),
    }
}

fn predict_decision_tree(model: &DecisionTreeModel, sample: &[f64]) -> (usize, f64) {
    match model {
        DecisionTreeModel::Leaf {
            prediction,
            class_1_count,
            total,
        } => {
            let probability = if *total == 0 {
                0.5
            } else {
                *class_1_count as f64 / *total as f64
            };
            (*prediction, probability)
        }
        DecisionTreeModel::Split {
            feature,
            threshold,
            left,
            right,
        } if sample[*feature] <= *threshold => predict_decision_tree(left, sample),
        DecisionTreeModel::Split { right, .. } => predict_decision_tree(right, sample),
    }
}

pub fn decision_tree(
    data: &Mat<f64>,
    labels: &[usize],
    max_depth: usize,
    min_samples_split: usize,
    min_samples_leaf: usize,
) -> Result<DecisionTreeResult> {
    decision_tree_fit_predict(
        data,
        labels,
        data,
        max_depth,
        min_samples_split,
        min_samples_leaf,
    )
}

/// Train a decision tree on `train_data`/`train_labels` and predict on
/// `test_data`.  Leaf-level class proportions for probability estimates are
/// computed from the **training** data (as they should be).
pub fn decision_tree_fit_predict(
    train_data: &Mat<f64>,
    train_labels: &[usize],
    test_data: &Mat<f64>,
    max_depth: usize,
    min_samples_split: usize,
    min_samples_leaf: usize,
) -> Result<DecisionTreeResult> {
    let (n_train, n_cols) = train_data.shape();
    let (n_test, n_test_cols) = test_data.shape();
    if n_train == 0 {
        return Err(ClassifyError::Empty);
    }
    if train_labels.len() != n_train {
        return Err(ClassifyError::LabelMismatch {
            labels: train_labels.len(),
            rows: n_train,
        });
    }
    if n_test_cols != n_cols {
        return Err(ClassifyError::Other(format!(
            "train and test feature counts differ: train={n_cols}, test={n_test_cols}"
        )));
    }

    let model = fit_decision_tree(
        train_data,
        train_labels,
        (0..n_train).collect(),
        0,
        max_depth,
        min_samples_split,
        min_samples_leaf,
    );
    let mut predictions = Vec::with_capacity(n_test);
    let mut probabilities = Vec::with_capacity(n_test);
    for row in 0..n_test {
        let sample: Vec<f64> = (0..n_cols).map(|column| test_data[(row, column)]).collect();
        let (prediction, probability) = predict_decision_tree(&model, &sample);
        predictions.push(prediction);
        probabilities.push(probability);
    }

    Ok(DecisionTreeResult {
        predictions,
        probabilities,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::mat_from_row_major;

    /// Train-predict separation: train on one set, predict on a different set
    /// with overlapping but non-identical points. Verifies that the `_fit_predict`
    /// variants produce correct output lengths and probability directions.
    #[test]
    fn test_fit_predict_separation() {
        // Training data: 4 class-0 + 4 class-1 (well separated)
        let train = mat_from_row_major(
            8,
            2,
            &[
                0.0, 0.0, 0.5, 0.5, 0.1, 0.2, 0.3, 0.1, 5.0, 5.0, 5.5, 5.5, 5.1, 5.2, 5.3, 5.1,
            ],
        );
        let labels = vec![0, 0, 0, 0, 1, 1, 1, 1];

        // Test data: 2 new points not in the training set
        let test = mat_from_row_major(2, 2, &[0.2, 0.3, 4.8, 5.2]);

        // Logistic
        let r = logistic_fit_predict(&train, &labels, &test, 0.1, 200).unwrap();
        assert_eq!(r.predictions.len(), 2);
        assert_eq!(r.predictions, vec![0, 1]);
        assert!(r.probabilities[0] < 0.5);
        assert!(r.probabilities[1] > 0.5);

        // Gaussian NB
        let r = gaussian_nb_fit_predict(&train, &labels, &test).unwrap();
        assert_eq!(r.predictions.len(), 2);
        assert_eq!(r.predictions, vec![0, 1]);

        // KNN (already had separate train/test)
        let r = knn_classify(&train, &labels, &test, 3).unwrap();
        assert_eq!(r.predictions, vec![0, 1]);

        // Decision tree
        let r = decision_tree_fit_predict(&train, &labels, &test, 10, 2, 1).unwrap();
        assert_eq!(r.predictions.len(), 2);
        assert_eq!(r.predictions, vec![0, 1]);
    }

    #[test]
    fn test_decision_tree_learns_xor() {
        let train = mat_from_row_major(4, 2, &[0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 1.0]);
        let labels = vec![0, 1, 1, 0];
        let test = mat_from_row_major(5, 2, &[0.2, 0.2, 0.2, 0.8, 0.8, 0.2, 0.8, 0.8, 2.0, 2.0]);

        let result = decision_tree_fit_predict(&train, &labels, &test, 5, 2, 1).unwrap();
        assert_eq!(result.predictions, vec![0, 1, 1, 0, 0]);
        assert_eq!(result.probabilities, vec![0.0, 1.0, 1.0, 0.0, 0.0]);
    }

    #[test]
    fn test_decision_tree_respects_min_samples_leaf() {
        let train = mat_from_row_major(5, 1, &[0.0, 1.0, 2.0, 3.0, 4.0]);
        let labels = vec![0, 0, 0, 0, 1];
        let result = decision_tree_fit_predict(&train, &labels, &train, 5, 2, 2).unwrap();

        assert_eq!(result.predictions, vec![0, 0, 0, 1, 1]);
        assert_eq!(
            result.probabilities,
            vec![0.0, 0.0, 0.0, 0.5, 0.5],
            "a mixed leaf with two samples cannot be split into one-sample leaves"
        );
    }

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
        assert!(
            result
                .probabilities
                .iter()
                .all(|&p| (0.0..=1.0).contains(&p))
        );
        // Verify probability direction: class-1 samples should have higher P
        let mean_pos: f64 = result.probabilities[4..].iter().sum::<f64>() / 4.0;
        let mean_neg: f64 = result.probabilities[..4].iter().sum::<f64>() / 4.0;
        assert!(
            mean_pos > mean_neg,
            "P(class=1) should be higher for class-1 samples"
        );
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
        let mean_pos: f64 = result.probabilities[4..].iter().sum::<f64>() / 4.0;
        let mean_neg: f64 = result.probabilities[..4].iter().sum::<f64>() / 4.0;
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
                8.0, 8.0, 9.0, 9.0, 8.5, 8.5, // 3 × class 1 (far)
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
        let max_pos_prob = result.probabilities[7..]
            .iter()
            .cloned()
            .fold(0.0f64, f64::max);
        assert!(
            max_pos_prob > 0.5,
            "at least one class-1 sample should have P > 0.5, got max={max_pos_prob}"
        );
    }
}
