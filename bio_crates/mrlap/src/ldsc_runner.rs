//! Cross-trait LDSC — Rust port of `MRlap/R/run_LDSC.R`.
//!
//! MRlap delegates its LD-score-regression stage to GenomicSEM::ldsc (which is
//! itself a wrapper around the Python LDSC suite). We instead call the
//! already-validated Rust [`ldsc`] crate's [`ldsc::regress::RG`], which is a
//! faithful port of the same Python `RG` class. This produces every quantity
//! the correction stage needs: exposure/outcome h² + SEs, the cross-trait
//! intercept (λ) + SE, gencov, and rg.

use crate::error::Result;
use faer::Mat;

/// Inputs aligned per-SNP (all same length `n_snp`):
///  - `z1`, `z2`: standardised Z-scores for the two traits (= `std_beta*sqrt(N)`)
///  - `n1`, `n2`: per-SNP sample sizes
///  - `ref_ld`, `w_ld`: LD-score and weight-LD-score
///  - `m`: total SNP count for the annotation (the `M` used by LDSC)
#[derive(Clone, Debug)]
pub struct LdscInput<'a> {
    pub z1: &'a [f64],
    pub z2: &'a [f64],
    pub n1: &'a [f64],
    pub n2: &'a [f64],
    pub ref_ld: &'a [f64],
    pub w_ld: &'a [f64],
    pub m: f64,
    /// Block-jackknife block count (default 200).
    pub n_blocks: usize,
}

/// LDSC outputs that feed the MRlap correction + reporting (matches the list
/// returned by `run_LDSC.R`).
#[derive(Clone, Debug)]
pub struct LdscResult {
    pub h2_exp: f64,
    pub h2_exp_se: f64,
    pub int_exp: f64,
    pub h2_out: f64,
    pub h2_out_se: f64,
    pub int_out: f64,
    /// Cross-trait intercept λ (sample-overlap proxy).
    pub lambda: f64,
    pub lambda_se: f64,
    /// Genetic covariance σ_g and its SE.
    pub rgcov: f64,
    pub rgcov_se: f64,
    /// Genetic correlation ρ_g (NaN if either h² ≤ 0).
    pub rg: f64,
}

/// Run cross-trait LDSC. Intercepts are left free (the LDSC default), and the
/// two-step floor is set to 30 — matching `run_LDSC.R` (which calls
/// `GenomicSEM::ldsc` with no `intercept` argument, leaving them unconstrained).
pub fn run_ldsc(input: &LdscInput<'_>) -> Result<LdscResult> {
    let n = input.z1.len();
    if input.z2.len() != n
        || input.n1.len() != n
        || input.n2.len() != n
        || input.ref_ld.len() != n
        || input.w_ld.len() != n
    {
        return Err(crate::error::MrlapError::input(
            "ldsc_runner: per-SNP column length mismatch",
        ));
    }
    let x = Mat::from_fn(n, 1, |i, _| input.ref_ld[i]);
    let m = vec![input.m];

    let rg = ldsc::regress::RG::new(
        input.z1,
        input.z2,
        &x,
        input.w_ld,
        input.n1,
        input.n2,
        &m,
        None, // intercept_hsq1 free
        None, // intercept_hsq2 free
        None, // intercept_gencov free
        input.n_blocks,
        Some(30.0), // two-step default
    )?;

    Ok(LdscResult {
        h2_exp: rg.hsq1.reg.tot,
        h2_exp_se: rg.hsq1.reg.tot_se,
        int_exp: rg.hsq1.reg.intercept.unwrap_or(1.0),
        h2_out: rg.hsq2.reg.tot,
        h2_out_se: rg.hsq2.reg.tot_se,
        int_out: rg.hsq2.reg.intercept.unwrap_or(1.0),
        lambda: rg.gencov.reg.intercept.unwrap_or(0.0),
        lambda_se: rg.gencov.reg.intercept_se.unwrap_or(f64::NAN),
        rgcov: rg.gencov.reg.tot,
        rgcov_se: rg.gencov.reg.tot_se,
        rg: rg.rg_ratio,
    })
}
