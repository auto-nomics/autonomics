//! Anomaly detection — Isolation Forest, LOF, Z-score outlier rule.

use faer::Mat;
use rand::SeedableRng;
use rand::Rng;
use rand_chacha::ChaCha8Rng;
use rand::seq::SliceRandom;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AnomalyError {
    #[error("empty input")]
    Empty,
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, AnomalyError>;

// ═══════════════════════════════════════════════════════════════════════
// Z-score / IQR outlier detection
// ═══════════════════════════════════════════════════════════════════════

pub struct OutlierResult {
    pub is_outlier: Vec<bool>,
    pub scores: Vec<f64>,
}

pub fn zscore_outliers(data: &Mat<f64>, threshold: f64) -> Result<OutlierResult> {
    let (nrows, ncols) = data.shape();
    if nrows == 0 {
        return Err(AnomalyError::Empty);
    }

    // Compute mean & std per column
    let mut means = vec![0.0; ncols];
    let mut stds = vec![0.0; ncols];
    for j in 0..ncols {
        let col: Vec<f64> = (0..nrows).map(|i| data[(i, j)]).collect();
        means[j] = col.iter().sum::<f64>() / nrows as f64;
        let var = col.iter().map(|x| (x - means[j]).powi(2)).sum::<f64>() / nrows as f64;
        stds[j] = var.sqrt();
    }

    // Per-row max z-score
    let scores: Vec<f64> = (0..nrows)
        .map(|i| {
            let z_scores: Vec<f64> = (0..ncols)
                .map(|j| {
                    if stds[j] > 0.0 {
                        ((data[(i, j)] - means[j]) / stds[j]).abs()
                    } else {
                        0.0
                    }
                })
                .collect();
            z_scores.iter().fold(0.0f64, |a, b| a.max(*b))
        })
        .collect();

    let is_outlier: Vec<bool> = scores.iter().map(|&s| s > threshold).collect();

    Ok(OutlierResult { is_outlier, scores })
}

// ═══════════════════════════════════════════════════════════════════════
// Isolation Forest (custom faer — random partition trees)
// ═══════════════════════════════════════════════════════════════════════

pub fn isolation_forest(
    data: &Mat<f64>,
    n_trees: usize,
    max_samples: usize,
    seed: u64,
) -> Result<OutlierResult> {
    let (nrows, ncols) = data.shape();
    if nrows == 0 {
        return Err(AnomalyError::Empty);
    }

    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let max_samples = max_samples.min(nrows);

    // Average path length for unsupervised isolation forest
    let c_n = if max_samples > 2 {
        2.0 * ((max_samples as f64 - 1.0).ln()) - 2.0 * ((max_samples as f64 - 1.0) / max_samples as f64).ln()
    } else if max_samples == 2 {
        1.0
    } else {
        0.0
    };

    // Compute average path length per sample across all trees
    let mut path_lengths = vec![0.0f64; nrows];

    for _ in 0..n_trees {
        // Subsample
        let indices: Vec<usize> = {
            let mut idx: Vec<usize> = (0..nrows).collect();
            idx.shuffle(&mut rng);
            idx.into_iter().take(max_samples).collect()
        };

        // Build isolation tree and compute path lengths for ALL samples
        for i in 0..nrows {
            let point: Vec<f64> = (0..ncols).map(|j| data[(i, j)]).collect();
            let subsample: Vec<Vec<f64>> = indices
                .iter()
                .map(|&idx| (0..ncols).map(|j| data[(idx, j)]).collect())
                .collect();
            path_lengths[i] += isolation_tree_path(&point, &subsample, &mut rng, 0, 8);
        }
    }

    // Average path lengths
    let avg_paths: Vec<f64> = path_lengths.iter().map(|&p| p / n_trees as f64).collect();

    // Anomaly score: s = 2^(-E(h)/c(n))
    let scores: Vec<f64> = avg_paths.iter().map(|&h| 2.0_f64.powf(-h / c_n)).collect();
    let is_outlier: Vec<bool> = scores.iter().map(|&s| s > 0.6).collect();

    Ok(OutlierResult { is_outlier, scores })
}

fn isolation_tree_path(
    point: &[f64],
    data: &[Vec<f64>],
    rng: &mut ChaCha8Rng,
    depth: usize,
    max_depth: usize,
) -> f64 {
    if depth >= max_depth || data.len() <= 1 {
        return depth as f64;
    }

    let ncols = point.len();
    // Pick random feature and split value
    let feature = rng.random_range(0..ncols);
    let col_vals: Vec<f64> = data.iter().map(|row| row[feature]).collect();
    let min_val = col_vals.iter().fold(f64::INFINITY, |a, b| a.min(*b));
    let max_val = col_vals.iter().fold(f64::NEG_INFINITY, |a, b| a.max(*b));

    if min_val == max_val {
        return depth as f64;
    }

    let split = rng.random::<f64>() * (max_val - min_val) + min_val;

    // Go left or right based on point's value
    let go_left = point[feature] < split;
    let subset: Vec<&Vec<f64>> = data.iter().filter(|row| {
        (go_left && row[feature] < split) || (!go_left && row[feature] >= split)
    }).collect();

    if subset.is_empty() {
        return depth as f64;
    }

    let subset_owned: Vec<Vec<f64>> = subset.into_iter().cloned().collect();
    isolation_tree_path(point, &subset_owned, rng, depth + 1, max_depth)
}

// ═══════════════════════════════════════════════════════════════════════
// Local Outlier Factor (LOF) — native faer
// ═══════════════════════════════════════════════════════════════════════

pub fn local_outlier_factor(
    data: &Mat<f64>,
    k: usize,
) -> Result<OutlierResult> {
    let (nrows, ncols) = data.shape();
    if nrows == 0 {
        return Err(AnomalyError::Empty);
    }

    // Compute pairwise distances
    let dist = |i: usize, j: usize| -> f64 {
        (0..ncols).map(|c| (data[(i, c)] - data[(j, c)]).powi(2)).sum::<f64>().sqrt()
    };

    // k-distance and k-nearest neighbors for each point
    let mut k_distances = vec![0.0; nrows];
    let mut knn: Vec<Vec<usize>> = vec![Vec::new(); nrows];

    for i in 0..nrows {
        let mut dists: Vec<(f64, usize)> = (0..nrows)
            .filter(|&j| j != i)
            .map(|j| (dist(i, j), j))
            .collect();
        dists.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let kk = k.min(dists.len());
        knn[i] = dists.iter().take(kk).map(|(_, j)| *j).collect();
        k_distances[i] = dists.get(kk.saturating_sub(1)).map(|(d, _)| *d).unwrap_or(0.0);
    }

    // Local reachability density
    let lrd: Vec<f64> = (0..nrows).map(|i| {
        let sum_reach: f64 = knn[i].iter().map(|&j| k_distances[j].max(dist(i, j))).sum();
        if sum_reach > 0.0 {
            knn[i].len() as f64 / sum_reach
        } else {
            0.0
        }
    }).collect();

    // LOF = average lrd of neighbors / lrd of point
    let scores: Vec<f64> = (0..nrows).map(|i| {
        if lrd[i] > 0.0 && !knn[i].is_empty() {
            let sum_lrd: f64 = knn[i].iter().map(|&j| lrd[j]).sum();
            sum_lrd / (knn[i].len() as f64 * lrd[i])
        } else {
            1.0
        }
    }).collect();

    let is_outlier: Vec<bool> = scores.iter().map(|&s| s > 1.5).collect();

    Ok(OutlierResult { is_outlier, scores })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::mat_from_row_major;

    #[test]
    fn test_zscore_outliers() {
        let data = mat_from_row_major(5, 1, &[1.0, 2.0, 3.0, 4.0, 100.0]);
        let result = zscore_outliers(&data, 1.5).unwrap();
        assert!(result.is_outlier[4]); // 100.0 is an outlier
        assert!(!result.is_outlier[0]);
    }

    #[test]
    fn test_isolation_forest() {
        let data = mat_from_row_major(10, 2,
            &[0.0, 0.0, 0.1, 0.1, 0.2, 0.2, 0.3, 0.3, 0.4, 0.4,
              0.5, 0.5, 0.6, 0.6, 0.7, 0.7, 0.8, 0.8, 50.0, 50.0]);
        let result = isolation_forest(&data, 50, 5, 42).unwrap();
        assert_eq!(result.scores.len(), 10);
        // The outlier (50,50) should have the highest anomaly score
        let max_idx = result.scores.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).map(|(i, _)| i);
        assert_eq!(max_idx, Some(9)); // index of (50,50)
    }

    #[test]
    fn test_lof() {
        let data = mat_from_row_major(6, 2,
            &[0.0, 0.0, 0.1, 0.1, 0.2, 0.2, 0.3, 0.3, 0.4, 0.4, 10.0, 10.0]);
        let result = local_outlier_factor(&data, 3).unwrap();
        assert_eq!(result.scores.len(), 6);
        // The isolated point should have higher LOF
        assert!(result.scores[5] > result.scores[0]);
    }
}
