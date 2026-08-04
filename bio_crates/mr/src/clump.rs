//! Greedy LD clumping — pure algorithm, no IO.
//!
//! Mirrors the semantics of PLINK `--clump` (and TwoSampleMR's
//! `clump_data`): select a set of independent index SNPs from a list of
//! candidates by iteratively picking the most significant SNP and removing
//! all SNPs in LD (r² ≥ threshold) within a genomic distance window.
//!
//! This module is **algorithm only** — the r² values are supplied by a
//! caller-provided closure, so the clumping logic can be unit-tested without
//! any genotype data.

// ---------------------------------------------------------------------------
// Input row type
// ---------------------------------------------------------------------------

/// A SNP with its p-value and genomic position, ready for clumping.
///
/// `pval` should be the GWAS p-value of the SNP in the exposure GWAS
/// (lower = more significant = selected as index SNP first).
/// `chr` and `bp` (base-pair position) are used for the distance window.
#[derive(Clone, Debug)]
pub struct ClumpSnp {
    pub rsid: String,
    pub pval: f64,
    pub chr: i64,
    pub bp: i64,
}

// ---------------------------------------------------------------------------
// Greedy clumping
// ---------------------------------------------------------------------------

/// Greedy LD clumping.
///
/// Given a slice of candidate SNPs and a function `r2_fn(i, j)` returning the
/// LD r² between `snps[i]` and `snps[j]`, returns the rsIDs of the independent
/// index SNPs.
///
/// Algorithm (matches PLINK `--clump`):
/// 1. Sort SNPs by p-value ascending (most significant first).
/// 2. Take the top un-removed SNP as an index SNP.
/// 3. Remove all SNPs on the same chromosome, within `kb` kilobases, AND with
///    r² ≥ `r2_thresh` relative to the index SNP.
/// 4. Repeat with the next most-significant remaining SNP.
/// 5. Return the rsIDs of all index SNPs, in the order they were selected.
///
/// SNPs with non-finite p-values are treated as p = 1.0 (least significant).
pub fn greedy_clump<F>(snps: &[ClumpSnp], r2_thresh: f64, kb: i32, r2_fn: F) -> Vec<String>
where
    F: Fn(usize, usize) -> f64,
{
    let n = snps.len();
    if n == 0 {
        return Vec::new();
    }

    let kb_thresh = (kb as i64) * 1000;

    // Sort indices by pval ascending; non-finite pvals sort last.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        let pa = if snps[a].pval.is_finite() {
            snps[a].pval
        } else {
            1.0
        };
        let pb = if snps[b].pval.is_finite() {
            snps[b].pval
        } else {
            1.0
        };
        pa.partial_cmp(&pb).unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut removed = vec![false; n];
    let mut index_snps = Vec::new();

    for &i in &order {
        if removed[i] {
            continue;
        }
        index_snps.push(snps[i].rsid.clone());

        // Prune SNPs in LD with this index SNP.
        for &j in &order {
            if j == i || removed[j] {
                continue;
            }
            // Different chromosomes are never in LD clumping range.
            if snps[j].chr != snps[i].chr {
                continue;
            }
            // Distance window check.
            if (snps[j].bp - snps[i].bp).abs() > kb_thresh {
                continue;
            }
            // r² threshold check.
            if r2_fn(i, j) >= r2_thresh {
                removed[j] = true;
            }
        }
    }

    index_snps
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_snps() -> Vec<ClumpSnp> {
        vec![
            ClumpSnp {
                rsid: "rs1".into(),
                pval: 1e-10,
                chr: 1,
                bp: 1_000_000,
            },
            ClumpSnp {
                rsid: "rs2".into(),
                pval: 1e-9,
                chr: 1,
                bp: 1_000_500,
            },
            ClumpSnp {
                rsid: "rs3".into(),
                pval: 1e-8,
                chr: 1,
                bp: 5_000_000,
            },
            ClumpSnp {
                rsid: "rs4".into(),
                pval: 1e-7,
                chr: 2,
                bp: 1_000_000,
            },
        ]
    }

    #[test]
    fn high_r2_prunes_neighbor() {
        // rs1 is most significant; rs2 is within kb and r²=0.9 → pruned.
        // rs3 is on chr1 but far away → retained. rs4 is on chr2 → retained.
        let snps = make_snps();
        let r2_fn = |i: usize, j: usize| -> f64 {
            // rs1-rs2 have high LD; everything else is independent.
            let pair = (i.min(j), i.max(j));
            match pair {
                (0, 1) => 0.9,
                _ => 0.0,
            }
        };
        let kept = greedy_clump(&snps, 0.001, 5000, r2_fn);
        assert!(kept.contains(&"rs1".to_string()));
        assert!(!kept.contains(&"rs2".to_string()));
        assert!(kept.contains(&"rs3".to_string()));
        assert!(kept.contains(&"rs4".to_string()));
    }

    #[test]
    fn distance_window_prevents_pruning() {
        // rs1 and rs3 are on chr1 but 4 Mb apart. With kb=500 (500 kb window)
        // they are outside the window → both retained even if r² is high.
        let snps = make_snps();
        let r2_fn = |_: usize, _: usize| 0.9;
        let kept = greedy_clump(&snps, 0.001, 500, r2_fn);
        assert!(kept.contains(&"rs1".to_string()));
        // rs2 is 500 bp away → within 500 kb → pruned.
        assert!(!kept.contains(&"rs2".to_string()));
        // rs3 is 4 Mb away → outside 500 kb window → retained.
        assert!(kept.contains(&"rs3".to_string()));
    }

    #[test]
    fn different_chromosomes_never_pruned() {
        // rs1 (chr1) and rs4 (chr2) at same BP with r²=1.0 — different chr
        // means they are never in LD clumping range.
        let snps = make_snps();
        let r2_fn = |_: usize, _: usize| 1.0;
        let kept = greedy_clump(&snps, 0.001, 5000, r2_fn);
        assert!(kept.contains(&"rs1".to_string()));
        assert!(kept.contains(&"rs4".to_string()));
    }

    #[test]
    fn low_r2_threshold_keeps_everything() {
        // r2_fn always returns 0.0 → nothing pruned.
        let snps = make_snps();
        let r2_fn = |_: usize, _: usize| 0.0;
        let kept = greedy_clump(&snps, 0.001, 5000, r2_fn);
        assert_eq!(kept.len(), 4);
    }

    #[test]
    fn order_follows_significance() {
        // The first index SNP should be the most significant one.
        let snps = make_snps();
        let r2_fn = |_: usize, _: usize| 0.0;
        let kept = greedy_clump(&snps, 0.001, 5000, r2_fn);
        assert_eq!(kept[0], "rs1"); // pval = 1e-10 (lowest)
    }

    #[test]
    fn empty_input() {
        let kept = greedy_clump(&[], 0.001, 5000, |_, _| 0.0);
        assert!(kept.is_empty());
    }

    #[test]
    fn non_finite_pval_treated_as_one() {
        let snps = vec![
            ClumpSnp {
                rsid: "rs_nan".into(),
                pval: f64::NAN,
                chr: 1,
                bp: 100,
            },
            ClumpSnp {
                rsid: "rs_ok".into(),
                pval: 0.5,
                chr: 1,
                bp: 200,
            },
        ];
        let kept = greedy_clump(&snps, 0.001, 5000, |_, _| 0.0);
        // rs_ok has finite pval 0.5 < 1.0 (NaN default) → selected first.
        assert_eq!(kept[0], "rs_ok");
    }
}
