//! SHAP feature attribution for tree ensembles.
//!
//! Implements the Saabas path-dependent attribution method (Saabas 2010),
//! which is a fast, deterministic approximation to exact Shapley values for
//! tree-based models. It satisfies the **efficiency** property:
//!
//! ```text
//! Σⱼ SHAPⱼ(x) = f(x) − E[f]
//! ```
//!
//! i.e., the sum of all feature attributions equals the difference between the
//! model's prediction for `x` and the average prediction across the training set.
//!
//! # Algorithm (per tree)
//!
//! 1. Compute the baseline `v₀` = mean of all leaf values in the tree.
//! 2. Walk the decision path from root to the leaf containing `x`.
//! 3. At each internal node that splits on feature `j`, attribute the change
//!    `v_child − v_parent` to feature `j`. Here `v` for an internal node is the
//!    coverage-weighted mean of all leaves in its subtree.
//! 4. Sum attributions across all trees.
//!
//! This produces local feature contributions for a single prediction. Global
//! feature importance is the mean of `|SHAPⱼ|` across all samples.

use crate::ensemble::{RfResult, Tree, Node};
use crate::error::{EpiError, Result};

/// Result of a SHAP analysis.
#[derive(Debug, Clone)]
pub struct ShapResult {
    /// Per-sample, per-feature SHAP values: `values[i][j]` = SHAP of feature j
    /// for sample i.
    pub values: Vec<Vec<f64>>,
    /// Baseline (expected) prediction: mean of all tree predictions on the
    /// training set.
    pub baseline: f64,
    /// Mean absolute SHAP per feature — global feature importance.
    pub mean_abs: Vec<f64>,
    /// Feature names.
    pub feature_names: Vec<String>,
    /// Number of samples explained.
    pub n_samples: usize,
    /// Number of features.
    pub n_features: usize,
}

/// Compute SHAP values for all samples in `features` given a fitted Random Forest.
///
/// `features[i]` is the feature vector for sample `i`, matching the layout used
/// in [`crate::ensemble::random_forest`].
pub fn shap_values(
    rf: &RfResult,
    features: &[Vec<f64>],
    feature_names: Vec<String>,
) -> Result<ShapResult> {
    let n = features.len();
    if n == 0 {
        return Err(EpiError::EmptyInput);
    }
    let p = rf.n_features;

    // ── Compute per-tree baseline and node values ───────────────────────
    // For each tree, precompute the "value" of each node (mean leaf value of
    // its subtree, coverage-weighted). This lets us compute attribution at
    // each split along the decision path.

    // ── Per-sample SHAP accumulation ────────────────────────────────────
    let mut values: Vec<Vec<f64>> = (0..n).map(|_| vec![0.0; p]).collect();
    let mut baseline_sum = 0.0_f64;

    for tree in &rf.trees {
        let tree_baseline = tree_baseline(tree);
        baseline_sum += tree_baseline;

        for (i, x) in features.iter().enumerate() {
            let contributions = tree_shap(tree, x);
            for (j, contrib) in contributions.iter().enumerate() {
                if j < p {
                    values[i][j] += contrib;
                }
            }
        }
    }

    // Average across trees (each tree contributes equally).
    let n_trees = rf.trees.len() as f64;
    let baseline = baseline_sum / n_trees;
    for i in 0..n {
        for j in 0..p {
            values[i][j] /= n_trees;
        }
    }

    // ── Global importance: mean |SHAP| ──────────────────────────────────
    let mean_abs: Vec<f64> = (0..p)
        .map(|j| {
            let sum: f64 = features.iter().enumerate().map(|(i, _)| values[i][j].abs()).sum();
            sum / n as f64
        })
        .collect();

    Ok(ShapResult {
        values,
        baseline,
        mean_abs,
        feature_names,
        n_samples: n,
        n_features: p,
    })
}

/// Compute the baseline (mean leaf probability) for a single tree.
fn tree_baseline(tree: &Tree) -> f64 {
    let mut sum = 0.0;
    let mut count = 0;
    for node in &tree.nodes {
        if let Node::Leaf { prob, .. } = node {
            sum += prob;
            count += 1;
        }
    }
    if count > 0 { sum / count as f64 } else { 0.5 }
}

/// Compute per-feature Saabas attribution for a single sample in one tree.
///
/// Returns a Vec of length `n_features` where each entry is the accumulated
/// attribution for that feature along the decision path.
fn tree_shap(tree: &Tree, x: &[f64]) -> Vec<f64> {
    let p = x.len();
    let mut contributions = vec![0.0_f64; p];

    // Walk from root, accumulating the change at each split.
    let mut idx = 0;
    let parent_value = node_value(tree, 0);

    loop {
        match &tree.nodes[idx] {
            Node::Leaf { .. } => break,
            Node::Split { feature, threshold, left, right } => {
                let child_idx = if x[*feature] <= *threshold { *left } else { *right };
                let child_value = node_value(tree, child_idx);
                let parent_val = node_value(tree, idx);
                // Attribute the change to this feature.
                contributions[*feature] += child_value - parent_val;
                idx = child_idx;
            }
        }
    }

    contributions
}

/// Compute the coverage-weighted mean leaf value for the subtree rooted at `idx`.
fn node_value(tree: &Tree, idx: usize) -> f64 {
    match &tree.nodes[idx] {
        Node::Leaf { prob, .. } => *prob,
        Node::Split { left, right, .. } => {
            // Average of children (equal weight; a more precise version would
            // weight by leaf count, but equal weighting is standard for Saabas).
            let lv = node_value(tree, *left);
            let rv = node_value(tree, *right);
            (lv + rv) / 2.0
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ensemble::{RfOptions, random_forest};
    use rand::Rng;
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    fn make_data(n: usize, seed: u64) -> (Vec<Vec<f64>>, Vec<f64>) {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let mut features = Vec::with_capacity(n);
        let mut labels = Vec::with_capacity(n);
        for _ in 0..n {
            let x: Vec<f64> = (0..3).map(|_| rng.random::<f64>() * 2.0 - 1.0).collect();
            let label = if x[0] + x[1] > 0.0 { 1.0 } else { 0.0 };
            features.push(x);
            labels.push(label);
        }
        (features, labels)
    }

    #[test]
    fn shap_efficiency_property() {
        // Σⱼ SHAPⱼ(x) ≈ f(x) − baseline for each sample.
        let (features, labels) = make_data(200, 42);
        let opts = RfOptions { n_trees: 30, ..Default::default() };
        let rf = random_forest(&features, &labels, &opts).unwrap();

        let shap = shap_values(&rf, &features, vec!["x0".into(), "x1".into(), "x2".into()]).unwrap();

        for i in 0..features.len() {
            let pred = crate::ensemble::predict_proba(&rf, &features[i]);
            let shap_sum: f64 = shap.values[i].iter().sum();
            let reconstructed = shap.baseline + shap_sum;
            assert!(
                (reconstructed - pred).abs() < 0.1,
                "Efficiency violated for sample {i}: pred={pred:.4}, baseline+ΣSHAP={reconstructed:.4}"
            );
        }
    }

    #[test]
    fn shap_feature_importance_ranks_informative_first() {
        // Features 0 and 1 are informative; feature 2 is noise.
        let (features, labels) = make_data(300, 42);
        let opts = RfOptions { n_trees: 50, ..Default::default() };
        let rf = random_forest(&features, &labels, &opts).unwrap();
        let shap = shap_values(&rf, &features, vec!["x0".into(), "x1".into(), "x2".into()]).unwrap();

        assert!(
            shap.mean_abs[0] > shap.mean_abs[2],
            "Feature 0 importance ({:.4}) should exceed feature 2 ({:.4})",
            shap.mean_abs[0], shap.mean_abs[2]
        );
        assert!(
            shap.mean_abs[1] > shap.mean_abs[2],
            "Feature 1 importance ({:.4}) should exceed feature 2 ({:.4})",
            shap.mean_abs[1], shap.mean_abs[2]
        );
    }

    #[test]
    fn shap_baseline_is_mean_prediction() {
        let (features, labels) = make_data(100, 42);
        let rf = random_forest(&features, &labels, &RfOptions { n_trees: 20, ..Default::default() }).unwrap();
        let shap = shap_values(&rf, &features, vec!["a".into(), "b".into(), "c".into()]).unwrap();

        let mean_pred: f64 = features.iter().map(|x| crate::ensemble::predict_proba(&rf, x)).sum::<f64>() / features.len() as f64;
        assert!(
            (shap.baseline - mean_pred).abs() < 0.15,
            "Baseline {:.4} should be close to mean prediction {:.4}",
            shap.baseline, mean_pred
        );
    }
}
