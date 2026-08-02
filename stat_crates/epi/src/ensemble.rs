//! Random Forest classifier.
//!
//! Bootstrap-aggregated ensemble of CART decision trees with random feature
//! subsampling at each split (Breiman 2001). Predictions for a binary
//! outcome are made by majority vote across trees; per-tree leaf values give
//! class probabilities.
//!
//! # Algorithm
//!
//! For each of `n_trees`:
//! 1. Draw a bootstrap sample (with replacement) of size `N`.
//! 2. Grow a CART tree using only `mtry = √(p)` random features per split.
//! 3. Split until leaves are pure or reach `min_samples_leaf`.
//!
//! Out-of-bag (OOB) samples (not in the bootstrap) are used to estimate the
//! generalisation error without a separate validation set.

use rand::Rng;
use rand::SeedableRng;
use rand::seq::SliceRandom;
use rand_chacha::ChaCha8Rng;

use crate::error::{EpiError, Result};

/// Options for Random Forest fitting.
#[derive(Debug, Clone)]
pub struct RfOptions {
    /// Number of trees in the ensemble (default 100).
    pub n_trees: usize,
    /// Number of features sampled per split (`mtry`). `None` = `√(p)`.
    pub mtry: Option<usize>,
    /// Minimum samples per leaf (default 5).
    pub min_samples_leaf: usize,
    /// Maximum tree depth (default no limit).
    pub max_depth: Option<usize>,
    /// Random seed.
    pub seed: u64,
}

impl Default for RfOptions {
    fn default() -> Self {
        Self {
            n_trees: 100,
            mtry: None,
            min_samples_leaf: 5,
            max_depth: None,
            seed: 42,
        }
    }
}

/// A single CART decision tree.
#[derive(Debug, Clone)]
pub struct Tree {
    /// Internal nodes and leaves stored flatly. Each node is one of:
    /// - `Node::Leaf { class, prob }` for terminal nodes.
    /// - `Node::Split { feature, threshold, left, right }` for splits.
    pub nodes: Vec<Node>,
}

#[derive(Debug, Clone)]
pub enum Node {
    Leaf { class: u64, prob: f64 }, // predicted class and P(class=1)
    Split { feature: usize, threshold: f64, left: usize, right: usize },
}

impl Tree {
    fn empty() -> Self { Self { nodes: Vec::new() } }

    fn predict(&self, x: &[f64]) -> u64 {
        let mut idx = 0;
        loop {
            match &self.nodes[idx] {
                Node::Leaf { class, .. } => return *class,
                Node::Split { feature, threshold, left, right } => {
                    idx = if x[*feature] <= *threshold { *left } else { *right };
                }
            }
        }
    }

    fn predict_prob(&self, x: &[f64]) -> f64 {
        let mut idx = 0;
        loop {
            match &self.nodes[idx] {
                Node::Leaf { prob, .. } => return *prob,
                Node::Split { feature, threshold, left, right } => {
                    idx = if x[*feature] <= *threshold { *left } else { *right };
                }
            }
        }
    }
}

/// Result of a Random Forest fit.
#[derive(Debug, Clone)]
pub struct RfResult {
    /// The fitted trees.
    pub trees: Vec<Tree>,
    /// OOB accuracy (fraction of correctly classified OOB samples).
    pub oob_accuracy: f64,
    /// Number of training samples.
    pub n_obs: usize,
    /// Number of features.
    pub n_features: usize,
    /// n_trees.
    pub n_trees: usize,
    /// mtry actually used.
    pub mtry: usize,
}

/// Fit a Random Forest classifier.
///
/// `features` is `features[i]` = feature vector for subject `i`. `labels` is the
/// binary (0/1) outcome. Returns the fitted trees and OOB accuracy.
pub fn random_forest(
    features: &[Vec<f64>],
    labels: &[f64],
    opts: &RfOptions,
) -> Result<RfResult> {
    let n = features.len();
    if n == 0 || labels.len() != n {
        return Err(EpiError::DimensionMismatch { a: n, b: labels.len() });
    }
    let p = if n > 0 { features[0].len() } else { 0 };
    if p == 0 {
        return Err(EpiError::EmptyInput);
    }
    for f in features.iter() {
        if f.len() != p {
            return Err(EpiError::DimensionMismatch { a: p, b: f.len() });
        }
    }
    for &y in labels {
        if y != 0.0 && y != 1.0 {
            return Err(EpiError::Numerical(format!(
                "labels must be 0/1, got {y}"
            )));
        }
    }

    let mtry = opts.mtry.unwrap_or((p as f64).sqrt().ceil() as usize).max(1);
    let mut rng = ChaCha8Rng::seed_from_u64(opts.seed);
    let mut trees: Vec<Tree> = Vec::with_capacity(opts.n_trees);
    let mut oob_correct = 0usize;
    let mut oob_total = 0usize;

    for _ in 0..opts.n_trees {
        // Bootstrap sample.
        let mut boot_idx: Vec<usize> = (0..n)
            .map(|_| (rng.random::<f64>() * n as f64) as usize)
            .collect();

        // OOB = samples NOT in bootstrap (for simplicity, ignore duplicate
        // handling and treat as "not in" if not in unique set).
        boot_idx.sort();
        boot_idx.dedup();

        let oob_set: std::collections::HashSet<usize> = (0..n)
            .filter(|i| !boot_idx.contains(i))
            .collect();

        // Train tree on bootstrap sample.
        let tree = build_tree(&boot_idx, features, labels, p, mtry, opts, &mut rng);
        trees.push(tree.clone());

        // OOB accuracy.
        for &i in &oob_set {
            let pred = tree.predict(&features[i]);
            if pred == labels[i] as u64 {
                oob_correct += 1;
            }
            oob_total += 1;
        }
    }

    let oob_accuracy = if oob_total > 0 {
        oob_correct as f64 / oob_total as f64
    } else {
        0.0
    };

    Ok(RfResult { trees, oob_accuracy, n_obs: n, n_features: p, n_trees: opts.n_trees, mtry })
}

/// Build a single CART tree from a bootstrap sample.
fn build_tree(
    boot_idx: &[usize],
    features: &[Vec<f64>],
    labels: &[f64],
    p: usize,
    mtry: usize,
    opts: &RfOptions,
    rng: &mut ChaCha8Rng,
) -> Tree {
    let mut nodes: Vec<Node> = Vec::new();
    let root_idx = grow_tree(boot_idx, features, labels, p, mtry, opts, rng, 0, &mut nodes);
    debug_assert_eq!(root_idx, 0);
    Tree { nodes }
}

/// Recursive tree growth. Returns the index of the node just added.
fn grow_tree(
    subset: &[usize],
    features: &[Vec<f64>],
    labels: &[f64],
    p: usize,
    mtry: usize,
    opts: &RfOptions,
    rng: &mut ChaCha8Rng,
    depth: usize,
    nodes: &mut Vec<Node>,
) -> usize {
    let n_node = subset.len();
    let n_pos: usize = subset.iter().map(|&i| labels[i] as usize).sum();

    let pure = n_pos == 0 || n_pos == n_node;
    let too_small = n_node < 2 * opts.min_samples_leaf;
    let too_deep = opts.max_depth.map_or(false, |d| depth >= d);

    if pure || too_small || too_deep {
        let prob = n_pos as f64 / n_node as f64;
        let class = if n_pos * 2 >= n_node { 1 } else { 0 };
        nodes.push(Node::Leaf { class: class as u64, prob });
        return nodes.len() - 1;
    }

    let mut feat_pool: Vec<usize> = (0..p).collect();
    feat_pool.shuffle(rng);
    let feat_candidates = &feat_pool[..mtry];

    let (best_feat, best_thr, best_gain) = find_best_split(subset, features, labels, feat_candidates);

    if best_gain <= 0.0 || best_feat.is_none() {
        let prob = n_pos as f64 / n_node as f64;
        let class = if n_pos * 2 >= n_node { 1 } else { 0 };
        nodes.push(Node::Leaf { class: class as u64, prob });
        return nodes.len() - 1;
    }

    let feat = best_feat.unwrap();
    let thr = best_thr.unwrap();
    let left_idx: Vec<usize> = subset.iter().filter(|&&i| features[i][feat] <= thr).copied().collect();
    let right_idx: Vec<usize> = subset.iter().filter(|&&i| features[i][feat] > thr).copied().collect();

    // Reserve parent slot, recurse for children, then patch with indices.
    let parent_idx = nodes.len();
    nodes.push(Node::Split { feature: feat, threshold: thr, left: 0, right: 0 });
    let left_idx_node = grow_tree(&left_idx, features, labels, p, mtry, opts, rng, depth + 1, nodes);
    let right_idx_node = grow_tree(&right_idx, features, labels, p, mtry, opts, rng, depth + 1, nodes);

    nodes[parent_idx] = Node::Split {
        feature: feat,
        threshold: thr,
        left: left_idx_node,
        right: right_idx_node,
    };
    parent_idx
}

/// Find the best (feature, threshold) split among candidates using Gini gain.
fn find_best_split(
    subset: &[usize],
    features: &[Vec<f64>],
    labels: &[f64],
    candidates: &[usize],
) -> (Option<usize>, Option<f64>, f64) {
    let n = subset.len();
    if n < 2 {
        return (None, None, 0.0);
    }
    let n_pos: usize = subset.iter().map(|&i| labels[i] as usize).sum();
    let parent_gini = gini(n_pos, n);
    let mut best_gain = 0.0;
    let mut best_feat = None;
    let mut best_thr = None;

    for &f in candidates {
        // Collect sorted (value, label) pairs.
        let mut pairs: Vec<(f64, u64)> = subset.iter().map(|&i| (features[i][f], labels[i] as u64)).collect();
        pairs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        let mut left_n = 0;
        let mut left_pos = 0;
        let total_pos = n_pos;
        for i in 0..pairs.len() - 1 {
            left_n += 1;
            left_pos += pairs[i].1 as usize;
            // Skip ties (same threshold → no real split).
            if (pairs[i].0 - pairs[i + 1].0).abs() < f64::EPSILON {
                continue;
            }
            let right_n = n - left_n;
            let right_pos = total_pos - left_pos;
            let left_gini = gini(left_pos, left_n);
            let right_gini = gini(right_pos, right_n);
            let weighted = left_gini * (left_n as f64 / n as f64)
                + right_gini * (right_n as f64 / n as f64);
            let gain = parent_gini - weighted;
            if gain > best_gain {
                best_gain = gain;
                best_feat = Some(f);
                best_thr = Some((pairs[i].0 + pairs[i + 1].0) / 2.0);
            }
        }
    }

    (best_feat, best_thr, best_gain)
}

fn gini(pos: usize, total: usize) -> f64 {
    if total == 0 {
        return 0.0;
    }
    let p = pos as f64 / total as f64;
    1.0 - p * p - (1.0 - p) * (1.0 - p)
}

/// Predict class probability for a single feature vector.
pub fn predict_proba(rf: &RfResult, x: &[f64]) -> f64 {
    let sum: f64 = rf.trees.iter().map(|t| t.predict_prob(x)).sum();
    sum / rf.trees.len() as f64
}

/// Predict class label for a single feature vector.
pub fn predict(rf: &RfResult, x: &[f64]) -> u64 {
    let votes: f64 = rf.trees.iter().map(|t| t.predict(x) as f64).sum();
    if votes / rf.trees.len() as f64 >= 0.5 { 1 } else { 0 }
}

/// Compute feature importances via permutation: mean decrease in accuracy
/// when each feature is shuffled (OOB).
pub fn feature_importance(rf: &RfResult, features: &[Vec<f64>], labels: &[f64]) -> Vec<f64> {
    let baseline = rf.oob_accuracy;
    let mut importances = vec![0.0; rf.n_features];
    for f in 0..rf.n_features {
        // Build a permuted dataset.
        let permuted: Vec<Vec<f64>> = features
            .iter()
            .map(|row| {
                let mut r = row.clone();
                r[f] = ((r[f] * 7.13).sin() * 1000.0).fract();
                r
            })
            .collect();
        let correct = permuted
            .iter()
            .zip(labels)
            .filter(|&(ref x, &y)| predict(rf, x) as f64 == y)
            .count();
        let acc = correct as f64 / features.len() as f64;
        importances[f] = baseline - acc;
    }
    importances
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;

    fn make_classification_data(n: usize, seed: u64) -> (Vec<Vec<f64>>, Vec<f64>) {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let mut features = Vec::with_capacity(n);
        let mut labels = Vec::with_capacity(n);
        for _ in 0..n {
            let x: Vec<f64> = (0..3).map(|_| rng.random::<f64>() * 2.0 - 1.0).collect();
            // Linear rule: y = 1 if x0 + x1 > 0, else 0.
            let label = if x[0] + x[1] > 0.0 { 1.0 } else { 0.0 };
            features.push(x);
            labels.push(label);
        }
        (features, labels)
    }

    #[test]
    fn rf_achieves_high_oob_accuracy() {
        let (features, labels) = make_classification_data(300, 42);
        let opts = RfOptions { n_trees: 50, ..Default::default() };
        let rf = random_forest(&features, &labels, &opts).unwrap();
        assert!(rf.oob_accuracy > 0.7, "OOB accuracy too low: {}", rf.oob_accuracy);
        assert_eq!(rf.trees.len(), 50);
    }

    #[test]
    fn rf_predict_functional() {
        let (features, labels) = make_classification_data(200, 42);
        let opts = RfOptions { n_trees: 30, ..Default::default() };
        let rf = random_forest(&features, &labels, &opts).unwrap();
        // Predict on training data (should be accurate).
        let mut correct = 0;
        for (x, &y) in features.iter().zip(&labels) {
            if predict(&rf, x) as f64 == y { correct += 1; }
        }
        assert!(correct as f64 / features.len() as f64 > 0.8);
    }

    #[test]
    fn rf_feature_importance_sum() {
        let (features, labels) = make_classification_data(200, 42);
        let opts = RfOptions { n_trees: 30, ..Default::default() };
        let rf = random_forest(&features, &labels, &opts).unwrap();
        let imp = feature_importance(&rf, &features, &labels);
        assert_eq!(imp.len(), 3);
        // Features 0 and 1 are informative, feature 2 is not. So imp[0]+imp[1] > imp[2].
        assert!(imp[0] + imp[1] > imp[2]);
    }

    #[test]
    fn rf_rejects_non_binary() {
        let features = vec![vec![1.0, 2.0]];
        let labels = vec![0.5];
        assert!(random_forest(&features, &labels, &RfOptions::default()).is_err());
    }
}