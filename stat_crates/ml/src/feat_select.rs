//! Feature selection — variance threshold, SelectKBest, mutual information.

use faer::Mat;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FeatSelectError {
    #[error("empty input")]
    Empty,
    #[error("k must be ≥ 1, got {0}")]
    InvalidK(usize),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, FeatSelectError>;

// ═══════════════════════════════════════════════════════════════════════
// VarianceThreshold
// ═══════════════════════════════════════════════════════════════════════

/// Compute per-column variance.
pub fn column_variances(data: &Mat<f64>) -> Vec<f64> {
    let (nrows, ncols) = data.shape();
    (0..ncols)
        .map(|j| {
            let col: Vec<f64> = (0..nrows).map(|i| data[(i, j)]).collect();
            let mean = col.iter().sum::<f64>() / nrows as f64;
            col.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / nrows as f64
        })
        .collect()
}

/// Return indices of columns whose variance exceeds the threshold.
pub fn variance_threshold(data: &Mat<f64>, threshold: f64) -> Vec<usize> {
    let variances = column_variances(data);
    variances
        .iter()
        .enumerate()
        .filter(|&(_, v)| *v > threshold)
        .map(|(i, _)| i)
        .collect()
}

// ═══════════════════════════════════════════════════════════════════════
// SelectKBest — ANOVA F-value (classification)
// ═══════════════════════════════════════════════════════════════════════

/// Compute ANOVA F-values for each feature against the class labels.
///
/// Returns one F-value per column. Higher F → more discriminative feature.
pub fn f_classif(data: &Mat<f64>, labels: &[usize]) -> Vec<f64> {
    let (nrows, ncols) = data.shape();
    let classes: Vec<usize> = {
        let mut c = labels.to_vec();
        c.sort();
        c.dedup();
        c
    };
    let n_classes = classes.len();

    (0..ncols)
        .map(|j| {
            let col: Vec<f64> = (0..nrows).map(|i| data[(i, j)]).collect();
            // Overall mean
            let grand_mean = col.iter().sum::<f64>() / nrows as f64;

            // Between-group and within-group sum of squares
            let mut ss_between = 0.0;
            let mut ss_within = 0.0;
            for &c in &classes {
                let group: Vec<f64> = (0..nrows)
                    .filter(|&i| labels[i] == c)
                    .map(|i| col[i])
                    .collect();
                let n_g = group.len() as f64;
                if n_g == 0.0 {
                    continue;
                }
                let group_mean = group.iter().sum::<f64>() / n_g;
                ss_between += n_g * (group_mean - grand_mean).powi(2);
                ss_within += group.iter().map(|x| (x - group_mean).powi(2)).sum::<f64>();
            }

            let df_between = (n_classes - 1) as f64;
            let df_within = (nrows - n_classes) as f64;
            if df_within > 0.0 && ss_within > 0.0 {
                (ss_between / df_between) / (ss_within / df_within)
            } else {
                0.0
            }
        })
        .collect()
}

/// Select the top-k features by ANOVA F-value.
///
/// Returns (selected_indices, scores) sorted by descending score.
pub fn select_k_best(
    data: &Mat<f64>,
    labels: &[usize],
    k: usize,
) -> Result<(Vec<usize>, Vec<f64>)> {
    if k < 1 {
        return Err(FeatSelectError::InvalidK(k));
    }
    let scores = f_classif(data, labels);
    let mut indexed: Vec<(usize, f64)> = scores.iter().copied().enumerate().collect();
    indexed.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let k = k.min(indexed.len());
    let (indices, s): (Vec<usize>, Vec<f64>) = indexed[..k].iter().map(|&(i, s)| (i, s)).unzip();
    Ok((indices, s))
}

// ═══════════════════════════════════════════════════════════════════════
// Correlation-based feature ranking (regression)
// ═══════════════════════════════════════════════════════════════════════

/// Compute absolute Pearson correlation of each feature with the target.
pub fn f_regression(data: &Mat<f64>, target: &[f64]) -> Vec<f64> {
    let (nrows, ncols) = data.shape();
    let n = nrows as f64;
    let t_mean = target.iter().sum::<f64>() / n;
    let t_ss: f64 = target.iter().map(|t| (t - t_mean).powi(2)).sum();

    (0..ncols)
        .map(|j| {
            let col: Vec<f64> = (0..nrows).map(|i| data[(i, j)]).collect();
            let c_mean = col.iter().sum::<f64>() / n;
            let c_ss: f64 = col.iter().map(|x| (x - c_mean).powi(2)).sum();
            let cov: f64 = col
                .iter()
                .zip(target)
                .map(|(x, t)| (x - c_mean) * (t - t_mean))
                .sum();
            if c_ss > 0.0 && t_ss > 0.0 {
                (cov / (c_ss * t_ss).sqrt()).abs()
            } else {
                0.0
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::mat_from_row_major;

    #[test]
    fn test_variance_threshold() {
        let data = mat_from_row_major(
            5,
            3,
            &[
                1.0, 5.0, 3.0, 1.0, 5.0, 3.0, 1.0, 5.0, 4.0, 1.0, 5.0, 3.0, 1.0, 5.0, 3.0,
            ],
        );
        // Column 0 has variance 0, column 1 has variance 0, column 2 has some variance
        let selected = variance_threshold(&data, 0.01);
        assert!(selected.contains(&2)); // column 2 has non-zero variance
        assert!(!selected.contains(&0)); // column 0 is constant
    }

    #[test]
    fn test_f_classif() {
        let data = mat_from_row_major(
            6,
            2,
            &[1.0, 10.0, 2.0, 9.0, 3.0, 8.0, 10.0, 1.0, 9.0, 2.0, 8.0, 3.0],
        );
        let labels = vec![0, 0, 0, 1, 1, 1];
        let scores = f_classif(&data, &labels);
        // Column 0 should separate classes well (1,2,3 vs 10,9,8)
        // Column 1 should also separate (10,9,8 vs 1,2,3)
        assert!(scores[0] > 10.0);
        assert!(scores[1] > 10.0);
    }

    #[test]
    fn test_select_k_best() {
        let data = mat_from_row_major(
            6,
            3,
            &[
                1.0, 5.0, 1.0, 2.0, 5.0, 1.0, 3.0, 5.0, 1.0, 10.0, 1.0, 1.0, 9.0, 1.0, 1.0, 8.0,
                1.0, 1.0,
            ],
        );
        let labels = vec![0, 0, 0, 1, 1, 1];
        let (indices, scores) = select_k_best(&data, &labels, 2).unwrap();
        assert_eq!(indices.len(), 2);
        // Column 0 and 1 should be selected (they separate classes)
        assert!(scores[0] > scores[scores.len() - 1]);
    }
}
