//! Multiple-testing correction — direct port of `stats::p.adjust`.
//!
//! Seven methods, matching R's `p.adjust.methods`:
//!
//! | Method        | Controls | Notes                                            |
//! |---------------|----------|--------------------------------------------------|
//! | `Bonferroni`  | FWER     | `n·p`, capped at 1.                              |
//! | `Holm`        | FWER     | Step-down Bonferroni.                            |
//! | `Hochberg`    | FWER     | Step-up; valid under independence.               |
//! | `Hommel`      | FWER     | Set-based step-down (Hommel, 1988).              |
//! | `Bh` (= `Fdr`)| FDR      | Benjamini–Hochberg step-up.                       |
//! | `By`          | FDR      | Benjamini–Yekutieli; valid under dependence.      |
//! | `None`        | —        | Identity.                                         |
//!
//! Line-by-line port of the R 4.x algorithm; cross-validated against
//! `stats::p.adjust` in `tests/xval_padjust.rs`.

use serde::{Deserialize, Serialize};
use schemars::JsonSchema;

use crate::{HypoError, Result};

/// Method selector mirroring `p.adjust.methods`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum AdjustMethod {
    Bonferroni,
    Holm,
    Hochberg,
    Hommel,
    /// Benjamini–Hochberg (also known as `fdr`).
    Bh,
    /// Benjamini–Yekutieli.
    By,
    None,
}

impl AdjustMethod {
    /// Parse from R's string (case-insensitive; `"fdr"` is `Bh`).
    pub fn parse(s: &str) -> Result<Self> {
        let lower = s.to_ascii_lowercase();
        match lower.as_str() {
            "bonferroni" | "bonf" => Ok(Self::Bonferroni),
            "holm" => Ok(Self::Holm),
            "hochberg" => Ok(Self::Hochberg),
            "hommel" => Ok(Self::Hommel),
            "bh" | "fdr" => Ok(Self::Bh),
            "by" => Ok(Self::By),
            "none" => Ok(Self::None),
            other => Err(HypoError::InvalidInput(format!(
                "unknown p.adjust method '{other}'"
            ))),
        }
    }
}

/// Index permutation that sorts `v` ascending (stable on ties).
fn argsort_ascending(v: &[f64]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..v.len()).collect();
    idx.sort_by(|&a, &b| {
        v[a].partial_cmp(&v[b]).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(&b))
    });
    idx
}

/// Index permutation that sorts `v` descending (stable on ties).
fn argsort_descending(v: &[f64]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..v.len()).collect();
    idx.sort_by(|&a, &b| {
        v[b].partial_cmp(&v[a]).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(&b))
    });
    idx
}

/// Core port of `stats::p.adjust`. `n_total` overrides the family size
/// (R's `n` argument); when `None`, defaults to `p.len()`.
pub fn p_adjust_raw(p: &[f64], method: AdjustMethod, n_total: Option<usize>) -> Result<Vec<f64>> {
    let n = n_total.unwrap_or(p.len());
    if matches!(method, AdjustMethod::None) {
        return Ok(p.to_vec());
    }
    if p.is_empty() {
        return Ok(Vec::new());
    }
    if n <= 1 {
        return Ok(p.to_vec());
    }
    let n_f = n as f64;

    let result = match method {
        AdjustMethod::Bonferroni => p.iter().map(|&pi| (pi * n_f).min(1.0)).collect(),
        AdjustMethod::Holm => holm(p, n_f),
        AdjustMethod::Hochberg => hochberg(p, n_f),
        AdjustMethod::Hommel => hommel(p, n_f),
        AdjustMethod::Bh => bh_by(p, n_f, 1.0),
        AdjustMethod::By => bh_by(p, n_f, harmonic(n_f)),
        AdjustMethod::None => unreachable!(),
    };
    Ok(result)
}

/// Holm step-down: order ascending; rank-i weight = `n − i + 1`; enforce
/// monotonicity via cummax; unpermute.
fn holm(p: &[f64], n: f64) -> Vec<f64> {
    let o = argsort_ascending(p);
    let nn = p.len();
    let mut sorted_adj = vec![0.0_f64; nn];
    let mut cummax = 0.0_f64;
    for (rank, &idx) in o.iter().enumerate() {
        let factor = n - rank as f64; // n − i + 1, i = rank+1
        let val = (p[idx] * factor).min(1.0);
        if val > cummax {
            cummax = val;
        }
        sorted_adj[rank] = cummax;
    }
    let mut result = vec![0.0_f64; nn];
    for (rank, &idx) in o.iter().enumerate() {
        result[idx] = sorted_adj[rank];
    }
    result
}

/// Hochberg step-up: order descending; rank-i weight = `i` (the (n−i+1) of
/// R, where i descends from n to 1); cummin enforces monotonicity; unpermute.
fn hochberg(p: &[f64], n: f64) -> Vec<f64> {
    let _ = n;
    let nn = p.len();
    let o = argsort_descending(p);
    let mut sorted_adj = vec![0.0_f64; nn];
    let mut cummin = f64::INFINITY;
    for (rank, &idx) in o.iter().enumerate() {
        let factor = (rank + 1) as f64; // 1, 2, ..., n for largest→smallest
        let val = p[idx] * factor;
        if val < cummin {
            cummin = val;
        }
        sorted_adj[rank] = cummin.min(1.0);
    }
    let mut result = vec![0.0_f64; nn];
    for (rank, &idx) in o.iter().enumerate() {
        result[idx] = sorted_adj[rank];
    }
    result
}

/// BH / BY step-up: order descending; rank-i weight = `n / (n − rank)`; scale
/// by `q` (1 for BH, harmonic number H_n for BY); cummin; unpermute.
fn bh_by(p: &[f64], n: f64, q: f64) -> Vec<f64> {
    let nn = p.len();
    let o = argsort_descending(p);
    let mut sorted_adj = vec![0.0_f64; nn];
    let mut cummin = f64::INFINITY;
    for (rank, &idx) in o.iter().enumerate() {
        // For rank=0 (largest p), i = n in R → n/i = 1. For rank=k, i = n-k → n/(n-k).
        let i_r = n - rank as f64; // R's i in descending order n, n-1, ..., 1
        let weight = if i_r > 0.0 { n / i_r } else { f64::INFINITY };
        let val = p[idx] * weight * q;
        if val < cummin {
            cummin = val;
        }
        sorted_adj[rank] = cummin;
    }
    let mut result = vec![0.0_f64; nn];
    for (rank, &idx) in o.iter().enumerate() {
        result[idx] = sorted_adj[rank].min(1.0);
    }
    result
}

/// N-th harmonic number `H_n = Σ_{i=1}^n 1/i`.
fn harmonic(n: f64) -> f64 {
    let n_int = n.round() as i64;
    if n_int <= 0 {
        return 0.0;
    }
    (1..=n_int).map(|i| 1.0 / i as f64).sum()
}

/// Hommel (1988) — exact port of R's `p.adjust(method = "hommel")`.
///
/// For `n == 2`, R falls back to Hochberg. For `n > 2`, the algorithm sorts
/// p ascending, initialises `q = pa = min(n·p_(i)/i)` for all i, then for
/// each `j` from `n−1` down to `2` refines `q` and accumulates `pa` via
/// element-wise max. The final result is `pmax(pa, p_sorted)`, unpermuted.
fn hommel(p: &[f64], n: f64) -> Vec<f64> {
    let nn = p.len();
    if nn <= 1 {
        return p.to_vec();
    }
    // R special-cases n == 2 hommel → hochberg.
    if nn == 2 && (n as usize) == 2 {
        return hochberg(p, n);
    }
    let o = argsort_ascending(p);
    let p_sorted: Vec<f64> = o.iter().map(|&idx| p[idx]).collect();
    let n_f = n; // family size (R's n argument)

    // Initialise: q = pa = min(n * p_sorted[i] / i) broadcast over all positions.
    let init = (0..nn)
        .map(|rank| n_f * p_sorted[rank] / (rank + 1) as f64)
        .fold(f64::INFINITY, f64::min);
    let mut q = vec![init; nn];
    let mut pa = vec![init; nn];

    // j from n-1 down to 2.
    for j in (2..=(nn - 1)).rev() {
        let n_ij = nn - j + 1; // |ij| = n - j + 1

        // q1 = min(j * p_sorted[i2] / (2:j))
        // i2 (0-indexed) = {n-j+1, ..., n-1}, length j-1.
        // divisors = {2, 3, ..., j}.
        let q1 = (0..(j - 1))
            .map(|k| j as f64 * p_sorted[n_ij + k] / (2 + k) as f64)
            .fold(f64::INFINITY, f64::min);

        // q[ij] = pmin(j * p_sorted[ij], q1)  (ij = 0..n_ij-1, 0-indexed)
        for k in 0..n_ij {
            q[k] = (j as f64 * p_sorted[k]).min(q1);
        }
        // q[i2] = q[n-j+1] (1-indexed) = q[n_ij-1] (last of the ij block, 0-indexed).
        let q_bridge = q[n_ij - 1];
        for k in n_ij..nn {
            q[k] = q_bridge;
        }
        // pa = pmax(pa, q)
        for k in 0..nn {
            pa[k] = pa[k].max(q[k]);
        }
    }

    // Final: pmax(pa, p_sorted), then unpermute.
    let mut result = vec![0.0_f64; nn];
    for (rank, &idx) in o.iter().enumerate() {
        result[idx] = pa[rank].max(p_sorted[rank]);
    }
    result
}

/// Adjust a family of [`crate::HypothesisTest`]s in place: each one's
/// `p_value` becomes the adjusted value, the original p is preserved in
/// `extras.original_pval`, and adjustment metadata is recorded. Mirrors
/// `hypothesize::adjust_pval` operating on a list of tests.
pub fn adjust_pvals(
    tests: &mut [&mut crate::HypothesisTest],
    method: AdjustMethod,
    n_total: Option<usize>,
) -> Result<()> {
    let pvals: Vec<f64> = tests.iter().map(|t| t.p_value).collect();
    let adjusted = p_adjust_raw(&pvals, method, n_total)?;
    let n_eff = n_total.unwrap_or(pvals.len()) as u64;
    for (t, adj) in tests.iter_mut().zip(adjusted) {
        t.extras
            .insert(crate::KEY_ORIGINAL_PVAL.into(), serde_json::json!(t.p_value));
        t.extras.insert(
            "adjustment_method".into(),
            serde_json::json!(format!("{method:?}").to_lowercase()),
        );
        t.extras
            .insert("n_tests".into(), serde_json::json!(n_eff));
        t.p_value = adj;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: &[f64], b: &[f64], tol: f64) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < tol)
    }

    #[test]
    fn none_is_identity() {
        let p = vec![0.01, 0.02, 0.5];
        assert_eq!(p_adjust_raw(&p, AdjustMethod::None, None).unwrap(), p);
    }

    #[test]
    fn bonferroni_basic() {
        let p = vec![0.01, 0.02, 0.5, 0.6];
        let adj = p_adjust_raw(&p, AdjustMethod::Bonferroni, None).unwrap();
        assert!(close(&adj, &[0.04, 0.08, 1.0, 1.0], 1e-12));
    }

    #[test]
    fn bonferroni_with_n_override() {
        let p = vec![0.01, 0.02];
        let adj = p_adjust_raw(&p, AdjustMethod::Bonferroni, Some(10)).unwrap();
        assert!(close(&adj, &[0.1, 0.2], 1e-12));
    }

    #[test]
    fn holm_basic() {
        // p.adjust(c(0.01, 0.02, 0.5, 0.6), "holm") = c(0.04, 0.06, 1.0, 1.0)
        let p = vec![0.01_f64, 0.02, 0.5, 0.6];
        let adj = p_adjust_raw(&p, AdjustMethod::Holm, None).unwrap();
        assert!(close(&adj, &[0.04, 0.06, 1.0, 1.0], 1e-12));
    }

    #[test]
    fn holm_unsorted_input() {
        let p = vec![0.5_f64, 0.01, 0.6, 0.02];
        let adj = p_adjust_raw(&p, AdjustMethod::Holm, None).unwrap();
        assert!(close(&adj, &[1.0, 0.04, 1.0, 0.06], 1e-12));
    }

    #[test]
    fn hochberg_basic() {
        // p.adjust(c(0.01, 0.02, 0.5, 0.6), "hochberg") = c(0.04, 0.06, 0.6, 0.6)
        let p = vec![0.01_f64, 0.02, 0.5, 0.6];
        let adj = p_adjust_raw(&p, AdjustMethod::Hochberg, None).unwrap();
        assert!(close(&adj, &[0.04, 0.06, 0.6, 0.6], 1e-12));
    }

    #[test]
    fn bh_basic() {
        // p.adjust(c(0.01, 0.02, 0.5, 0.6), "BH") = c(0.04, 0.04, 0.6, 0.6)
        let p = vec![0.01_f64, 0.02, 0.5, 0.6];
        let adj = p_adjust_raw(&p, AdjustMethod::Bh, None).unwrap();
        assert!(close(&adj, &[0.04, 0.04, 0.6, 0.6], 1e-12));
    }

    #[test]
    fn bh_larger_set() {
        // p.adjust(c(0.005, 0.01, 0.02, 0.04, 0.5), "BH")
        //   = c(0.025, 0.025, 0.0333, 0.05, 0.5)
        let p = vec![0.005_f64, 0.01, 0.02, 0.04, 0.5];
        let adj = p_adjust_raw(&p, AdjustMethod::Bh, None).unwrap();
        assert!(close(&adj, &[0.025, 0.025, 0.033_333, 0.05, 0.5], 1e-6));
    }

    #[test]
    fn by_at_least_as_conservative_as_bh() {
        let p = vec![0.005_f64, 0.01, 0.02, 0.04, 0.5];
        let bh = p_adjust_raw(&p, AdjustMethod::Bh, None).unwrap();
        let by = p_adjust_raw(&p, AdjustMethod::By, None).unwrap();
        for (b, y) in bh.iter().zip(by.iter()) {
            assert!(*y >= b - 1e-12, "BY must be ≥ BH: {y} < {b}");
        }
    }

    #[test]
    fn hommel_matches_r() {
        // R: p.adjust(c(0.01, 0.02, 0.5, 0.6), "hommel") = c(0.04, 0.06, 0.6, 0.6)
        let p = vec![0.01_f64, 0.02, 0.5, 0.6];
        let adj = p_adjust_raw(&p, AdjustMethod::Hommel, None).unwrap();
        assert!(close(&adj, &[0.04, 0.06, 0.6, 0.6], 1e-9));
    }

    #[test]
    fn hommel_unsorted_input() {
        let p = vec![0.6_f64, 0.01, 0.5, 0.02];
        let adj = p_adjust_raw(&p, AdjustMethod::Hommel, None).unwrap();
        assert!(close(&adj, &[0.6, 0.04, 0.6, 0.06], 1e-9));
    }

    #[test]
    fn hommel_n2_falls_back_to_hochberg() {
        let p = vec![0.01_f64, 0.02];
        let adj = p_adjust_raw(&p, AdjustMethod::Hommel, None).unwrap();
        let hoc = p_adjust_raw(&p, AdjustMethod::Hochberg, None).unwrap();
        assert!(close(&adj, &hoc, 1e-12));
    }

    #[test]
    fn holm_dominates_bonferroni() {
        let p = vec![0.001_f64, 0.01, 0.02, 0.04, 0.5, 0.9];
        let bonf = p_adjust_raw(&p, AdjustMethod::Bonferroni, None).unwrap();
        let holm = p_adjust_raw(&p, AdjustMethod::Holm, None).unwrap();
        for (h, b) in holm.iter().zip(bonf.iter()) {
            assert!(*h <= *b + 1e-9);
        }
    }

    #[test]
    fn parse_methods() {
        assert_eq!(AdjustMethod::parse("BH").unwrap(), AdjustMethod::Bh);
        assert_eq!(AdjustMethod::parse("fdr").unwrap(), AdjustMethod::Bh);
        assert_eq!(AdjustMethod::parse("BY").unwrap(), AdjustMethod::By);
        assert!(AdjustMethod::parse("unknown").is_err());
    }
}
