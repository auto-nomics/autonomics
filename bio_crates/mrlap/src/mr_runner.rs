//! IVW-MR + MR-Egger — Rust port of `MRlap/R/run_MR.R` (lines 163-184).
//!
//! Delegates the regression to the already-validated `mr` crate
//! ([`mr::methods::ivw::mr_ivw`] and [`mr::methods::egger::mr_egger_regression`]),
//! which faithfully reproduce TwoSampleMR's implementations. Instrument
//! selection (thresholding + pruning + reverse filtering) lives in
//! [`crate::pruning`].

use crate::pruning::Instrument;
use mr::methods::egger::mr_egger_regression;
use mr::methods::ivw::mr_ivw;
use mr::Parameters;

/// Result of the MR stage — matches the values MRlap reports.
#[derive(Clone, Debug)]
pub struct MrResult {
    pub alpha_obs: f64,
    pub alpha_obs_se: f64,
    pub egger_b: f64,
    pub egger_se: f64,
    pub egger_intercept_p: f64,
    pub n_exp: f64, // mean exposure N over the instruments
    pub n_out: f64, // mean outcome N over the instruments
    pub n_iv: usize,
}

/// Run IVW-MR + Egger on a set of instruments (`run_MR.R` lines 165-169).
///
/// Inputs are the exposure/outcome standardised effects & SEs of the pruned IVs.
pub fn run_mr(ivs: &[Instrument]) -> MrResult {
    let b_exp: Vec<f64> = ivs.iter().map(|i| i.std_beta_exp).collect();
    let b_out: Vec<f64> = ivs.iter().map(|i| i.std_beta_out).collect();
    let se_exp: Vec<f64> = ivs.iter().map(|i| i.std_se_exp).collect();
    let se_out: Vec<f64> = ivs.iter().map(|i| i.std_se_out).collect();

    let params = Parameters::default();
    let ivw = mr_ivw(&b_exp, &b_out, &se_exp, &se_out, &params);
    let egger = mr_egger_regression(&b_exp, &b_out, &se_exp, &se_out, &params);

    let n_exp = mean(ivs.iter().map(|i| i.n_exp));
    let n_out = mean(ivs.iter().map(|i| i.n_out));

    MrResult {
        alpha_obs: ivw.b,
        alpha_obs_se: ivw.se,
        egger_b: egger.b,
        egger_se: egger.se,
        egger_intercept_p: egger.pval_i.unwrap_or(f64::NAN),
        n_exp,
        n_out,
        n_iv: ivs.len(),
    }
}

fn mean<I: Iterator<Item = f64>>(it: I) -> f64 {
    let (sum, n) = it.fold((0.0_f64, 0usize), |(s, n), v| (s + v, n + 1));
    if n == 0 {
        f64::NAN
    } else {
        sum / n as f64
    }
}
