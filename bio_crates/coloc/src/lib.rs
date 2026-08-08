//! Rust port of the R package [`coloc`](https://cran.r-project.org/package=coloc)
//! (Wallace & Giambartolomei) — colocalisation analysis from GWAS summary
//! statistics under the single-causal-variant assumption.
//!
//! The package tests whether two traits share the *same* causal variant at a
//! locus by computing approximate Bayes factors (ABFs) at each SNP and
//! combining them into five posterior-probability hypotheses:
//!
//! | Hypothesis | Meaning                                        |
//! |------------|-------------------------------------------------|
//! | **H0**     | No association with either trait                 |
//! | **H1**     | Association with trait 1 only                    |
//! | **H2**     | Association with trait 2 only                    |
//! | **H3**     | Association with both traits, *different* variants |
//! | **H4**     | Association with both traits, *shared* variant   |
//!
//! Two analyses are provided:
//!
//! * [`coloc_abf`] — fully Bayesian colocalisation (the primary entry point).
//! * [`finemap_abf`] — Bayesian fine-mapping for a single trait.
//!
//! Both functions accept datasets that supply either (beta, varbeta) or
//! (pvalues, MAF, N), mirroring the R package's flexibility.
//!
//! # References
//!
//! - Giambartolomei C, Vukcevic D, Schadt EE, Franke L, Hingorani AD, Wallace C,
//!   Plagnol V (2014). *Bayesian test for colocalisation between pairs of
//!   genetic association studies using summary statistics.* PLoS Genetics
//!   10(5):e1004383.

pub mod error;

pub use error::{ColocError, Result};

use serde::{Deserialize, Serialize};

// =====================================================================
// Data types
// =====================================================================

/// Trait type for a coloc dataset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TraitType {
    /// Quantitative trait.
    Quant,
    /// Case–control (binary) trait.
    CC,
}

impl std::fmt::Display for TraitType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TraitType::Quant => f.write_str("quant"),
            TraitType::CC => f.write_str("cc"),
        }
    }
}

impl std::str::FromStr for TraitType {
    type Err = ColocError;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "quant" => Ok(TraitType::Quant),
            "cc" => Ok(TraitType::CC),
            other => Err(ColocError::BadTraitType(other.into())),
        }
    }
}

/// A coloc dataset — the summary statistics for one trait at one locus.
///
/// Fields mirror the R `list` elements documented in `check_dataset()`.
/// Either (`beta`, `varbeta`) or (`pvalues`, `maf`, `n`) must be supplied.
/// The `type` field is always required.  For `type = "cc"`, `s` is required
/// if `pvalues` are used.  For `type = "quant"`, `sd_y` must be supplied or
/// can be estimated from (`varbeta`, `maf`, `n`).
#[derive(Clone, Debug)]
pub struct Dataset {
    /// SNP identifiers.
    pub snp: Vec<String>,
    /// Regression coefficients (optional — alternative to `pvalues`).
    pub beta: Option<Vec<f64>>,
    /// Variance of `beta`.
    pub varbeta: Option<Vec<f64>>,
    /// P-values (optional — alternative to `beta`/`varbeta`).
    pub pvalues: Option<Vec<f64>>,
    /// Minor allele frequencies (required when using `pvalues`).
    pub maf: Option<Vec<f64>>,
    /// Sample size (scalar). Required when using `pvalues`.
    pub n: Option<f64>,
    /// Trait type.
    pub r#type: TraitType,
    /// For case–control: proportion of cases.  Required when `type = CC` and
    /// using `pvalues`.
    pub s: Option<f64>,
    /// For quantitative: population SD of the trait.  If absent, estimated
    /// from (`varbeta`, `maf`, `n`).
    pub sd_y: Option<f64>,
    /// Optional genomic positions.
    pub position: Option<Vec<f64>>,
}

/// Effect-size priors — variance of the prior effect distribution.
///
/// Defaults match the R package: 0.15 for quantitative traits (× sdY),
/// 0.2 for case–control.
#[derive(Clone, Debug)]
pub struct EffectPriors {
    pub quant: f64,
    pub cc: f64,
}

impl Default for EffectPriors {
    fn default() -> Self {
        Self {
            quant: 0.15,
            cc: 0.2,
        }
    }
}

// =====================================================================
// Numerical helpers
// =====================================================================

/// Log-sum-exp: `max(x) + log(sum(exp(x - max(x))))`.
///
/// Direct port of R's `logsum()`.
#[inline]
pub fn logsum(x: &[f64]) -> f64 {
    debug_assert!(!x.is_empty(), "logsum: empty slice");
    let my_max = x.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if my_max == f64::NEG_INFINITY {
        return f64::NEG_INFINITY;
    }
    let s: f64 = x.iter().map(|&v| (v - my_max).exp()).sum();
    my_max + s.ln()
}

/// Log-diff-exp: `max(x,y) + log(max(exp(x-max) - exp(y-max), 0))`.
///
/// Direct port of R's `logdiff()`.
#[inline]
pub fn logdiff(x: f64, y: f64) -> f64 {
    let my_max = x.max(y);
    let diff = (x - my_max).exp() - (y - my_max).exp();
    my_max + diff.max(0.0).ln()
}

// =====================================================================
// Variance of MLE beta
// =====================================================================

/// Variance of MLE beta for a quantitative trait with `var(Y) = 1`.
///
/// `1 / (2 * N * f * (1 - f))`
#[inline]
pub fn var_data(f: f64, n: f64) -> f64 {
    1.0 / (2.0 * n * f * (1.0 - f))
}

/// Variance of MLE beta for a case–control trait.
///
/// `1 / (2 * N * f * (1 - f) * s * (1 - s))`
#[inline]
pub fn var_data_cc(f: f64, n: f64, s: f64) -> f64 {
    1.0 / (2.0 * n * f * (1.0 - f) * s * (1.0 - s))
}

// =====================================================================
// Approximate Bayes Factors
// =====================================================================

/// Intermediate quantities from ABF calculation.
#[derive(Clone, Debug, Default)]
pub struct BfIntermediates {
    /// Variance of beta-hat (V).
    pub v: f64,
    /// Z score (`beta/sqrt(varbeta)` or derived from p).
    pub z: f64,
    /// Shrinkage factor `r = sd_prior² / (sd_prior² + V)`.
    pub r: f64,
    /// Log approximate Bayes factor.
    pub l_abf: f64,
}

/// Approximate Bayes Factors from p-values.
///
/// Direct port of R's `approx.bf.p()`.
pub fn approx_bf_p(
    p: &[f64],
    f: &[f64],
    r#type: TraitType,
    n: f64,
    s: Option<f64>,
) -> Vec<BfIntermediates> {
    let sd_prior = match r#type {
        TraitType::Quant => 0.15,
        TraitType::CC => 0.2,
    };
    p.iter()
        .zip(f.iter())
        .map(|(&pi, &fi)| {
            let v = match r#type {
                TraitType::Quant => var_data(fi, n),
                TraitType::CC => var_data_cc(fi, n, s.unwrap_or(0.5)),
            };
            // z = qnorm(0.5 * p, lower.tail = FALSE)
            let z = normal_inv_cdf_upper(0.5 * pi);
            let r = sd_prior * sd_prior / (sd_prior * sd_prior + v);
            let l_abf = 0.5 * ((1.0 - r).ln() + r * z * z);
            BfIntermediates { v, z, r, l_abf }
        })
        .collect()
}

/// Approximate Bayes Factors from supplied effect estimates and variances.
///
/// Direct port of R's `approx.bf.estimates()`.
pub fn approx_bf_estimates(
    z: &[f64],
    v: &[f64],
    r#type: TraitType,
    sd_y: f64,
    effect_priors: &EffectPriors,
) -> Vec<BfIntermediates> {
    let sd_prior = match r#type {
        TraitType::Quant => effect_priors.quant * sd_y,
        TraitType::CC => effect_priors.cc,
    };
    z.iter()
        .zip(v.iter())
        .map(|(&zi, &vi)| {
            let r = sd_prior * sd_prior / (sd_prior * sd_prior + vi);
            let l_abf = 0.5 * ((1.0 - r).ln() + r * zi * zi);
            BfIntermediates {
                v: vi,
                z: zi,
                r,
                l_abf,
            }
        })
        .collect()
}

// =====================================================================
// sdY estimation
// =====================================================================

/// Estimate the trait standard deviation from variance of coefficients, MAF,
/// and sample size.
///
/// `var(beta-hat) ≈ var(Y) / (n * var(X))` where `var(X) = 2*maf*(1-maf)`.
/// So we regress `n * var(X)` against `1 / var(beta)` and take `sqrt(slope)`.
///
/// Direct port of R's `sdY.est()`.  Uses OLS through the origin.
pub fn sd_y_est(vbeta: &[f64], maf: &[f64], n: f64) -> Result<f64> {
    let oneover: Vec<f64> = vbeta.iter().map(|&v| 1.0 / v).collect();
    let nvx: Vec<f64> = maf
        .iter()
        .map(|&f| 2.0 * n * f * (1.0 - f))
        .collect();
    // OLS through origin: nvx = coef * oneover  →  coef = sum(nvx * oneover) / sum(oneover^2)
    let num: f64 = oneover.iter().zip(nvx.iter()).map(|(&o, &x)| o * x).sum();
    let den: f64 = oneover.iter().map(|&o| o * o).sum();
    let cf = num / den;
    if cf < 0.0 {
        return Err(ColocError::NegativeSdY);
    }
    Ok(cf.sqrt())
}

// =====================================================================
// Prior adjustment
// =====================================================================

/// Clamp `p` so that `nsnps * p < 1`.
///
/// Direct port of R's `adjust_prior()`.
pub fn adjust_prior(p: f64, nsnps: usize) -> f64 {
    if nsnps as f64 * p >= 1.0 {
        1.0 / (nsnps as f64 + 1.0)
    } else {
        p
    }
}

// =====================================================================
// Process dataset → lABF
// =====================================================================

/// Result of processing one dataset: log ABF per SNP.
#[derive(Clone, Debug)]
pub struct ProcessedDataset {
    pub snp: Vec<String>,
    pub l_abf: Vec<f64>,
    /// Optional intermediates (V, z, r per SNP).
    pub v: Option<Vec<f64>>,
    pub z: Option<Vec<f64>>,
    pub r: Option<Vec<f64>>,
    pub position: Option<Vec<f64>>,
}

/// Process a dataset, computing log ABF per SNP.
///
/// Direct port of R's `process.dataset()`.
pub fn process_dataset(d: &Dataset) -> Result<ProcessedDataset> {
    if let (Some(beta), Some(varbeta)) = (&d.beta, &d.varbeta) {
        // Use beta / varbeta.
        let sd_y = if d.r#type == TraitType::Quant {
            match d.sd_y {
                Some(sdy) => sdy,
                None => {
                    let maf = d.maf.as_ref().ok_or(ColocError::MissingMafForSdY)?;
                    let n = d.n.ok_or(ColocError::MissingNForSdY)?;
                    sd_y_est(varbeta, maf, n)?
                }
            }
        } else {
            d.sd_y.unwrap_or(1.0)
        };
        let z: Vec<f64> = beta
            .iter()
            .zip(varbeta.iter())
            .map(|(&b, &vb)| b / vb.sqrt())
            .collect();
        let bfs = approx_bf_estimates(&z, varbeta, d.r#type, sd_y, &EffectPriors::default());
        Ok(ProcessedDataset {
            snp: d.snp.clone(),
            l_abf: bfs.iter().map(|b| b.l_abf).collect(),
            v: Some(bfs.iter().map(|b| b.v).collect()),
            z: Some(bfs.iter().map(|b| b.z).collect()),
            r: Some(bfs.iter().map(|b| b.r).collect()),
            position: d.position.clone(),
        })
    } else if let (Some(pvalues), Some(maf), Some(n)) = (&d.pvalues, &d.maf, d.n) {
        // Use p-values / MAF approximation.
        let bfs = approx_bf_p(pvalues, maf, d.r#type, n, d.s);
        Ok(ProcessedDataset {
            snp: d.snp.clone(),
            l_abf: bfs.iter().map(|b| b.l_abf).collect(),
            v: Some(bfs.iter().map(|b| b.v).collect()),
            z: Some(bfs.iter().map(|b| b.z).collect()),
            r: Some(bfs.iter().map(|b| b.r).collect()),
            position: d.position.clone(),
        })
    } else {
        Err(ColocError::InsufficientData)
    }
}

// =====================================================================
// Combine ABFs → posterior probabilities
// =====================================================================

/// Posterior probabilities for the five hypotheses.
#[derive(Clone, Debug, Default)]
pub struct PosteriorProbs {
    pub pp_h0: f64,
    pub pp_h1: f64,
    pub pp_h2: f64,
    pub pp_h3: f64,
    pub pp_h4: f64,
}

impl PosteriorProbs {
    /// As a 5-element array `[H0, H1, H2, H3, H4]`.
    pub fn as_array(&self) -> [f64; 5] {
        [self.pp_h0, self.pp_h1, self.pp_h2, self.pp_h3, self.pp_h4]
    }
}

/// Combine log ABFs from two datasets under five colocalisation hypotheses.
///
/// Direct port of R's `combine.abf()`.
pub fn combine_abf(l1: &[f64], l2: &[f64], p1: f64, p2: f64, p12: f64) -> PosteriorProbs {
    assert_eq!(l1.len(), l2.len());
    let lsum: Vec<f64> = l1.iter().zip(l2.iter()).map(|(&a, &b)| a + b).collect();

    let l_h0 = 0.0;
    let l_h1 = p1.ln() + logsum(l1);
    let l_h2 = p2.ln() + logsum(l2);
    // logdiff(logsum(l1) + logsum(l2), logsum(lsum))
    let l_h3 = p1.ln() + p2.ln() + logdiff(logsum(l1) + logsum(l2), logsum(&lsum));
    let l_h4 = p12.ln() + logsum(&lsum);

    let all = [l_h0, l_h1, l_h2, l_h3, l_h4];
    let denom = logsum(&all);
    let pp: Vec<f64> = all.iter().map(|&v| (v - denom).exp()).collect();

    PosteriorProbs {
        pp_h0: pp[0],
        pp_h1: pp[1],
        pp_h2: pp[2],
        pp_h3: pp[3],
        pp_h4: pp[4],
    }
}

/// Weighted version of `combine_abf` — per-SNP prior weights.
///
/// Direct port of R's `combine_abf_weighted()`.
pub fn combine_abf_weighted(
    l1: &[f64],
    l2: &[f64],
    p1: f64,
    p2: f64,
    p12: f64,
    prior_weights1: Option<&[f64]>,
    prior_weights2: Option<&[f64]>,
) -> PosteriorProbs {
    assert_eq!(l1.len(), l2.len());
    let q = l1.len();

    let (p1_vec, p2_vec): (Vec<f64>, Vec<f64>) = match (prior_weights1, prior_weights2) {
        (Some(w1), Some(w2)) => {
            let s1: f64 = w1.iter().sum();
            let s2: f64 = w2.iter().sum();
            let p1v: Vec<f64> = w1.iter().map(|&w| q as f64 * p1 * (w / s1)).collect();
            let p2v: Vec<f64> = w2.iter().map(|&w| q as f64 * p2 * (w / s2)).collect();
            (p1v, p2v)
        }
        (Some(w1), None) => {
            let s1: f64 = w1.iter().sum();
            let p1v: Vec<f64> = w1.iter().map(|&w| q as f64 * p1 * (w / s1)).collect();
            let p2v = vec![p2; q];
            (p1v, p2v)
        }
        (None, Some(w2)) => {
            let p1v = vec![p1; q];
            let s2: f64 = w2.iter().sum();
            let p2v: Vec<f64> = w2.iter().map(|&w| q as f64 * p2 * (w / s2)).collect();
            (p1v, p2v)
        }
        (None, None) => {
            let p1v = vec![p1; q];
            let p2v = vec![p2; q];
            (p1v, p2v)
        }
    };

    // p12_vec = p1_vec * p2_vec * (p12 / (p1 * p2))
    let factor = p12 / (p1 * p2);
    let p12_vec: Vec<f64> = p1_vec
        .iter()
        .zip(p2_vec.iter())
        .map(|(&a, &b)| a * b * factor)
        .collect();

    let lsum: Vec<f64> = l1.iter().zip(l2.iter()).map(|(&a, &b)| a + b).collect();
    let l_h0 = 0.0;
    let lp1_l1: Vec<f64> = p1_vec.iter().zip(l1.iter()).map(|(&p, &l)| p.ln() + l).collect();
    let lp2_l2: Vec<f64> = p2_vec.iter().zip(l2.iter()).map(|(&p, &l)| p.ln() + l).collect();
    let l_h1 = logsum(&lp1_l1);
    let l_h2 = logsum(&lp2_l2);

    // logdiff(logsum(lp1_l1) + logsum(lp2_l2), logsum(lp1+lp2+lsum))
    let lp12_lsum: Vec<f64> = p1_vec
        .iter()
        .zip(p2_vec.iter())
        .zip(lsum.iter())
        .map(|((&a, &b), &s)| a.ln() + b.ln() + s)
        .collect();
    let l_h3 = logdiff(logsum(&lp1_l1) + logsum(&lp2_l2), logsum(&lp12_lsum));

    let lp12sum: Vec<f64> = p12_vec
        .iter()
        .zip(lsum.iter())
        .map(|(&p, &s)| p.ln() + s)
        .collect();
    let l_h4 = logsum(&lp12sum);

    let all = [l_h0, l_h1, l_h2, l_h3, l_h4];
    let denom = logsum(&all);
    let pp: Vec<f64> = all.iter().map(|&v| (v - denom).exp()).collect();

    PosteriorProbs {
        pp_h0: pp[0],
        pp_h1: pp[1],
        pp_h2: pp[2],
        pp_h3: pp[3],
        pp_h4: pp[4],
    }
}

// =====================================================================
// Merge datasets on SNP
// =====================================================================

/// Merge two processed datasets on SNP identifiers, preserving the order
/// of dataset 1.
///
/// Mirrors R's `merge(df1, df2)` which sorts by the merge key.
fn merge_on_snp(
    df1: &ProcessedDataset,
    df2: &ProcessedDataset,
) -> Vec<(usize, usize)> {
    // Build snp → index for df2.
    let mut idx2: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for (i, s) in df2.snp.iter().enumerate() {
        idx2.insert(s.as_str(), i);
    }
    // Find common SNPs — R's merge() sorts by key, so we match that.
    let mut pairs: Vec<(usize, usize)> = df1
        .snp
        .iter()
        .enumerate()
        .filter_map(|(i, s)| idx2.get(s.as_str()).map(|&j| (i, j)))
        .collect();
    // R's merge() sorts the result by the merge key. We sort by df1 snp name
    // to match.
    pairs.sort_by(|&(i, _), &(j, _)| df1.snp[i].cmp(&df1.snp[j]));
    pairs
}

// =====================================================================
// coloc.abf — main colocalisation analysis
// =====================================================================

/// Output of [`coloc_abf`].
#[derive(Clone, Debug)]
pub struct ColocAbfResult {
    /// Number of common SNPs.
    pub nsnps: usize,
    /// Posterior probabilities.
    pub pp: PosteriorProbs,
    /// Per-SNP posterior probability of being the shared causal variant (H4).
    pub snp_pp_h4: Vec<f64>,
    /// SNP identifiers (merged).
    pub snp: Vec<String>,
    /// Priors used (after adjustment).
    pub p1: f64,
    pub p2: f64,
    pub p12: f64,
    /// Per-SNP log ABFs for trait 1 (merged).
    pub l_abf_df1: Vec<f64>,
    /// Per-SNP log ABFs for trait 2 (merged).
    pub l_abf_df2: Vec<f64>,
    /// Optional per-SNP positions.
    pub position: Option<Vec<f64>>,
}

/// Options for [`coloc_abf`].
#[derive(Clone, Debug)]
pub struct ColocAbfOptions {
    pub p1: f64,
    pub p2: f64,
    pub p12: f64,
    pub prior_weights1: Option<Vec<f64>>,
    pub prior_weights2: Option<Vec<f64>>,
    /// Common MAF to use for both datasets (overrides dataset MAF).
    pub maf: Option<Vec<f64>>,
}

impl Default for ColocAbfOptions {
    fn default() -> Self {
        Self {
            p1: 1e-4,
            p2: 1e-4,
            p12: 1e-5,
            prior_weights1: None,
            prior_weights2: None,
            maf: None,
        }
    }
}

/// Fully Bayesian colocalisation analysis using Bayes factors.
///
/// Direct port of R's `coloc.abf()`.
pub fn coloc_abf(
    dataset1: &Dataset,
    dataset2: &Dataset,
    opts: &ColocAbfOptions,
) -> Result<ColocAbfResult> {
    // Apply common MAF if the dataset doesn't have one.
    let d1 = if dataset1.maf.is_none() {
        if let Some(maf) = &opts.maf {
            let mut d = dataset1.clone();
            d.maf = Some(maf.clone());
            d
        } else {
            dataset1.clone()
        }
    } else {
        dataset1.clone()
    };
    let d2 = if dataset2.maf.is_none() {
        if let Some(maf) = &opts.maf {
            let mut d = dataset2.clone();
            d.maf = Some(maf.clone());
            d
        } else {
            dataset2.clone()
        }
    } else {
        dataset2.clone()
    };

    check_dataset(&d1, "")?;
    check_dataset(&d2, "")?;

    let df1 = process_dataset(&d1)?;
    let df2 = process_dataset(&d2)?;

    // Adjust priors.
    let p1 = adjust_prior(opts.p1, df1.snp.len());
    let p2 = adjust_prior(opts.p2, df2.snp.len());

    // Merge on SNP.
    let pairs = merge_on_snp(&df1, &df2);
    if pairs.is_empty() {
        return Err(ColocError::NoCommonSnps);
    }

    let p12 = adjust_prior(opts.p12, pairs.len());

    let merged_snp: Vec<String> = pairs.iter().map(|&(i, _)| df1.snp[i].clone()).collect();
    let l1: Vec<f64> = pairs.iter().map(|&(i, _)| df1.l_abf[i]).collect();
    let l2: Vec<f64> = pairs.iter().map(|&(_, j)| df2.l_abf[j]).collect();

    // Filter prior weights to merged SNPs.
    // NOTE: R's coloc.abf() does `prior_weights1[which(df1$snp %in% merged.df$snp)]`
    // which preserves df1's ORIGINAL order, NOT merge order. Since combine_abf_weighted
    // receives the merged lABF values (in merge order), the weights end up aligned by
    // position, not by SNP identity. We reproduce this R behavior exactly.
    let merged_snp_set: std::collections::HashSet<&str> = df1
        .snp
        .iter()
        .filter(|s| df2.snp.iter().any(|s2| s2 == *s))
        .map(|s| s.as_str())
        .collect();
    let pw1: Option<Vec<f64>> = opts.prior_weights1.as_ref().map(|w| {
        df1.snp
            .iter()
            .enumerate()
            .filter(|(_, s)| merged_snp_set.contains(s.as_str()))
            .map(|(i, _)| w[i])
            .collect()
    });
    let pw2: Option<Vec<f64>> = opts.prior_weights2.as_ref().map(|w| {
        df2.snp
            .iter()
            .enumerate()
            .filter(|(_, s)| merged_snp_set.contains(s.as_str()))
            .map(|(i, _)| w[i])
            .collect()
    });

    // SNP.PP.H4.
    let lsum: Vec<f64> = l1.iter().zip(l2.iter()).map(|(&a, &b)| a + b).collect();
    let denom = logsum(&lsum);
    let snp_pp_h4: Vec<f64> = lsum.iter().map(|&v| (v - denom).exp()).collect();

    // Posterior probabilities.
    let pp = if pw1.is_some() || pw2.is_some() {
        combine_abf_weighted(&l1, &l2, p1, p2, p12, pw1.as_deref(), pw2.as_deref())
    } else {
        combine_abf(&l1, &l2, p1, p2, p12)
    };

    // Merged positions (if present in both).
    let position = match (&df1.position, &df2.position) {
        (Some(pos1), Some(_)) => Some(pairs.iter().map(|&(i, _)| pos1[i]).collect()),
        _ => None,
    };

    Ok(ColocAbfResult {
        nsnps: pairs.len(),
        pp,
        snp_pp_h4,
        snp: merged_snp,
        p1,
        p2,
        p12,
        l_abf_df1: l1,
        l_abf_df2: l2,
        position,
    })
}

// =====================================================================
// finemap.abf — single-trait fine-mapping
// =====================================================================

/// Per-SNP output of [`finemap_abf`].
#[derive(Clone, Debug, Default)]
pub struct FinemapAbfSnp {
    pub snp: String,
    pub v: f64,
    pub z: f64,
    pub r: f64,
    pub l_abf: f64,
    pub prior: f64,
    pub snp_pp: f64,
}

/// Output of [`finemap_abf`].
#[derive(Clone, Debug)]
pub struct FinemapAbfResult {
    pub snps: Vec<FinemapAbfSnp>,
    /// The null entry prior (snp = "null").
    pub null_prior: f64,
    /// Posterior probability of the null hypothesis (no causal variant).
    pub null_snp_pp: f64,
}

/// Fine-map a single dataset.
///
/// Direct port of R's `finemap.abf()`.
pub fn finemap_abf(
    dataset: &Dataset,
    p1: f64,
    prior_weights: Option<&[f64]>,
) -> Result<FinemapAbfResult> {
    check_dataset(dataset, "")?;

    let df = process_dataset(dataset)?;
    let nsnps = df.snp.len();
    let p1 = adjust_prior(p1, nsnps);

    // Compute prior vector.
    let prior_vec: Vec<f64> = match prior_weights {
        Some(w) => {
            let s: f64 = w.iter().sum();
            w.iter().map(|&wi| p1 * nsnps as f64 * wi / s).collect()
        }
        None => vec![p1; nsnps],
    };
    let null_prior = 1.0 - nsnps as f64 * p1;

    // SNP.PP = exp(lABF + log(prior) - logsum(lABF + log(prior)))
    // Note: R appends a null row with lABF=0 and prior=1-nsnps*p1, and the
    // denominator includes it.
    let log_prior: Vec<f64> = prior_vec.iter().map(|&p| p.ln()).collect();
    let mut all_entries: Vec<f64> = df
        .l_abf
        .iter()
        .zip(log_prior.iter())
        .map(|(&l, &lp)| l + lp)
        .collect();
    // Include the null entry in the denominator.
    all_entries.push(0.0_f64 + null_prior.ln());
    let denom = logsum(&all_entries);

    let mut snps: Vec<FinemapAbfSnp> = df
        .snp
        .iter()
        .enumerate()
        .map(|(i, snp)| FinemapAbfSnp {
            snp: snp.clone(),
            v: df.v.as_ref().map(|v| v[i]).unwrap_or(0.0),
            z: df.z.as_ref().map(|z| z[i]).unwrap_or(0.0),
            r: df.r.as_ref().map(|r| r[i]).unwrap_or(0.0),
            l_abf: df.l_abf[i],
            prior: prior_vec[i],
            snp_pp: (df.l_abf[i] + log_prior[i] - denom).exp(),
        })
        .collect();

    // R appends a null row; we keep it separate.
    let _ = &mut snps; // suppress unused mut
    let null_snp_pp = (0.0 + null_prior.ln() - denom).exp();
    Ok(FinemapAbfResult {
        snps,
        null_prior,
        null_snp_pp,
    })
}

// =====================================================================
// coloc.detail — detailed output with H3 decomposition
// =====================================================================

/// Output of [`coloc_detail`].
#[derive(Clone, Debug)]
pub struct ColocDetailResult {
    pub nsnps: usize,
    pub pp: PosteriorProbs,
    pub snp: Vec<String>,
    /// lABF for trait 1 (named lbf1 / lABF.h1 in R).
    pub lbf1: Vec<f64>,
    /// lABF for trait 2 (named lbf2 / lABF.h2 in R).
    pub lbf2: Vec<f64>,
    /// lABF for shared signal (lbf4 = lbf1 + lbf2).
    pub lbf4: Vec<f64>,
    /// Per-SNP PP for H4.
    pub snp_pp_h4: Vec<f64>,
    /// H3 pairwise: for each (snp1, snp2) pair, lABF.h3.
    pub h3_pairs: Vec<H3Pair>,
    pub p1: f64,
    pub p2: f64,
    pub p12: f64,
}

/// One pairwise entry in the H3 decomposition.
#[derive(Clone, Debug)]
pub struct H3Pair {
    pub snp1: String,
    pub snp2: String,
    pub lbf3: f64,
}

/// Detailed colocalisation analysis.
///
/// Direct port of R's `coloc.detail()`. Unlike [`coloc_abf`], this function
/// computes the full H3 (two distinct causal variants) decomposition across
/// all SNP pairs.
pub fn coloc_detail(
    dataset1: &Dataset,
    dataset2: &Dataset,
    p1: f64,
    p2: f64,
    p12: f64,
) -> Result<ColocDetailResult> {
    check_dataset(dataset1, "")?;
    check_dataset(dataset2, "")?;

    let df1 = process_dataset(dataset1)?;
    let df2 = process_dataset(dataset2)?;

    // Merge — coloc.detail uses merge by "snp" (and "position" if present in both).
    let pairs = merge_on_snp(&df1, &df2);
    if pairs.is_empty() {
        return Err(ColocError::NoCommonSnps);
    }

    let merged_snp: Vec<String> = pairs.iter().map(|&(i, _)| df1.snp[i].clone()).collect();
    let lbf1: Vec<f64> = pairs.iter().map(|&(i, _)| df1.l_abf[i]).collect();
    let lbf2: Vec<f64> = pairs.iter().map(|&(_, j)| df2.l_abf[j]).collect();
    let lbf4: Vec<f64> = lbf1.iter().zip(lbf2.iter()).map(|(&a, &b)| a + b).collect();
    let denom = logsum(&lbf4);
    let snp_pp_h4: Vec<f64> = lbf4.iter().map(|&v| (v - denom).exp()).collect();

    // H3 pairwise: expand.grid(snp1, snp2).
    let n = merged_snp.len();
    let mut h3_pairs = Vec::with_capacity(n * n);
    for i in 0..n {
        for j in 0..n {
            // expand.grid iterates snp2 fastest (R's as.data.frame(expand.grid)).
            h3_pairs.push(H3Pair {
                snp1: merged_snp[j].clone(), // first arg varies slowest
                snp2: merged_snp[i].clone(),  // second arg varies fastest
                lbf3: lbf1[j] + lbf2[i],
            });
        }
    }
    // R expand.grid(snp1=df$snp, snp2=df$snp) → snp1 repeats fully for each
    // snp2, i.e. for each i in 0..n, for each j in 0..n: (snp1[j], snp2[i]).
    // That matches the loop above. But actually the R code uses:
    //   expand.grid(snp1=df$snp, snp2=df$snp)
    // which produces all (snp1[j], snp2[i]) for i outer, j inner.  Wait —
    // expand.grid varies the first argument fastest.  Let me re-check.
    //
    // R: expand.grid(snp1=1:3, snp2=1:3) →
    //   snp1 = 1,2,3,1,2,3,1,2,3
    //   snp2 = 1,1,1,2,2,2,3,3,3
    // So snp1 varies fastest. Let me fix the loop:
    h3_pairs.clear();
    for i in 0..n {
        // i indexes snp2
        for j in 0..n {
            // j indexes snp1
            h3_pairs.push(H3Pair {
                snp1: merged_snp[j].clone(),
                snp2: merged_snp[i].clone(),
                lbf3: lbf1[j] + lbf2[i],
            });
        }
    }

    // Posterior probs using coloc.process formula (same as combine.abf but
    // using lbf names).
    let pp = {
        let l_h0 = 0.0;
        let l_h1 = p1.ln() + logsum(&lbf1);
        let l_h2 = p2.ln() + logsum(&lbf2);
        let lbf3_all: Vec<f64> = h3_pairs.iter().map(|h| h.lbf3).collect();
        let l_h3 = p1.ln() + p2.ln() + logsum(&lbf3_all);
        let l_h4 = p12.ln() + logsum(&lbf4);
        let all = [l_h0, l_h1, l_h2, l_h3, l_h4];
        let denom = logsum(&all);
        let pp_vec: Vec<f64> = all.iter().map(|&v| (v - denom).exp()).collect();
        PosteriorProbs {
            pp_h0: pp_vec[0],
            pp_h1: pp_vec[1],
            pp_h2: pp_vec[2],
            pp_h3: pp_vec[3],
            pp_h4: pp_vec[4],
        }
    };

    Ok(ColocDetailResult {
        nsnps: pairs.len(),
        pp,
        snp: merged_snp,
        lbf1,
        lbf2,
        lbf4,
        snp_pp_h4,
        h3_pairs,
        p1,
        p2,
        p12,
    })
}

// =====================================================================
// Credible sets
// =====================================================================

/// Extract a credible set from fine-mapping results.
///
/// Direct port of R's `credible.sets()`. Returns SNPs sorted by decreasing
/// posterior probability until cumulative probability ≥ `credible_size`.
pub fn credible_sets(snps: &[FinemapAbfSnp], credible_size: f64) -> Vec<(String, f64)> {
    let mut sorted: Vec<&FinemapAbfSnp> = snps.iter().collect();
    sorted.sort_by(|a, b| b.snp_pp.partial_cmp(&a.snp_pp).unwrap_or(std::cmp::Ordering::Equal));
    let mut cumsum = 0.0;
    let mut out = Vec::new();
    for s in &sorted {
        cumsum += s.snp_pp;
        out.push((s.snp.clone(), s.snp_pp));
        if cumsum >= credible_size {
            break;
        }
    }
    out
}

// =====================================================================
// check_dataset
// =====================================================================

/// Check a coloc dataset for errors.
///
/// Direct port of R's `check_dataset()`.
pub fn check_dataset(d: &Dataset, suffix: &str) -> Result<()> {
    let suffix = suffix.to_string();

    // type must be quant or cc (enforced by the enum).

    // SNP uniqueness.
    let mut seen = std::collections::HashSet::new();
    for s in &d.snp {
        if !seen.insert(s.as_str()) {
            return Err(ColocError::DuplicateSnps(suffix));
        }
    }

    // MAF validity.
    if let Some(maf) = &d.maf {
        for &f in maf {
            if !f.is_finite() || f <= 0.0 || f >= 1.0 {
                return Err(ColocError::BadMaf(suffix));
            }
        }
    }

    // Length consistency.
    let n = d.snp.len();
    for (name, vec) in [
        ("beta", &d.beta),
        ("varbeta", &d.varbeta),
        ("pvalues", &d.pvalues),
        ("maf", &d.maf),
        ("position", &d.position),
    ] {
        if let Some(v) = vec {
            if v.len() != n {
                return Err(ColocError::LengthMismatch {
                    suffix,
                    element: name.into(),
                });
            }
        }
    }

    // Check for missing values (NaN/Inf) in numeric vectors.
    for v in [&d.beta, &d.varbeta, &d.pvalues, &d.maf].into_iter().flatten() {
        for &x in v {
            if x.is_nan() {
                return Err(ColocError::MissingValues {
                    suffix,
                    element: "numeric".into(),
                });
            }
        }
    }

    // Check for infinite beta/varbeta.
    if let Some(beta) = &d.beta {
        if beta.iter().any(|&b| b.is_infinite()) {
            return Err(ColocError::InfiniteValues(suffix));
        }
    }
    if let Some(varbeta) = &d.varbeta {
        if varbeta.iter().any(|&v| v.is_infinite()) {
            return Err(ColocError::InfiniteValues(suffix));
        }
        if varbeta.contains(&0.0) {
            return Err(ColocError::ZeroVarbeta(suffix));
        }
    }

    // s must be between 0 and 1 for cc.
    if d.r#type == TraitType::CC {
        if let Some(s) = d.s {
            if s <= 0.0 || s >= 1.0 {
                return Err(ColocError::BadS(suffix));
            }
        }
    }

    // Either beta+varbeta or pvalues+maf.
    let has_bv = d.beta.is_some() && d.varbeta.is_some();
    let has_pval = d.pvalues.is_some() && d.maf.is_some();

    if !has_bv && !has_pval {
        return Err(ColocError::InsufficientData);
    }

    if !has_bv {
        // need pvalues + MAF
        if d.pvalues.is_none() || d.maf.is_none() {
            return Err(ColocError::InsufficientData);
        }
        // check p-values > 0
        if let Some(p) = &d.pvalues {
            if p.iter().any(|&p| p <= 0.0) {
                return Err(ColocError::NonPositivePvalue(suffix));
            }
        }
        if d.r#type == TraitType::CC && d.s.is_none() {
            return Err(ColocError::MissingSForCC(suffix));
        }
        if d.n.is_none() || d.n.unwrap_or(0.0) <= 0.0 {
            return Err(ColocError::BadN(suffix));
        }
    }

    // For quant without sdY, need MAF + N.
    if d.r#type == TraitType::Quant && d.sd_y.is_none()
        && (d.maf.is_none() || d.n.is_none()) {
            return Err(ColocError::MissingSdYOrMafN(suffix));
        }

    Ok(())
}

// =====================================================================
// Normal quantile (inverse CDF) — replaces R's qnorm
// =====================================================================

/// Inverse of the upper-tail normal CDF: `qnorm(p, lower.tail=FALSE)`.
///
/// Returns `z` such that `P(Z > z) = p` for standard normal Z.
/// Uses the Acklam algorithm (same accuracy as R's qnorm to ~1e-9).
fn normal_inv_cdf_upper(p: f64) -> f64 {
    // qnorm(p, lower.tail=FALSE) = -qnorm(p, lower.tail=TRUE) = qnorm(1-p, lower.tail=TRUE)
    normal_inv_cdf(1.0 - p)
}

/// Standard normal inverse CDF (lower tail) using the Acklam algorithm.
///
/// Relative error < 1.15e-9 across the full range.
fn normal_inv_cdf(p: f64) -> f64 {
    // Coefficients from Peter Acklam's algorithm.
    const A: [f64; 6] = [
        -3.969683028665376e+01,
        2.209460984245205e+02,
        -2.759285104469687e+02,
        1.383_577_518_672_69e2,
        -3.066479806614716e+01,
        2.506628277459239e+00,
    ];
    const B: [f64; 5] = [
        -5.447609879822406e+01,
        1.615858368580409e+02,
        -1.556989798598866e+02,
        6.680131188771972e+01,
        -1.328068155288572e+01,
    ];
    const C: [f64; 6] = [
        -7.784894002430293e-03,
        -3.223964580411365e-01,
        -2.400758277161838e+00,
        -2.549732539343734e+00,
        4.374664141464968e+00,
        2.938163982698783e+00,
    ];
    const D: [f64; 4] = [
        7.784695709041462e-03,
        3.224671290700398e-01,
        2.445134137142996e+00,
        3.754408661907416e+00,
    ];

    const P_LOW: f64 = 0.02425;
    const P_HIGH: f64 = 1.0 - P_LOW;

    if p < P_LOW {
        // Rational approximation for lower region.
        let q = (-2.0 * p.ln()).sqrt();
        let num = ((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5];
        let den = (((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0;
        return num / den;
    }

    if p <= P_HIGH {
        // Rational approximation for central region.
        let q = p - 0.5;
        let r = q * q;
        let num = (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q;
        let den = ((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0;
        return num / den;
    }

    // Upper region.
    let q = (-2.0 * (1.0 - p).ln()).sqrt();
    let num = ((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5];
    let den = (((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0;
    -num / den
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_logsum() {
        // logsum of [0, 0] = ln(2)
        let x = vec![0.0, 0.0];
        let result = logsum(&x);
        assert!((result - (2.0_f64).ln()).abs() < 1e-15);

        // logsum of [ln(1), ln(2), ln(3)] = ln(6)
        let x = vec![1.0_f64.ln(), (2.0_f64).ln(), (3.0_f64).ln()];
        let result = logsum(&x);
        assert!((result - (6.0_f64).ln()).abs() < 1e-14);
    }

    #[test]
    fn test_logdiff() {
        // logdiff(ln(3), ln(1)) = ln(3-1) = ln(2)
        let result = logdiff((3.0_f64).ln(), (1.0_f64).ln());
        assert!((result - (2.0_f64).ln()).abs() < 1e-15);
    }

    #[test]
    fn test_var_data() {
        // Var.data(0.3, 1000) = 1 / (2*1000*0.3*0.7)
        let v = var_data(0.3, 1000.0);
        let expected = 1.0 / (2.0 * 1000.0 * 0.3 * 0.7);
        assert!((v - expected).abs() < 1e-15);
    }

    #[test]
    fn test_var_data_cc() {
        // Var.data.cc(0.3, 1000, 0.5) = 1 / (2*1000*0.3*0.7*0.5*0.5)
        let v = var_data_cc(0.3, 1000.0, 0.5);
        let expected = 1.0 / (2.0 * 1000.0 * 0.3 * 0.7 * 0.5 * 0.5);
        assert!((v - expected).abs() < 1e-15);
    }

    #[test]
    fn test_normal_inv_cdf() {
        // qnorm(0.975) ≈ 1.959964
        let z = normal_inv_cdf(0.975);
        assert!((z - 1.959963985).abs() < 1e-6);
    }

    #[test]
    fn test_combine_abf_sums_to_one() {
        let l1 = vec![0.5, 1.0, -0.3, 2.0];
        let l2 = vec![0.2, -0.5, 1.0, 0.8];
        let pp = combine_abf(&l1, &l2, 1e-4, 1e-4, 1e-5);
        let sum = pp.pp_h0 + pp.pp_h1 + pp.pp_h2 + pp.pp_h3 + pp.pp_h4;
        assert!((sum - 1.0).abs() < 1e-14);
    }

    #[test]
    fn test_weighted_equals_unweighted() {
        // When weights are uniform, weighted == unweighted.
        let l1 = vec![0.5, 1.0, -0.3, 2.0];
        let l2 = vec![0.2, -0.5, 1.0, 0.8];
        let pp_u = combine_abf(&l1, &l2, 1e-4, 1e-4, 1e-5);
        let pp_w = combine_abf_weighted(&l1, &l2, 1e-4, 1e-4, 1e-5, None, None);
        assert!((pp_u.pp_h0 - pp_w.pp_h0).abs() < 1e-14);
        assert!((pp_u.pp_h1 - pp_w.pp_h1).abs() < 1e-14);
        assert!((pp_u.pp_h2 - pp_w.pp_h2).abs() < 1e-14);
        assert!((pp_u.pp_h3 - pp_w.pp_h3).abs() < 1e-14);
        assert!((pp_u.pp_h4 - pp_w.pp_h4).abs() < 1e-14);
    }

    #[test]
    fn test_adjust_prior() {
        assert!((adjust_prior(1e-4, 50) - 1e-4).abs() < 1e-15);
        // nsnps * p >= 1 → 1/(nsnps+1)
        assert!((adjust_prior(0.5, 10) - 1.0 / 11.0).abs() < 1e-15);
    }

    #[test]
    fn test_coloc_abf_basic() {
        // Simple quant-quant coloc with beta/varbeta.
        let d1 = Dataset {
            snp: vec!["s1".into(), "s2".into(), "s3".into()],
            beta: Some(vec![0.5, 0.1, 0.3]),
            varbeta: Some(vec![0.01, 0.01, 0.01]),
            pvalues: None,
            maf: Some(vec![0.3, 0.2, 0.4]),
            n: Some(1000.0),
            r#type: TraitType::Quant,
            s: None,
            sd_y: Some(1.0),
            position: None,
        };
        let d2 = Dataset {
            snp: vec!["s1".into(), "s2".into(), "s3".into()],
            beta: Some(vec![0.4, 0.05, 0.25]),
            varbeta: Some(vec![0.01, 0.01, 0.01]),
            pvalues: None,
            maf: Some(vec![0.3, 0.2, 0.4]),
            n: Some(1000.0),
            r#type: TraitType::Quant,
            s: None,
            sd_y: Some(1.0),
            position: None,
        };
        let result = coloc_abf(&d1, &d2, &ColocAbfOptions::default()).unwrap();
        let sum = result.pp.pp_h0 + result.pp.pp_h1 + result.pp.pp_h2 + result.pp.pp_h3
            + result.pp.pp_h4;
        assert!((sum - 1.0).abs() < 1e-14, "PPs must sum to 1, got {sum}");
        assert_eq!(result.nsnps, 3);
        // Similar effect directions → expect H4 > H3.
        assert!(result.pp.pp_h4 > result.pp.pp_h3);
    }

    #[test]
    fn test_finemap_abf() {
        let d = Dataset {
            snp: vec!["s1".into(), "s2".into()],
            beta: Some(vec![1.0, 0.1]),
            varbeta: Some(vec![0.01, 0.01]),
            pvalues: None,
            maf: Some(vec![0.3, 0.2]),
            n: Some(500.0),
            r#type: TraitType::Quant,
            s: None,
            sd_y: Some(1.0),
            position: None,
        };
        let result = finemap_abf(&d, 1e-4, None).unwrap();
        // SNP with larger |beta| should have higher PP.
        assert!(result.snps[0].snp_pp > result.snps[1].snp_pp);
        let sum: f64 = result.snps.iter().map(|s| s.snp_pp).sum::<f64>() + result.null_snp_pp;
        assert!((sum - 1.0).abs() < 1e-14);
    }
}
