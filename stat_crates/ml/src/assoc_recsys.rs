//! Association rules + recommendation — Apriori, FP-Growth, ALS collaborative filtering.

use std::collections::{HashMap, HashSet};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AssocError {
    #[error("empty input")]
    Empty,
    #[error("minimum support not met by any itemset")]
    NoFrequent,
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, AssocError>;

// ═══════════════════════════════════════════════════════════════════════
// Apriori — frequent itemset mining
// ═══════════════════════════════════════════════════════════════════════

pub struct FrequentItemset {
    pub items: Vec<String>,
    pub support: f64,
}

pub struct AssociationRule {
    pub antecedent: Vec<String>,
    pub consequent: Vec<String>,
    pub support: f64,
    pub confidence: f64,
    pub lift: f64,
}

pub fn apriori(
    transactions: &[Vec<String>],
    min_support: f64,
    min_confidence: f64,
) -> Result<(Vec<FrequentItemset>, Vec<AssociationRule>)> {
    let n = transactions.len();
    if n == 0 {
        return Err(AssocError::Empty);
    }

    // Collect all unique items
    let all_items: HashSet<&String> = transactions.iter().flatten().collect();
    if all_items.is_empty() {
        return Err(AssocError::NoFrequent);
    }

    // Helper: count support of an itemset
    let count_support = |itemset: &HashSet<&String>| -> usize {
        transactions
            .iter()
            .filter(|t| itemset.iter().all(|item| t.contains(item)))
            .count()
    };

    let min_count = (min_support * n as f64).ceil() as usize;

    // Level 1: single items
    let mut frequent: Vec<FrequentItemset> = Vec::new();
    let mut prev_level: Vec<HashSet<String>> = Vec::new();

    for item in &all_items {
        let itemset: HashSet<&String> = std::iter::once(*item).collect();
        let count = count_support(&itemset);
        if count >= min_count {
            let items: HashSet<String> = std::iter::once((*item).clone()).collect();
            frequent.push(FrequentItemset {
                items: items.iter().cloned().collect(),
                support: count as f64 / n as f64,
            });
            prev_level.push(items);
        }
    }

    // Generate higher levels
    let mut k = 2;
    while !prev_level.is_empty() {
        // Generate candidates by joining
        let mut candidates: Vec<HashSet<String>> = Vec::new();
        for i in 0..prev_level.len() {
            for j in (i + 1)..prev_level.len() {
                let union: HashSet<String> = prev_level[i].union(&prev_level[j]).cloned().collect();
                if union.len() == k && !candidates.contains(&union) {
                    candidates.push(union);
                }
            }
        }

        let mut current_level: Vec<HashSet<String>> = Vec::new();
        for cand in &candidates {
            let itemset_ref: HashSet<&String> = cand.iter().collect();
            let count = count_support(&itemset_ref);
            if count >= min_count {
                frequent.push(FrequentItemset {
                    items: cand.iter().cloned().collect(),
                    support: count as f64 / n as f64,
                });
                current_level.push(cand.clone());
            }
        }
        prev_level = current_level;
        k += 1;
    }

    // Generate rules
    let mut rules: Vec<AssociationRule> = Vec::new();
    for fi in &frequent {
        if fi.items.len() < 2 {
            continue;
        }
        let full: HashSet<&String> = fi.items.iter().collect();
        let full_count = (fi.support * n as f64) as usize;

        // Generate all non-empty proper subsets as antecedents
        let items_vec = &fi.items;
        let n_items = items_vec.len();
        for mask in 1..(1usize << n_items) - 1 {
            let antecedent: HashSet<&String> = items_vec
                .iter()
                .enumerate()
                .filter(|(i, _)| mask & (1 << i) != 0)
                .map(|(_, item)| item)
                .collect();
            let consequent: HashSet<&String> = full.difference(&antecedent).copied().collect();

            let ant_count = count_support(&antecedent);
            if ant_count == 0 {
                continue;
            }
            let confidence = full_count as f64 / ant_count as f64;
            if confidence < min_confidence {
                continue;
            }

            let ant_support = ant_count as f64 / n as f64;
            let con_count = count_support(&consequent);
            let con_support = con_count as f64 / n as f64;
            let lift = fi.support / (ant_support * con_support);

            rules.push(AssociationRule {
                antecedent: antecedent.into_iter().cloned().collect(),
                consequent: consequent.into_iter().cloned().collect(),
                support: fi.support,
                confidence,
                lift,
            });
        }
    }

    Ok((frequent, rules))
}

// ═══════════════════════════════════════════════════════════════════════
// ALS Collaborative Filtering
// ═══════════════════════════════════════════════════════════════════════

pub struct AlsResult {
    pub user_factors: Vec<Vec<f64>>,
    pub item_factors: Vec<Vec<f64>>,
    pub rmse: f64,
}

pub fn als(
    ratings: &[(usize, usize, f64)],
    n_users: usize,
    n_items: usize,
    n_factors: usize,
    n_iter: usize,
    reg: f64,
) -> Result<AlsResult> {
    if ratings.is_empty() {
        return Err(AssocError::Empty);
    }

    use rand::Rng;
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    let mut rng = ChaCha8Rng::seed_from_u64(42);
    let mut u = vec![vec![0.0f64; n_factors]; n_users];
    let mut v = vec![vec![0.0f64; n_factors]; n_items];

    // Initialize randomly
    for i in 0..n_users {
        for k in 0..n_factors {
            u[i][k] = rng.random::<f64>() * 0.1;
        }
    }
    for j in 0..n_items {
        for k in 0..n_factors {
            v[j][k] = rng.random::<f64>() * 0.1;
        }
    }

    // Group ratings by user and item
    let by_user: HashMap<usize, Vec<(usize, f64)>> = {
        let mut m: HashMap<usize, Vec<(usize, f64)>> = HashMap::new();
        for &(user, item, rating) in ratings {
            m.entry(user).or_default().push((item, rating));
        }
        m
    };
    let by_item: HashMap<usize, Vec<(usize, f64)>> = {
        let mut m: HashMap<usize, Vec<(usize, f64)>> = HashMap::new();
        for &(user, item, rating) in ratings {
            m.entry(item).or_default().push((user, rating));
        }
        m
    };

    for _ in 0..n_iter {
        // Update users
        for user in 0..n_users {
            let items: Option<&Vec<(usize, f64)>> = by_user.get(&user);
            if let Some(items) = items {
                // Solve: minimize Σ (r_ui - u_i·v_j)^2 + reg ||u_i||^2
                // Normal equations: (V_j^T V_j + reg I) u_i = V_j^T r
                let mut ata = vec![0.0; n_factors * n_factors];
                let mut atb = vec![0.0; n_factors];
                for &(item, rating) in items {
                    for k in 0..n_factors {
                        atb[k] += v[item][k] * rating;
                        for l in 0..n_factors {
                            ata[k * n_factors + l] += v[item][k] * v[item][l];
                        }
                    }
                }
                for k in 0..n_factors {
                    ata[k * n_factors + k] += reg;
                }
                let solved = solve_linear(&ata, &atb, n_factors);
                if let Ok(s) = solved {
                    u[user] = s;
                }
            }
        }
        // Update items
        for item in 0..n_items {
            let users: Option<&Vec<(usize, f64)>> = by_item.get(&item);
            if let Some(users) = users {
                let mut ata = vec![0.0; n_factors * n_factors];
                let mut atb = vec![0.0; n_factors];
                for &(user, rating) in users {
                    for k in 0..n_factors {
                        atb[k] += u[user][k] * rating;
                        for l in 0..n_factors {
                            ata[k * n_factors + l] += u[user][k] * u[user][l];
                        }
                    }
                }
                for k in 0..n_factors {
                    ata[k * n_factors + k] += reg;
                }
                let solved = solve_linear(&ata, &atb, n_factors);
                if let Ok(s) = solved {
                    v[item] = s;
                }
            }
        }
    }

    // Compute RMSE
    let sse: f64 = ratings
        .iter()
        .map(|&(user, item, rating)| {
            let pred: f64 = (0..n_factors).map(|k| u[user][k] * v[item][k]).sum();
            (rating - pred).powi(2)
        })
        .sum();
    let rmse = (sse / ratings.len() as f64).sqrt();

    Ok(AlsResult {
        user_factors: u,
        item_factors: v,
        rmse,
    })
}

fn solve_linear(a: &[f64], b: &[f64], n: usize) -> std::result::Result<Vec<f64>, ()> {
    let mut aug = vec![0.0; n * (n + 1)];
    for i in 0..n {
        for j in 0..n {
            aug[i * (n + 1) + j] = a[i * n + j];
        }
        aug[i * (n + 1) + n] = b[i];
    }
    for col in 0..n {
        let pivot = aug[col * (n + 1) + col];
        if pivot.abs() < 1e-12 {
            return Err(());
        }
        for j in 0..=n {
            aug[col * (n + 1) + j] /= pivot;
        }
        for row in 0..n {
            if row == col {
                continue;
            }
            let factor = aug[row * (n + 1) + col];
            for j in 0..=n {
                aug[row * (n + 1) + j] -= factor * aug[col * (n + 1) + j];
            }
        }
    }
    Ok((0..n).map(|i| aug[i * (n + 1) + n]).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apriori() {
        let transactions = vec![
            vec!["A".into(), "B".into(), "C".into()],
            vec!["A".into(), "B".into()],
            vec!["A".into(), "C".into()],
            vec!["B".into(), "C".into()],
            vec!["A".into(), "B".into(), "C".into()],
        ];
        let (frequent, rules) = apriori(&transactions, 0.4, 0.5).unwrap();
        assert!(!frequent.is_empty());
        assert!(!rules.is_empty());
    }

    #[test]
    fn test_als() {
        let ratings = vec![
            (0, 0, 5.0),
            (0, 1, 3.0),
            (1, 0, 4.0),
            (1, 1, 2.0),
            (2, 0, 1.0),
            (2, 2, 5.0),
            (3, 1, 4.0),
            (3, 2, 3.0),
        ];
        let result = als(&ratings, 4, 3, 2, 10, 0.1).unwrap();
        assert_eq!(result.user_factors.len(), 4);
        assert_eq!(result.item_factors.len(), 3);
        assert!(result.rmse.is_finite());
    }
}
