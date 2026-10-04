//! Instrument pruning — Rust port of `MRlap/R/run_MR.R` (lines 57-158).
//!
//! Two modes, selected by `pruning_ld`:
//!
//! - **Distance pruning** (`pruning_ld == 0`, the default): greedily keeps the
//!   most-significant SNP and removes all others within `pruning_dist` kb on the
//!   same chromosome. Faithful port of the `prune_byDistance` closure (lines
//!   123-147), including its in-place delete-while-iterating behaviour.
//! - **LD pruning** (`pruning_ld > 0`): in the R reference this shells out to
//!   `ieugwasr::ld_clump` (remote API or local PLINK). That path is I/O and is
//!   not a numerical algorithm; the node layer performs clumping externally and
//!   feeds the surviving rsids via [`PruneMode::UserProvided`].
//!
//! Reverse-direction filtering (`MR_reverse`, lines 69-77) excludes instruments
//! whose outcome association dominates the exposure association.

use crate::error::{MrlapError, Result};
use crate::harmonise::HarmonisedRow;

/// How instruments are selected.
#[derive(Clone, Debug)]
pub enum PruneMode {
    /// Distance-based pruning (default). Fields: p-value threshold for
    /// selecting instruments, reverse-direction p threshold (R's `MR_reverse`),
    /// pruning distance in kb.
    Distance {
        mr_threshold: f64,
        mr_reverse: Option<f64>,
        pruning_dist_kb: f64,
    },
    /// LD-based pruning was done externally (PLINK / API); the caller passes the
    /// surviving rsids. Still applies thresholding + reverse filtering first.
    LdProvided {
        mr_threshold: f64,
        mr_reverse: Option<f64>,
        kept_rsids: Vec<String>,
    },
    /// Skip pruning entirely — use the user-provided instrument list as-is
    /// (R's `do_pruning = FALSE`, `user_SNPsToKeep`).
    UserProvided(Vec<String>),
}

/// A candidate instrument with the fields the MR / correction stages consume.
#[derive(Clone, Debug)]
pub struct Instrument {
    pub rsid: String,
    pub std_beta_exp: f64,
    pub std_se_exp: f64,
    pub std_beta_out: f64,
    pub std_se_out: f64,
    pub n_exp: f64,
    pub n_out: f64,
}

/// Validate a `MR_threshold` argument (R's bounds checks, lines 132-133).
pub fn validate_threshold(t: f64) -> Result<()> {
    if t > 1e-5 {
        return Err(MrlapError::invalid_arg(
            "MR_threshold: superior to the threshold limit (1e-5)",
        ));
    }
    Ok(())
}

/// Validate `MR_pruning_dist` (R's bounds checks, lines 138-140).
pub fn validate_pruning_dist(d: f64) -> Result<()> {
    if d < 10.0 {
        return Err(MrlapError::invalid_arg(
            "MR_pruning_dist: should be higher than 10Kb",
        ));
    }
    if d > 50000.0 {
        return Err(MrlapError::invalid_arg(
            "MR_pruning_dist: should be lower than 50Mb",
        ));
    }
    Ok(())
}

/// Reverse-direction filter (R `run_MR.R` lines 69-77). Excludes instruments
/// more strongly associated with the outcome than the exposure:
/// `reverse_t_threshold = stats::qnorm(MR_reverse)` — a one-sided lower-tail
/// quantile (e.g. −3.09 at 1e-3), so only IVs whose exposure association does
/// NOT dominate (stat ≤ qnorm) are dropped.
fn apply_reverse(data: &mut Vec<HarmonisedRow>, mr_reverse: Option<f64>) {
    if let Some(p) = mr_reverse {
        let t = crate::input::qnorm(p); // R: stats::qnorm(MR_reverse)
        data.retain(|r| {
            let stat = (r.std_beta_exp.abs() - r.std_beta_out.abs())
                / (r.std_se_exp.powi(2) + r.std_se_out.powi(2)).sqrt();
            stat > t
        });
    }
}

/// Greedy distance pruning — direct port of `prune_byDistance` (lines 123-147).
///
/// Input slices are `(rsid, chr, pos, p)`. Returns the kept rsids in the order
/// they survive (most significant first within each region).
fn prune_by_distance(mut rows: Vec<(String, i32, i64, f64)>, prune_dist_kb: f64) -> Vec<String> {
    // order by p ascending (most significant first) — `byP = T`
    rows.sort_by(|a, b| a.3.partial_cmp(&b.3).unwrap_or(std::cmp::Ordering::Equal));
    let prune_dist_bp = (prune_dist_kb * 1000.0) as i64;
    let mut i = 0;
    while i < rows.len() {
        // remove all later rows on the same chr within prune_dist_bp
        let mut j = i + 1;
        while j < rows.len() {
            if rows[j].0 == rows[i].0 {
                // safety: same rsid — keep moving (shouldn't happen post-join)
                j += 1;
                continue;
            }
            let same_chr = rows[j].1 == rows[i].1;
            let close = (rows[j].2 - rows[i].2).abs() < prune_dist_bp;
            if same_chr && close {
                rows.remove(j);
            } else {
                j += 1;
            }
        }
        i += 1;
    }
    rows.into_iter().map(|(s, _, _, _)| s).collect()
}

/// Select instruments from harmonised data according to `mode`.
///
/// Mirrors the full `run_MR.R` instrument-selection block (lines 44-158),
/// returning the surviving [`Instrument`]s plus the pruned harmonised rows.
pub fn select_instruments(
    data: &[HarmonisedRow],
    mode: &PruneMode,
) -> Result<(Vec<Instrument>, Vec<HarmonisedRow>)> {
    match mode {
        PruneMode::UserProvided(rsids) => {
            let pruned: Vec<HarmonisedRow> = data
                .iter()
                .filter(|d| rsids.iter().any(|s| s == &d.rsid))
                .cloned()
                .collect();
            if pruned.len() < 3 {
                return Err(MrlapError::invalid_arg(
                    "If `do_pruning` is FALSE then need at least 3 IVs",
                ));
            }
            Ok((to_instruments(&pruned), pruned))
        }
        PruneMode::Distance {
            mr_threshold,
            mr_reverse,
            pruning_dist_kb,
        } => {
            validate_threshold(*mr_threshold)?;
            validate_pruning_dist(*pruning_dist_kb)?;
            let mut thresh: Vec<HarmonisedRow> = data
                .iter()
                .filter(|d| d.p_exp < *mr_threshold)
                .cloned()
                .collect();
            if thresh.is_empty() {
                return Err(MrlapError::numerical("no IV left after thresholding"));
            }
            apply_reverse(&mut thresh, *mr_reverse);
            if thresh.is_empty() {
                return Err(MrlapError::numerical(
                    "no IV left after excluding IVs more strongly associated with the outcome",
                ));
            }
            let rows: Vec<(String, i32, i64, f64)> = thresh
                .iter()
                .map(|d| {
                    (
                        d.rsid.clone(),
                        d.chr_exp.unwrap_or(0),
                        d.pos_exp.unwrap_or(0),
                        d.p_exp,
                    )
                })
                .collect();
            let kept = prune_by_distance(rows, *pruning_dist_kb);
            let kept_set: std::collections::HashSet<&str> =
                kept.iter().map(|s| s.as_str()).collect();
            let pruned: Vec<HarmonisedRow> = thresh
                .iter()
                .filter(|d| kept_set.contains(d.rsid.as_str()))
                .cloned()
                .collect();
            Ok((to_instruments(&pruned), pruned))
        }
        PruneMode::LdProvided {
            mr_threshold,
            mr_reverse,
            kept_rsids,
        } => {
            validate_threshold(*mr_threshold)?;
            let mut thresh: Vec<HarmonisedRow> = data
                .iter()
                .filter(|d| d.p_exp < *mr_threshold)
                .cloned()
                .collect();
            if thresh.is_empty() {
                return Err(MrlapError::numerical("no IV left after thresholding"));
            }
            apply_reverse(&mut thresh, *mr_reverse);
            if thresh.is_empty() {
                return Err(MrlapError::numerical(
                    "no IV left after excluding IVs more strongly associated with the outcome",
                ));
            }
            let set: std::collections::HashSet<&str> =
                kept_rsids.iter().map(|s| s.as_str()).collect();
            let pruned: Vec<HarmonisedRow> = thresh
                .iter()
                .filter(|d| set.contains(d.rsid.as_str()))
                .cloned()
                .collect();
            Ok((to_instruments(&pruned), pruned))
        }
    }
}

fn to_instruments(pruned: &[HarmonisedRow]) -> Vec<Instrument> {
    pruned
        .iter()
        .map(|d| Instrument {
            rsid: d.rsid.clone(),
            std_beta_exp: d.std_beta_exp,
            std_se_exp: d.std_se_exp,
            std_beta_out: d.std_beta_out,
            std_se_out: d.std_se_out,
            n_exp: d.n_exp,
            n_out: d.n_out,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        rsid: &str,
        std_beta_exp: f64,
        std_se_exp: f64,
        std_beta_out: f64,
        std_se_out: f64,
    ) -> HarmonisedRow {
        HarmonisedRow {
            rsid: rsid.to_string(),
            chr_exp: Some(1),
            pos_exp: Some(1000),
            alt_exp: "A".into(),
            ref_exp: "G".into(),
            n_exp: 100_000.0,
            std_beta_exp,
            std_se_exp,
            p_exp: 1e-6,
            std_beta_out,
            std_se_out,
            p_out: 1e-6,
            n_out: 100_000.0,
        }
    }

    #[test]
    fn reverse_filter_is_one_sided_at_qnorm() {
        // R: reverse_t_threshold = stats::qnorm(1e-3) = -3.0902 (one-sided
        // lower tail). Regression: the old code used the two-sided
        // -qnorm(p/2) = +3.29, inverting the filter's direction and making it
        // drop exposure-dominating IVs instead of reverse-direction ones.
        // With equal SEs of 0.05 the denominator is 0.05·√2 ≈ 0.07071.
        let mut data = vec![
            row("strong", 0.30, 0.05, 0.02, 0.05), // stat ≈ +3.96 → keep
            row("mild", 0.00, 0.05, 0.2121, 0.05), // stat ≈ -3.00 > qnorm → keep
            row("rev", 0.00, 0.05, 0.2475, 0.05),  // stat ≈ -3.50 < qnorm → drop
        ];
        apply_reverse(&mut data, Some(1e-3));
        let ids: Vec<&str> = data.iter().map(|d| d.rsid.as_str()).collect();
        assert_eq!(ids, vec!["strong", "mild"]);

        // None disables the filter entirely.
        let mut all = vec![row("rev", 0.0, 0.05, 0.9, 0.05)];
        apply_reverse(&mut all, None);
        assert_eq!(all.len(), 1);
    }
}
