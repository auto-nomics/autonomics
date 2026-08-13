//! Summary statistic munging — port of `R/munge.R`.
//!
//! QC and harmonize raw GWAS summary statistics for use with LDSC.

use std::collections::HashMap;

use crate::error::{GenomicSemError, Result};

/// Column name mapping for recognized GWAS sumstat columns.
/// Mirrors `.get_renamed_colnames` in `R/utils.R`.
pub fn map_column_names(header: &[String]) -> HashMap<String, String> {
    let aliases: &[(&str, &[&str])] = &[
        (
            "SNP",
            &[
                "SNP",
                "SNPID",
                "RSID",
                "RS_NUMBER",
                "RS_NUMBERS",
                "MARKERNAME",
                "ID",
                "PREDICTOR",
                "SNP_ID",
                "VARIANTID",
                "VARIANT_ID",
                "RSIDS",
                "RS_ID",
            ],
        ),
        (
            "A1",
            &[
                "A1",
                "ALLELE1",
                "EFFECT_ALLELE",
                "INC_ALLELE",
                "REFERENCE_ALLELE",
                "EA",
                "REF",
            ],
        ),
        (
            "A2",
            &[
                "A2",
                "ALLELE2",
                "ALLELE0",
                "OTHER_ALLELE",
                "NON_EFFECT_ALLELE",
                "DEC_ALLELE",
                "OA",
                "NEA",
                "ALT",
                "A0",
            ],
        ),
        (
            "effect",
            &[
                "OR",
                "B",
                "BETA",
                "LOG_ODDS",
                "EFFECTS",
                "EFFECT",
                "SIGNED_SUMSTAT",
                "EST",
                "BETA1",
                "LOGOR",
            ],
        ),
        ("INFO", &["INFO", "IMPINFO"]),
        (
            "P",
            &[
                "P",
                "PVALUE",
                "PVAL",
                "P_VALUE",
                "P-VALUE",
                "P.VALUE",
                "P_VAL",
                "GC_PVALUE",
                "WALD_P",
            ],
        ),
        (
            "N",
            &[
                "N",
                "WEIGHT",
                "NCOMPLETESAMPLES",
                "TOTALSAMPLESIZE",
                "TOTALN",
                "TOTAL_N",
                "N_COMPLETE_SAMPLES",
                "SAMPLESIZE",
                "NEFF",
                "N_EFF",
                "N_EFFECTIVE",
                "SUMNEFF",
            ],
        ),
        (
            "MAF",
            &[
                "MAF",
                "CEUAF",
                "FREQ1",
                "EAF",
                "FREQ1.HAPMAP",
                "FREQALLELE1HAPMAPCEU",
                "FREQ.ALLELE1.HAPMAPCEU",
                "EFFECT_ALLELE_FREQ",
                "FREQ.A1",
                "A1FREQ",
                "ALLELEFREQ",
                "EFFECT_ALLELE_FREQUENCY",
            ],
        ),
        (
            "Z",
            &[
                "Z",
                "ZSCORE",
                "Z-SCORE",
                "ZSTATISTIC",
                "ZSTAT",
                "Z-STATISTIC",
            ],
        ),
        (
            "SE",
            &[
                "STDERR",
                "SE",
                "STDERRLOGOR",
                "SEBETA",
                "STANDARDERROR",
                "STANDARD_ERROR",
            ],
        ),
    ];

    let mut mapping = HashMap::new();
    let header_upper: Vec<String> = header.iter().map(|h| h.to_uppercase()).collect();

    for (canonical, candidates) in aliases {
        for &cand in *candidates {
            if let Some(pos) = header_upper.iter().position(|h| h == cand) {
                mapping.insert(header[pos].clone(), canonical.to_string());
                break;
            }
        }
    }

    mapping
}

/// Munged summary statistics for one trait.
#[derive(Clone, Debug)]
pub struct MungedSumstats {
    pub snp: Vec<String>,
    pub n: Vec<f64>,
    pub z: Vec<f64>,
    pub a1: Vec<String>,
    pub a2: Vec<String>,
}

/// Configuration for `munge`.
#[derive(Clone, Debug)]
pub struct MungeConfig {
    pub trait_names: Vec<String>,
    pub n: Vec<Option<f64>>,
    pub info_filter: f64,
    pub maf_filter: f64,
}

impl Default for MungeConfig {
    fn default() -> Self {
        Self {
            trait_names: Vec::new(),
            n: Vec::new(),
            info_filter: 0.9,
            maf_filter: 0.01,
        }
    }
}

/// Compute Z-score from effect and p-value, as GenomicSEM does.
///
/// `Z = sign(effect) * sqrt(qchisq(P, 1, lower.tail = FALSE))`
pub fn z_from_p(effect: f64, p: f64) -> f64 {
    let sign = if effect >= 0.0 { 1.0 } else { -1.0 };
    let q = crate::stats::qchisq_sf(p, 1.0);
    sign * q.sqrt()
}

/// Detect whether the effect column is odds ratios (OR) by checking if
/// the median is > 1.5 (GenomicSEM's heuristic).
pub fn is_odds_ratio(effects: &[f64]) -> bool {
    let mut sorted = effects.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = sorted.len();
    if n == 0 {
        return false;
    }
    let median = if n % 2 == 0 {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    } else {
        sorted[n / 2]
    };
    (median - 1.0).abs() < 0.5 && median > 0.5
}

/// Convert OR to log(OR) if needed.
pub fn log_if_or(effect: f64, is_or: bool) -> f64 {
    if is_or && effect > 0.0 {
        effect.ln()
    } else {
        effect
    }
}

/// Flip effect sign if A1 doesn't match reference.
pub fn align_effect(effect: f64, a1_data: &str, a1_ref: &str, a2_ref: &str) -> f64 {
    if a1_data != a1_ref {
        if a1_data == a2_ref {
            -effect
        } else {
            effect // ambiguous, keep as is
        }
    } else {
        effect
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_column_names() {
        let header = vec![
            "CHR".to_string(),
            "BP".to_string(),
            "SNP".to_string(),
            "A1".to_string(),
            "A2".to_string(),
            "B".to_string(),
            "P".to_string(),
            "N".to_string(),
        ];
        let mapping = map_column_names(&header);
        assert_eq!(mapping.get("SNP"), Some(&"SNP".to_string()));
        assert_eq!(mapping.get("B"), Some(&"effect".to_string()));
        assert_eq!(mapping.get("P"), Some(&"P".to_string()));
    }

    #[test]
    fn test_z_from_p() {
        let z = z_from_p(0.5, 0.05);
        assert!(z.abs() > 1.9 && z.abs() < 2.0);
        assert!(z > 0.0); // positive effect
    }

    #[test]
    fn test_is_odds_ratio() {
        assert!(is_odds_ratio(&[1.1, 1.05, 0.95, 1.0]));
        assert!(!is_odds_ratio(&[0.1, -0.05, 0.02, -0.01]));
    }
}
