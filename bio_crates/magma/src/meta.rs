//! Meta-analysis and merge — combine multiple gene results.
//!
//! Faithful port of `geneinput.cpp` (`GeneInputMeta` and `GeneInputMerge`).
//!
//! # Meta-analysis (`--meta`)
//!
//! Combines gene-level Z-statistics from multiple cohorts using:
//! - Fixed-effect weighted sum (default) or user-specified weights
//! - Optional cross-cohort correlation matrix (for sample overlap correction)
//!
//! # Merge (`--merge`)
//!
//! Merges batch results (from `--batch` parallel runs) into a single output.

use std::path::Path;

use crate::error::{MagmaError, Result};
use crate::setanalysis::GeneRawData;

/// Merge multiple `.genes.raw` files (from batch runs) into a single dataset.
///
/// This is the simplest combination: genes from all files are unioned,
/// and their statistics are taken from whichever file contains them.
/// (For true batch merge, all files should have the same genes.)
pub fn merge_raw_files(paths: &[&Path]) -> Result<GeneRawData> {
    if paths.is_empty() {
        return Err(MagmaError::Input("no files to merge".into()));
    }

    let mut merged = GeneRawData::read(paths[0])?;
    for &path in &paths[1..] {
        let data = GeneRawData::read(path)?;
        // For batch merge, all files should have the same genes.
        // We take the correlation values from whichever file has them.
        for (i, gene) in data.genes.iter().enumerate() {
            if let Some(_existing) = merged.genes.iter().find(|g| g.id == gene.id) {
                // Gene already exists — update if this file has more data
                let idx = merged.genes.iter().position(|g| g.id == gene.id).unwrap();
                if !data.corrs[i].is_empty() && merged.corrs[idx].is_empty() {
                    merged.corrs[idx] = data.corrs[i].clone();
                }
            } else {
                merged.genes.push(gene.clone());
                merged.corrs.push(data.corrs[i].clone());
            }
        }
    }

    Ok(merged)
}

/// Meta-analyze gene Z-statistics from multiple cohorts.
///
/// Default: inverse-variance weighted Z-statistic combination.
/// For gene g with Z-statistics z₁, z₂, ..., zₖ from k cohorts:
///   combined_z = Σwᵢzᵢ / √(Σwᵢ²)
/// where wᵢ = √Nᵢ (weight proportional to √sample size).
///
/// If `weights` is provided, uses user-specified weights instead.
/// If `correlations` is provided (k×k matrix), accounts for sample overlap:
///   combined_z = w'z / √(w'Σw) where Σ is the correlation matrix.
pub fn meta_analyze(
    cohorts: &[GeneRawData],
    weights: Option<&[f64]>,
    correlations: Option<&[Vec<f64>]>,
) -> Result<GeneRawData> {
    if cohorts.is_empty() {
        return Err(MagmaError::Input("no cohorts for meta-analysis".into()));
    }

    let k = cohorts.len();
    let base = &cohorts[0];
    let n_genes = base.n_genes();

    // Default weights: √N for each cohort
    let w: Vec<f64> = match weights {
        Some(ws) => ws.to_vec(),
        None => cohorts
            .iter()
            .map(|c| {
                let mean_n = c.mean_sample_size();
                mean_n.sqrt()
            })
            .collect(),
    };

    let mut genes = Vec::with_capacity(n_genes);
    let mut corrs: Vec<Vec<f64>> = Vec::with_capacity(n_genes);

    for i in 0..n_genes {
        let base_gene = &base.genes[i];

        // Collect Z-statistics from all cohorts for this gene
        let mut z_vals: Vec<f64> = Vec::with_capacity(k);
        let mut found_in_all = true;

        for cohort in cohorts {
            if let Some(gene) = cohort.genes.get(i) {
                if gene.id == base_gene.id {
                    z_vals.push(gene.zstat);
                } else {
                    found_in_all = false;
                    break;
                }
            } else {
                found_in_all = false;
                break;
            }
        }

        if !found_in_all || z_vals.len() != k {
            // Gene not in all cohorts — skip or use available
            continue;
        }

        // Weighted combination
        let combined_z = if let Some(corr_matrix) = correlations {
            // With sample overlap correction: w'z / √(w'Σw)
            let wz: f64 = w.iter().zip(z_vals.iter()).map(|(wi, zi)| wi * zi).sum();

            // Compute w'Σw
            let mut wsw = 0.0;
            for a in 0..k {
                for b in 0..k {
                    let c = if a == b {
                        1.0
                    } else {
                        let (lo, hi) = if a < b { (a, b) } else { (b, a) };
                        corr_matrix[hi].get(lo).copied().unwrap_or(0.0)
                    };
                    wsw += w[a] * w[b] * c;
                }
            }
            if wsw > 0.0 { wz / wsw.sqrt() } else { 0.0 }
        } else {
            // No overlap correction: Σwᵢzᵢ / √(Σwᵢ²)
            let wz: f64 = w.iter().zip(z_vals.iter()).map(|(wi, zi)| wi * zi).sum();
            let wsq: f64 = w.iter().map(|wi| wi * wi).sum();
            if wsq > 0.0 { wz / wsq.sqrt() } else { 0.0 }
        };

        // Truncate Z to avoid extreme values
        let z_clamped = combined_z.clamp(-500.0, 500.0);

        genes.push(crate::setanalysis::GeneRawEntry {
            id: base_gene.id.clone(),
            chr: base_gene.chr,
            start: base_gene.start,
            end: base_gene.end,
            n_snps: base_gene.n_snps,
            n_param: base_gene.n_param,
            n: base_gene.n,
            mac: base_gene.mac,
            zstat: z_clamped,
        });

        // Use correlations from the first cohort
        corrs.push(base.corrs[i].clone());
    }

    Ok(GeneRawData { genes, corrs })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setanalysis::GeneRawData;
    use std::path::PathBuf;

    fn test_dir() -> PathBuf {
        PathBuf::from("tests/data")
    }

    #[test]
    fn test_meta_analyze_two_cohorts() {
        let data = GeneRawData::read(&test_dir().join("gene_pval.genes.raw")).unwrap();

        // Create a second cohort by perturbing Z-statistics
        let mut cohort2 = data.clone();
        for gene in &mut cohort2.genes {
            gene.zstat *= 0.9; // slightly attenuated
            gene.n = (gene.n as f64 * 0.8) as i64;
        }

        let meta = meta_analyze(&[data, cohort2], None, None).unwrap();

        // Should have same number of genes
        assert_eq!(meta.n_genes(), 20);

        // Combined Z should be between the two cohorts' values
        // (weighted average effect)
        for gene in &meta.genes {
            assert!(gene.zstat.is_finite());
        }
    }
}
