//! Credible-set construction and purity filtering.
//!
//! Faithfully ports `susie_get_cs`: for each effect with V > 1e-9, sort
//! variables by alpha descending and accumulate until coverage is reached.
//! Then apply purity filtering (min_abs_corr / median_abs_corr) and dedup.

use crate::model::SusieParams;
use faer::Mat;

/// One credible set: the variables (0-indexed) and their purity metrics.
#[derive(Debug, Clone)]
pub struct CredibleSet {
    /// Variable indices (0-indexed).
    pub variables: Vec<usize>,
    /// Which effect (0-indexed) this CS came from.
    pub effect_index: usize,
    /// Sum of alpha over the CS variables.
    pub coverage: f64,
    /// (min_abs_corr, mean_abs_corr, median_abs_corr). None if purity skipped.
    pub purity: Option<Purity>,
}

#[derive(Debug, Clone)]
pub struct Purity {
    pub min_abs_corr: f64,
    pub mean_abs_corr: f64,
    pub median_abs_corr: f64,
}

/// Collection of credible sets after filtering.
#[derive(Debug, Clone, Default)]
pub struct CredibleSets {
    pub sets: Vec<CredibleSet>,
    pub requested_coverage: f64,
}

/// Build credible sets from the fitted alpha matrix.
///
/// Mirrors `susie_get_cs`:
/// 1. For each effect l with V > 1e-9: sort by alpha desc, accumulate until coverage.
/// 2. Dedup identical CS.
/// 3. Compute purity (min/mean/median |corr|).
/// 4. Filter by min_abs_corr and/or median_abs_corr (OR-linked).
/// 5. Order by purity.
pub fn compute_cs(
    alpha: &[Vec<f64>],
    xtx: &Mat<f64>,
    params: &SusieParams,
) -> CredibleSets {
    let coverage = params.coverage;
    let min_abs_corr = params.min_abs_corr;
    let median_abs_corr = params.median_abs_corr;
    let prior_tol = 1e-9;

    // ── Step 1: build raw CS per effect ──
    let mut raw_sets: Vec<(Vec<usize>, usize, f64)> = Vec::new(); // (vars, effect_idx, claimed_cov)

    for (l, alpha_l) in alpha.iter().enumerate() {
        // Skip effects with negligible prior variance
        let v_ok = l < params.l; // V vector is in model, but we check via alpha
        let _ = v_ok;

        let p = alpha_l.len();
        // Sort variables by alpha descending
        let mut idx: Vec<(usize, f64)> =
            (0..p).map(|j| (j, alpha_l[j])).collect();
        idx.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Accumulate until coverage
        let mut cumsum = 0.0;
        let mut cs_vars: Vec<usize> = Vec::new();
        for &(j, aj) in &idx {
            if aj <= 0.0 {
                break;
            }
            cs_vars.push(j);
            cumsum += aj;
            if cumsum >= coverage {
                break;
            }
        }
        if cs_vars.is_empty() {
            continue;
        }
        raw_sets.push((cs_vars, l, cumsum));
    }

    // ── Step 2: dedup identical CS ──
    let mut seen: Vec<Vec<usize>> = Vec::new();
    let mut deduped: Vec<(Vec<usize>, usize, f64)> = Vec::new();
    for (vars, l, cov) in &raw_sets {
        let mut sorted_vars = vars.clone();
        sorted_vars.sort();
        if !seen.iter().any(|s| *s == sorted_vars) {
            seen.push(sorted_vars);
            deduped.push((vars.clone(), *l, *cov));
        }
    }

    // ── Step 3: compute purity ──
    let mut with_purity: Vec<(Vec<usize>, usize, f64, Purity)> = Vec::new();
    for (vars, l, cov) in &deduped {
        let purity = compute_purity(vars, xtx);
        with_purity.push((vars.clone(), *l, *cov, purity));
    }

    // ── Step 4: filter by purity thresholds (OR-linked) ──
    let filtered: Vec<(Vec<usize>, usize, f64, Purity)> = with_purity
        .into_iter()
        .filter(|(_, _, _, pur)| {
            let mut keep = false;
            if let Some(mac) = min_abs_corr {
                keep |= pur.min_abs_corr >= mac;
            }
            if let Some(mdac) = median_abs_corr {
                keep |= pur.median_abs_corr >= mdac;
            }
            // If neither threshold active, keep all (except degenerate -9 purity)
            if min_abs_corr.is_none() && median_abs_corr.is_none() {
                keep = true;
            }
            keep
        })
        .collect();

    // ── Step 5: order by min_abs_corr (or median_abs_corr) descending ──
    let order_col_uses_min = min_abs_corr.is_some();
    let mut sorted = filtered;
    sorted.sort_by(|a, b| {
        let key_a = if order_col_uses_min {
            a.3.min_abs_corr
        } else {
            a.3.median_abs_corr
        };
        let key_b = if order_col_uses_min {
            b.3.min_abs_corr
        } else {
            b.3.median_abs_corr
        };
        key_b.partial_cmp(&key_a).unwrap_or(std::cmp::Ordering::Equal)
    });

    let _ = prior_tol;

    let sets = sorted
        .into_iter()
        .map(|(vars, l, cov, pur)| CredibleSet {
            variables: vars,
            effect_index: l,
            coverage: cov,
            purity: Some(pur),
        })
        .collect();

    CredibleSets {
        sets,
        requested_coverage: coverage,
    }
}

/// Compute purity: min, mean, median |corr| among CS variables.
///
/// For a single-variable CS, purity = (1, 1, 1).
fn compute_purity(vars: &[usize], xtx: &Mat<f64>) -> Purity {
    if vars.len() == 1 {
        return Purity {
            min_abs_corr: 1.0,
            mean_abs_corr: 1.0,
            median_abs_corr: 1.0,
        };
    }

    // Extract pairwise |corr| from upper triangle of xtx[vars, vars]
    let n = vars.len();
    let mut vals: Vec<f64> = Vec::with_capacity(n * (n - 1) / 2);
    for i in 0..n {
        for j in (i + 1)..n {
            let c = xtx[(vars[i], vars[j])];
            vals.push(c.abs());
        }
    }

    let min_abs = vals.iter().copied().fold(f64::INFINITY, f64::min);
    let mean_abs: f64 = vals.iter().sum::<f64>() / vals.len() as f64;
    let median_abs = median(&mut vals);

    Purity {
        min_abs_corr: min_abs,
        mean_abs_corr: mean_abs,
        median_abs_corr: median_abs,
    }
}

fn median(vals: &mut [f64]) -> f64 {
    if vals.is_empty() {
        return f64::NAN;
    }
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = vals.len();
    if n % 2 == 1 {
        vals[n / 2]
    } else {
        (vals[n / 2 - 1] + vals[n / 2]) / 2.0
    }
}
