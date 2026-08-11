//! Summary statistic merging — port of `R/sumstats.R`.
//!
//! Merge multiple munged GWAS summary statistics for multivariate GWAS.

use crate::error::Result;

/// Configuration for `sumstats`.
#[derive(Clone, Debug)]
pub struct SumstatsConfig {
    pub trait_names: Vec<String>,
    pub n: Vec<Option<f64>>,
    pub info_filter: f64,
    pub maf_filter: f64,
    pub keep_indel: bool,
    pub ambig: bool,
}

impl Default for SumstatsConfig {
    fn default() -> Self {
        Self {
            trait_names: Vec::new(),
            n: Vec::new(),
            info_filter: 0.6,
            maf_filter: 0.01,
            keep_indel: false,
            ambig: false,
        }
    }
}

/// Merged summary statistics ready for GWAS.
#[derive(Clone, Debug)]
pub struct MergedSumstats {
    pub snp: Vec<String>,
    pub chr: Vec<i32>,
    pub bp: Vec<i64>,
    pub maf: Vec<f64>,
    pub a1: Vec<String>,
    pub a2: Vec<String>,
    /// beta columns: beta.trait1, beta.trait2, ...
    pub betas: Vec<Vec<f64>>,
    /// SE columns: se.trait1, se.trait2, ...
    pub ses: Vec<Vec<f64>>,
    pub n_traits: usize,
}

impl MergedSumstats {
    /// Number of SNPs.
    pub fn n_snps(&self) -> usize {
        self.snp.len()
    }

    /// Get beta column for trait i.
    pub fn beta_col(&self, trait_idx: usize) -> &[f64] {
        &self.betas[trait_idx]
    }

    /// Get SE column for trait i.
    pub fn se_col(&self, trait_idx: usize) -> &[f64] {
        &self.ses[trait_idx]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merged_sumstats_construction() {
        let ms = MergedSumstats {
            snp: vec!["rs1".into(), "rs2".into()],
            chr: vec![1, 1],
            bp: vec![100, 200],
            maf: vec![0.3, 0.4],
            a1: vec!["A".into(), "C".into()],
            a2: vec!["G".into(), "T".into()],
            betas: vec![vec![0.1, 0.2], vec![0.3, 0.4]],
            ses: vec![vec![0.01, 0.02], vec![0.03, 0.04]],
            n_traits: 2,
        };
        assert_eq!(ms.n_snps(), 2);
        assert_eq!(ms.beta_col(0), &[0.1, 0.2]);
    }
}
