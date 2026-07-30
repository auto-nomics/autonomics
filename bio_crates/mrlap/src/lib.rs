//! `mrlap` — a pure-Rust port of [MRlap](https://github.com/n-mounier/MRlap)
//! (Mounier et al., *Nat Commun* 2024).
//!
//! MRlap performs two-sample Mendelian randomisation from GWAS summary
//! statistics while simultaneously correcting for (a) sample overlap between
//! exposure and outcome cohorts, (b) weak-instrument bias, and (c) Winner's
//! curse. It does so by combining cross-trait LD-score regression (to estimate
//! the overlap via the cross-trait intercept λ) with a closed-form de-biasing
//! formula that recovers the exposure's genetic architecture (π, σ²) from the
//! instruments themselves.
//!
//! ## Pipeline
//! 1. **Tidy** ([`input`]) — standardise each GWAS to `Z`, `std_beta = Z/√N`,
//!    `std_SE = 1/√N`.
//! 2. **Harmonise** ([`harmonise`]) — inner-join on rsid, align outcome alleles.
//! 3. **LDSC** ([`ldsc_runner`]) — cross-trait LD-score regression → h², λ, rg.
//! 4. **MR** ([`mr_runner`]) — IVW + Egger on pruned instruments.
//! 5. **Correct** ([`correction`]) — de-biased effect + bootstrap SE.
//!
//! The correction maths (the novel part) is a faithful, line-by-line port of
//! `get_correction.R`; the LDSC and MR stages reuse the already-validated
//! [`ldsc`] and [`mr`] crates. All RNG (parametric bootstrap) uses a seeded
//! ChaCha8 so runs are reproducible.

#![allow(clippy::doc_lazy_continuation)]

pub mod correction;
pub mod error;
pub mod harmonise;
pub mod input;
pub mod ldsc_runner;
pub mod mr_runner;
pub mod pruning;

use error::Result;

/// Full MRlap result — the three buckets R returns plus the harmonised IV table.
#[derive(Clone, Debug)]
pub struct MrlapResult {
    // ---- MR correction ----
    pub observed_effect: f64,
    pub observed_effect_se: f64,
    pub observed_effect_p: f64,
    pub corrected_effect: f64,
    pub corrected_effect_se: f64,
    pub corrected_effect_p: f64,
    pub test_difference: f64,
    pub p_difference: f64,
    pub egger_b: f64,
    pub egger_se: f64,
    pub egger_intercept_p: f64,
    pub n_iv: usize,
    // ---- LDSC ----
    pub h2_exp: f64,
    pub h2_exp_se: f64,
    pub int_exp: f64,
    pub h2_out: f64,
    pub h2_out_se: f64,
    pub int_out: f64,
    pub gcov: f64,
    pub gcov_se: f64,
    pub rg: f64,
    pub int_crosstrait: f64,
    pub int_crosstrait_se: f64,
    // ---- Genetic architecture ----
    pub polygenicity: f64,  // π
    pub per_snp_heritability: f64, // σ²
    // ---- bootstrap diagnostics ----
    pub n_sim: usize,
    pub neg_h2: bool,
    // ---- instruments ----
    pub ivs: Vec<pruning::Instrument>,
}

/// Configuration for [`run`]. The LDSC inputs are pre-aligned per-SNP vectors;
/// the GWAS tidying + harmonisation + pruning is done by the caller (or the DAG
/// node), so the crate stays pure-algorithm.
pub struct RunInput<'a> {
    pub ldsc: ldsc_runner::LdscInput<'a>,
    /// Pruning mode for instrument selection.
    pub prune: pruning::PruneMode,
    /// Harmonised exposure–outcome data (output of [`harmonise::harmonise`]).
    pub harmonised: &'a [harmonise::HarmonisedRow],
    /// Seed for the correction bootstrap RNG.
    pub seed: u64,
}

/// Run the full MRlap pipeline (LDSC → MR → correction).
///
/// This assumes the GWAS data has already been tidied ([`input::tidy`]) and
/// harmonised ([`harmonise::harmonise`]); the LDSC vectors must be aligned to
/// the *same* SNP set the LD score panel covers (typically HapMap3 + not-MHC).
pub fn run(input: RunInput<'_>) -> Result<MrlapResult> {
    // LDSC stage.
    let ldsc = ldsc_runner::run_ldsc(&input.ldsc)?;

    // Instrument selection.
    let (ivs, _pruned) = pruning::select_instruments(input.harmonised, &input.prune)?;
    if ivs.is_empty() {
        return Err(error::MrlapError::numerical("no instruments survived pruning"));
    }
    let mr = mr_runner::run_mr(&ivs);

    // Correction stage.
    let corr = correction::correct(
        &correction::CorrectionInput {
            iv_std_beta_exp: &ivs.iter().map(|i| i.std_beta_exp).collect::<Vec<_>>(),
            iv_std_se_exp: &ivs.iter().map(|i| i.std_se_exp).collect::<Vec<_>>(),
            lambda: ldsc.lambda,
            lambda_se: ldsc.lambda_se,
            h2_ldsc: ldsc.h2_exp,
            h2_ldsc_se: ldsc.h2_exp_se,
            alpha_obs: mr.alpha_obs,
            alpha_obs_se: mr.alpha_obs_se,
            n_exp: mr.n_exp,
            n_out: mr.n_out,
            mr_threshold: match &input.prune {
                pruning::PruneMode::Distance { mr_threshold, .. } => *mr_threshold,
                pruning::PruneMode::LdProvided { mr_threshold, .. } => *mr_threshold,
                pruning::PruneMode::UserProvided(_) => 5e-8,
            },
        },
        input.seed,
    )?;

    Ok(MrlapResult {
        observed_effect: mr.alpha_obs,
        observed_effect_se: mr.alpha_obs_se,
        observed_effect_p: input::pnorm2_abs(mr.alpha_obs / mr.alpha_obs_se),
        corrected_effect: corr.alpha_corrected,
        corrected_effect_se: corr.alpha_corrected_se,
        corrected_effect_p: input::pnorm2_abs(corr.alpha_corrected / corr.alpha_corrected_se),
        test_difference: corr.test_diff,
        p_difference: corr.p_diff,
        egger_b: mr.egger_b,
        egger_se: mr.egger_se,
        egger_intercept_p: mr.egger_intercept_p,
        n_iv: mr.n_iv,
        h2_exp: ldsc.h2_exp,
        h2_exp_se: ldsc.h2_exp_se,
        int_exp: ldsc.int_exp,
        h2_out: ldsc.h2_out,
        h2_out_se: ldsc.h2_out_se,
        int_out: ldsc.int_out,
        gcov: ldsc.rgcov,
        gcov_se: ldsc.rgcov_se,
        rg: ldsc.rg,
        int_crosstrait: ldsc.lambda,
        int_crosstrait_se: ldsc.lambda_se,
        polygenicity: corr.pi_x,
        per_snp_heritability: corr.sigma2_x,
        n_sim: corr.n_sim,
        neg_h2: corr.neg_h2,
        ivs,
    })
}
