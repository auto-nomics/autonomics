//! Harmonisation — Rust port of the allele-alignment block in
//! `MRlap/R/run_MR.R` (lines 32-40).
//!
//! Inner-joins exposure and outcome tidied rows on `rsid`, then aligns the
//! outcome's standardised effect to the exposure's effect allele:
//!
//! - same alleles  → keep sign
//! - swapped alleles → flip sign
//! - allele mismatch → drop the SNP
//!
//! `std_beta.exp` / `std_SE.exp` are kept as-is (exposure defines the effect
//! allele direction).

use crate::input::TidyRow;

/// A harmonised exposure–outcome pair ready for MR / pruning.
#[derive(Clone, Debug)]
pub struct HarmonisedRow {
    pub rsid: String,
    pub chr_exp: Option<i32>,
    pub pos_exp: Option<i64>,
    pub alt_exp: String,
    pub ref_exp: String,
    pub n_exp: f64,
    // Exposure standardised effects (effect allele = alt_exp).
    pub std_beta_exp: f64,
    pub std_se_exp: f64,
    pub p_exp: f64,
    // Outcome standardised effects, aligned to alt_exp.
    pub std_beta_out: f64,
    pub std_se_out: f64,
    pub p_out: f64,
    pub n_out: f64,
}

/// Inner-join two tidied GWAS on rsid and align outcome alleles to the exposure.
///
/// When the same rsid appears multiple times within one trait (duplicates), the
/// R `dplyr::inner_join` produces a Cartesian product per key; we replicate that
/// by emitting every matching pair. In practice GWAS sumstats are unique by rsid.
pub fn harmonise(exposure: &[TidyRow], outcome: &[TidyRow]) -> Vec<HarmonisedRow> {
    // Index outcome by rsid for the join (preserving multiplicity).
    let mut out: Vec<HarmonisedRow> = Vec::new();
    for e in exposure {
        for o in outcome {
            if e.rsid != o.rsid {
                continue;
            }
            // Allele alignment — `run_MR.R` lines 35-39.
            let aligned = if e.alt == o.alt && e.ref_allele == o.ref_allele {
                Some(o.std_beta)
            } else if e.ref_allele == o.alt && e.alt == o.ref_allele {
                Some(-o.std_beta)
            } else {
                None // mismatch → drop
            };
            let Some(std_beta_out) = aligned else {
                continue;
            };
            out.push(HarmonisedRow {
                rsid: e.rsid.clone(),
                chr_exp: e.chr,
                pos_exp: e.pos,
                alt_exp: e.alt.clone(),
                ref_exp: e.ref_allele.clone(),
                n_exp: e.n,
                std_beta_exp: e.std_beta,
                std_se_exp: e.std_se,
                p_exp: e.p,
                std_beta_out,
                std_se_out: o.std_se,
                p_out: o.p,
                n_out: o.n,
            });
        }
    }
    out
}
