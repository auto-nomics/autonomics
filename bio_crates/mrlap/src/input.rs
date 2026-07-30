//! Input munging — Rust port of `MRlap/R/tidy_inputGWAS.R`.
//!
//! Takes a "raw" GWAS record (any combination of BETA/SE, OR/SE, or Z, plus
//! alleles and sample size) and produces the standardised quantities MRlap uses
//! everywhere:
//!
//! - `Z`            — z-score (from `Z` column, or `BETA/SE`, or `log(OR)/SE`)
//! - `std_beta`     — `Z / sqrt(N)`  (standardised effect used by MR + LDSC)
//! - `std_SE`       — `1 / sqrt(N)`  (standardised SE)
//! - `p`            — two-sided normal p-value of Z
//!
//! SNPs with non-finite Z / N≤0 are dropped, mirroring the R reference (which
//! relies on downstream `na` propagation). No HLA filtering is performed — the
//! reference also has it commented out (lines 192-196 of `tidy_inputGWAS.R`).

use crate::error::{MrlapError, Result};
use statrs::distribution::{Continuous, ContinuousCDF, Normal};

/// A raw GWAS row as it arrives from any column-naming convention. Fields the
/// caller resolved from BETA/SE/OR/Z aliases; at least one of (`z`, `beta`+`se`,
/// `or`+`se`) must be present.
#[derive(Clone, Debug, Default)]
pub struct RawGwasRow {
    pub rsid: String,
    pub chr: Option<i32>,
    pub pos: Option<i64>,
    pub alt: String,   // effect allele (A1)
    pub ref_allele: String, // reference allele (A2)
    pub beta: Option<f64>,
    pub or: Option<f64>,
    pub se: Option<f64>,
    pub z: Option<f64>,
    pub n: f64,
}

/// A tidied GWAS row — the common currency of all downstream MRlap stages.
#[derive(Clone, Debug)]
pub struct TidyRow {
    pub rsid: String,
    pub chr: Option<i32>,
    pub pos: Option<i64>,
    pub alt: String,
    pub ref_allele: String,
    pub z: f64,
    pub n: f64,
    pub std_beta: f64,
    pub std_se: f64,
    pub p: f64,
}

fn std_normal() -> Normal {
    Normal::new(0.0, 1.0).expect("standard normal")
}

/// Two-sided p-value of a standard-normal z (R's `2*pnorm(-abs(z))`).
pub fn pnorm2_abs(z: f64) -> f64 {
    if !z.is_finite() {
        return f64::NAN;
    }
    2.0 * std_normal().cdf(-z.abs())
}

/// `-qnorm(p/2)` — the two-sided z threshold (R's `-stats::qnorm(MR_threshold/2)`).
pub fn z_threshold(two_sided_p: f64) -> f64 {
    -std_normal().inverse_cdf(two_sided_p / 2.0)
}

/// Standard normal density (R's `dnorm(x)`).
pub fn dnorm(x: f64) -> f64 {
    std_normal().pdf(x)
}

/// `pnorm(x)` — standard normal CDF.
pub fn pnorm(x: f64) -> f64 {
    std_normal().cdf(x)
}

/// Tidy a slice of raw rows. Mirrors `tidy_inputGWAS`'s per-row Z derivation
/// and standardisation. Rows with missing rsid, non-finite Z, or N≤0 are
/// dropped (the R code lets them become NA and they're filtered downstream).
pub fn tidy(rows: &[RawGwasRow], need_chrpos: bool) -> Result<Vec<TidyRow>> {
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        if r.rsid.is_empty() {
            continue;
        }
        if need_chrpos && (r.chr.is_none() || r.pos.is_none()) {
            return Err(MrlapError::input(format!(
                "rsid {} needs chr/pos but they are missing",
                r.rsid
            )));
        }
        // Z: explicit > BETA/SE > log(OR)/SE
        let z = if let Some(z) = r.z {
            z
        } else if let (Some(b), Some(se)) = (r.beta, r.se) {
            if se.is_finite() && se.abs() > 0.0 {
                b / se
            } else {
                f64::NAN
            }
        } else if let (Some(or), Some(se)) = (r.or, r.se) {
            if se.is_finite() && se.abs() > 0.0 && or > 0.0 {
                or.ln() / se
            } else {
                f64::NAN
            }
        } else {
            return Err(MrlapError::input(format!(
                "rsid {}: no effect column (Z, or BETA+SE, or OR+SE)",
                r.rsid
            )));
        };

        if !z.is_finite() || r.n.is_nan() || r.n <= 0.0 {
            continue;
        }
        let sqrt_n = r.n.sqrt();
        out.push(TidyRow {
            rsid: r.rsid.clone(),
            chr: r.chr,
            pos: r.pos,
            alt: r.alt.clone(),
            ref_allele: r.ref_allele.clone(),
            z,
            n: r.n,
            std_beta: z / sqrt_n,
            std_se: 1.0 / sqrt_n,
            p: pnorm2_abs(z),
        });
    }
    Ok(out)
}
