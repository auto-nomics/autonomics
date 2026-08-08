//! Data splitting — train/test split and K-Fold cross-validation indices.
//!
//! Deterministic via `ChaCha8Rng + seed`. Supports stratified splitting
//! (preserve class proportions) and group-based splitting (all rows of a
//! group stay in the same fold).

use rand::SeedableRng;
use rand::seq::SliceRandom;
use rand_chacha::ChaCha8Rng;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SplitError {
    #[error("test_size must be in (0, 1), got {0}")]
    InvalidTestSize(f64),
    #[error("not enough samples ({n}) for {k} folds")]
    TooFewSamples { n: usize, k: usize },
    #[error("stratify column has a single class; cannot stratify")]
    SingleClass,
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, SplitError>;

/// Indices for a single train/test split.
#[derive(Debug, Clone)]
pub struct TrainTestSplit {
    pub train_indices: Vec<usize>,
    pub test_indices: Vec<usize>,
}

/// Perform a train/test split.
///
/// - `n`: total number of samples.
/// - `test_size`: fraction in (0, 1).
/// - `stratify`: optional class labels per sample; when provided, each class
///   is split independently to preserve proportions.
/// - `seed`: RNG seed.
pub fn train_test_split(
    n: usize,
    test_size: f64,
    stratify: Option<&[usize]>,
    seed: u64,
) -> Result<TrainTestSplit> {
    if !(0.0..=1.0).contains(&test_size) || test_size == 0.0 {
        return Err(SplitError::InvalidTestSize(test_size));
    }
    let mut rng = ChaCha8Rng::seed_from_u64(seed);

    let (train_idx, test_idx) = match stratify {
        None => {
            let mut indices: Vec<usize> = (0..n).collect();
            indices.shuffle(&mut rng);
            let split_at = ((n as f64) * (1.0 - test_size)).round() as usize;
            let test = indices.split_off(split_at);
            (indices, test)
        }
        Some(labels) => {
            let mut classes: std::collections::HashMap<usize, Vec<usize>> =
                std::collections::HashMap::new();
            for (i, &label) in labels.iter().enumerate() {
                classes.entry(label).or_default().push(i);
            }
            if classes.len() < 2 {
                return Err(SplitError::SingleClass);
            }
            let mut train = Vec::new();
            let mut test = Vec::new();
            for (_, mut indices) in classes {
                indices.shuffle(&mut rng);
                let split_at =
                    ((indices.len() as f64) * (1.0 - test_size)).round() as usize;
                let mut rest = indices.split_off(split_at);
                train.extend(indices);
                test.append(&mut rest);
            }
            train.sort_unstable();
            test.sort_unstable();
            (train, test)
        }
    };

    Ok(TrainTestSplit {
        train_indices: train_idx,
        test_indices: test_idx,
    })
}

/// A single fold: (train_indices, test_indices).
pub type Fold = (Vec<usize>, Vec<usize>);

/// Generate K-Fold cross-validation folds.
///
/// - `n`: total samples.
/// - `k`: number of folds.
/// - `shuffle`: whether to shuffle before splitting.
/// - `seed`: RNG seed.
pub fn kfold(n: usize, k: usize, shuffle: bool, seed: u64) -> Result<Vec<Fold>> {
    if k < 2 {
        return Err(SplitError::TooFewSamples { n, k });
    }
    if n < k {
        return Err(SplitError::TooFewSamples { n, k });
    }
    let mut indices: Vec<usize> = (0..n).collect();
    if shuffle {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        indices.shuffle(&mut rng);
    }
    let fold_size = n / k;
    let remainder = n % k;
    let mut folds = Vec::with_capacity(k);
    let mut start = 0;
    for i in 0..k {
        let size = fold_size + if i < remainder { 1 } else { 0 };
        let end = start + size;
        let test = indices[start..end].to_vec();
        let train: Vec<usize> = indices[..start]
            .iter()
            .chain(indices[end..].iter())
            .copied()
            .collect();
        folds.push((train, test));
        start = end;
    }
    Ok(folds)
}

/// Generate stratified K-Fold cross-validation folds.
///
/// Each fold preserves the class proportions from `labels`.
pub fn stratified_kfold(
    labels: &[usize],
    k: usize,
    shuffle: bool,
    seed: u64,
) -> Result<Vec<Fold>> {
    let n = labels.len();
    if k < 2 || n < k {
        return Err(SplitError::TooFewSamples { n, k });
    }
    let mut rng = ChaCha8Rng::seed_from_u64(seed);

    // Group sample indices by class.
    let mut classes: std::collections::HashMap<usize, Vec<usize>> =
        std::collections::HashMap::new();
    for (i, &label) in labels.iter().enumerate() {
        classes.entry(label).or_default().push(i);
    }
    if classes.len() < 2 {
        return Err(SplitError::SingleClass);
    }

    // Shuffle within classes and distribute round-robin across folds.
    let mut fold_test: Vec<Vec<usize>> = vec![Vec::new(); k];
    for (_, indices) in classes.iter_mut() {
        if shuffle {
            indices.shuffle(&mut rng);
        }
        for (fold_idx, &sample) in indices.iter().enumerate() {
            fold_test[fold_idx % k].push(sample);
        }
    }

    let all_set: std::collections::HashSet<usize> = (0..n).collect();
    let mut folds = Vec::with_capacity(k);
    for test in &mut fold_test {
        test.sort_unstable();
        let test_set: std::collections::HashSet<usize> = test.iter().copied().collect();
        let train: Vec<usize> = all_set
            .iter()
            .filter(|&&i| !test_set.contains(&i))
            .copied()
            .collect();
        folds.push((train, test.clone()));
    }
    Ok(folds)
}

/// Generate group K-Fold folds: every row of the same group goes to the
/// same fold, ensuring no leakage.
pub fn group_kfold(groups: &[usize], k: usize) -> Result<Vec<Fold>> {
    let n = groups.len();
    if k < 2 {
        return Err(SplitError::TooFewSamples { n, k });
    }
    let mut group_members: std::collections::HashMap<usize, Vec<usize>> =
        std::collections::HashMap::new();
    for (i, &g) in groups.iter().enumerate() {
        group_members.entry(g).or_default().push(i);
    }
    let n_groups = group_members.len();
    if n_groups < k {
        return Err(SplitError::Other(format!(
            "need ≥{k} groups for group_kfold, got {n_groups}"
        )));
    }
    // Assign groups to folds round-robin (simple, deterministic).
    let sorted_groups: Vec<usize> = {
        let mut g: Vec<usize> = group_members.keys().copied().collect();
        g.sort_unstable();
        g
    };
    let mut fold_test: Vec<Vec<usize>> = vec![Vec::new(); k];
    for (fold_idx, &g) in sorted_groups.iter().enumerate() {
        fold_test[fold_idx % k].extend(group_members[&g].iter().copied());
    }
    let all_set: std::collections::HashSet<usize> = (0..n).collect();
    let mut folds = Vec::with_capacity(k);
    for test in &mut fold_test {
        test.sort_unstable();
        let test_set: std::collections::HashSet<usize> = test.iter().copied().collect();
        let train: Vec<usize> = all_set
            .iter()
            .filter(|&&i| !test_set.contains(&i))
            .copied()
            .collect();
        folds.push((train, test.clone()));
    }
    Ok(folds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_basic() {
        let s = train_test_split(100, 0.2, None, 42).unwrap();
        assert_eq!(s.train_indices.len(), 80);
        assert_eq!(s.test_indices.len(), 20);
        // No overlap
        let train_set: std::collections::HashSet<_> = s.train_indices.iter().collect();
        assert!(s.test_indices.iter().all(|i| !train_set.contains(i)));
    }

    #[test]
    fn split_stratified() {
        let labels: Vec<usize> = (0..100).map(|i| i % 2).collect();
        let s = train_test_split(100, 0.3, Some(&labels), 42).unwrap();
        // Each class should be ~70/30 split
        let train_class0 = s.train_indices.iter().filter(|&&i| labels[i] == 0).count();
        let test_class0 = s.test_indices.iter().filter(|&&i| labels[i] == 0).count();
        assert_eq!(train_class0 + test_class0, 50);
        assert!((10..=20).contains(&test_class0)); // ~30% of 50
    }

    #[test]
    fn kfold_basic() {
        let folds = kfold(20, 5, true, 42).unwrap();
        assert_eq!(folds.len(), 5);
        for (train, test) in &folds {
            assert_eq!(test.len(), 4);
            assert_eq!(train.len(), 16);
        }
    }

    #[test]
    fn stratified_kfold_balanced() {
        let labels: Vec<usize> = (0..100).map(|i| if i < 30 { 0 } else { 1 }).collect();
        let folds = stratified_kfold(&labels, 5, true, 42).unwrap();
        for (_, test) in &folds {
            let class0 = test.iter().filter(|&&i| labels[i] == 0).count();
            assert_eq!(class0, 6); // 30/5 = 6
        }
    }

    #[test]
    fn group_kfold_no_leakage() {
        let groups: Vec<usize> = (0..50).map(|i| i / 10).collect(); // 5 groups
        let folds = group_kfold(&groups, 5).unwrap();
        for (train, test) in &folds {
            let test_groups: std::collections::HashSet<usize> =
                test.iter().map(|&i| groups[i]).collect();
            let train_groups: std::collections::HashSet<usize> =
                train.iter().map(|&i| groups[i]).collect();
            assert!(test_groups.is_disjoint(&train_groups));
        }
    }
}
