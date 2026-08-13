//! Per-SNP multivariate GWAS — port of `R/userGWAS.R` and `R/commonfactorGWAS.R`.
//!
//! Fits a SEM to each SNP individually, extending the genetic covariance
//! matrix S with a SNP row/column, to test the effect of each SNP on
//! latent factors.

use faer::Mat;
use rayon::prelude::*;

use crate::error::Result;
use crate::sem::{self, EstimationMethod, SemConfig, parse_model};
use crate::sumstats::MergedSumstats;
use crate::utils::{Covstruc, GcMode, get_s_full, get_v_full, get_v_snp, smooth_if_needed};

/// Configuration for `userGWAS`.
#[derive(Clone, Debug)]
pub struct UserGwasConfig {
    pub estimation: EstimationMethod,
    pub model: String,
    pub parallel: bool,
    pub gc: GcMode,
    pub snpse: Option<f64>,
    pub std_lv: bool,
    pub fix_measurement: bool,
    pub toler: f64,
    pub smooth_check: bool,
    pub q_snp: bool,
}

impl Default for UserGwasConfig {
    fn default() -> Self {
        Self {
            estimation: EstimationMethod::DWLS,
            model: String::new(),
            parallel: true,
            gc: GcMode::Standard,
            snpse: None,
            std_lv: false,
            fix_measurement: true,
            toler: f64::EPSILON,
            smooth_check: false,
            q_snp: false,
        }
    }
}

/// Results for one SNP.
#[derive(Clone, Debug)]
pub struct SnpResult {
    pub snp: String,
    pub chr: i32,
    pub bp: i64,
    pub maf: f64,
    pub a1: String,
    pub a2: String,
    pub params: Vec<(String, String, String, f64, f64)>, // (lhs, op, rhs, est, se)
    pub chisq: f64,
    pub df: i32,
}

/// Run per-SNP GWAS across all SNPs.
///
/// For each SNP, constructs the extended S and V matrices, fits the SEM,
/// and extracts the SNP effect on each factor.
pub fn user_gwas(
    covstruc: &Covstruc,
    sumstats: &MergedSumstats,
    config: &UserGwasConfig,
) -> Result<Vec<SnpResult>> {
    let k = covstruc.s.nrows();
    let n_snps = sumstats.n_snps();

    // Pre-compute fixed pieces
    let var_snp_se2 = config.snpse.map(|s| s * s).unwrap_or(0.0005_f64.powi(2));

    // I_LD with diagonal floored at 1
    let mut i_ld = covstruc.i_mat.clone();
    for i in 0..k {
        if i_ld[(i, i)] <= 1.0 {
            i_ld[(i, i)] = 1.0;
        }
    }

    // Coords for V_SNP
    let coords: Vec<(usize, usize)> = (0..k)
        .flat_map(|i| (0..k).map(move |j| (i, j)))
        .filter(|&(i, j)| i <= j)
        .collect();

    // Parse model once
    let model = parse_model(&config.model)?;

    // Trait names
    let trait_names: Vec<String> = (0..k).map(|i| format!("V{}", i + 1)).collect();

    let sem_config = SemConfig {
        estimation: config.estimation,
        std_lv: config.std_lv,
        fix_resid: false,
        toler: config.toler,
        max_iter: 200, // fewer iterations per SNP
    };

    let process_snp = |i: usize| -> Result<SnpResult> {
        let var_snp = 2.0 * sumstats.maf[i] * (1.0 - sumstats.maf[i]);

        let beta_row: Vec<f64> = (0..k).map(|t| sumstats.beta_col(t)[i]).collect();
        let se_row: Vec<f64> = (0..k).map(|t| sumstats.se_col(t)[i]).collect();

        // Build V_SNP
        let v_snp = get_v_snp(&se_row, &i_ld, var_snp, config.gc, &coords, k);

        // Build V_full
        let v_full = get_v_full(k, &covstruc.v, var_snp_se2, &v_snp);
        let (v_full_smooth, _, _) = smooth_if_needed(&v_full);

        // Build S_full
        let s_full = get_s_full(k, &covstruc.s, var_snp, &beta_row, "SNP", &trait_names);
        let (s_full_smooth, _, _) = smooth_if_needed(&s_full);

        // Build weight matrix from diagonal of V_full
        let dim = (k + 1) * (k + 2) / 2;
        let mut w = Mat::zeros(dim, dim);
        for j in 0..dim {
            let mut val = v_full_smooth[(j, j)];
            if val.abs() < 2e-9 {
                val = 2e-9;
            }
            w[(j, j)] = 1.0 / val;
        }

        // Extended variable names include SNP
        let mut obs_vars = vec!["SNP".to_string()];
        obs_vars.extend(trait_names.iter().cloned());

        let result = sem::fit_sem(&s_full_smooth, &w, &model, &obs_vars, &sem_config)?;

        // Extract SNP effects
        let params: Vec<(String, String, String, f64, f64)> = result
            .params
            .iter()
            .enumerate()
            .filter(|(_, p)| p.rhs == "SNP" || p.lhs == "SNP")
            .map(|(idx, p)| {
                let se = result.se.get(idx).copied().unwrap_or(f64::NAN);
                (p.lhs.clone(), p.op.clone(), p.rhs.clone(), p.est, se)
            })
            .collect();

        Ok(SnpResult {
            snp: sumstats.snp[i].clone(),
            chr: sumstats.chr[i],
            bp: sumstats.bp[i],
            maf: sumstats.maf[i],
            a1: sumstats.a1[i].clone(),
            a2: sumstats.a2[i].clone(),
            params,
            chisq: result.chisq,
            df: result.df,
        })
    };

    let indices: Vec<usize> = (0..n_snps).collect();
    if config.parallel {
        let results: Vec<SnpResult> = indices
            .par_iter()
            .map(|&i| {
                process_snp(i).unwrap_or(SnpResult {
                    snp: sumstats.snp[i].clone(),
                    chr: sumstats.chr[i],
                    bp: sumstats.bp[i],
                    maf: sumstats.maf[i],
                    a1: sumstats.a1[i].clone(),
                    a2: sumstats.a2[i].clone(),
                    params: Vec::new(),
                    chisq: f64::NAN,
                    df: -1,
                })
            })
            .collect();
        Ok(results)
    } else {
        indices.iter().map(|&i| process_snp(i)).collect()
    }
}

/// Configuration for `commonfactorGWAS`.
#[derive(Clone, Debug)]
pub struct CommonFactorGwasConfig {
    pub estimation: EstimationMethod,
    pub parallel: bool,
    pub gc: GcMode,
    pub toler: f64,
}

impl Default for CommonFactorGwasConfig {
    fn default() -> Self {
        Self {
            estimation: EstimationMethod::DWLS,
            parallel: true,
            gc: GcMode::Standard,
            toler: f64::EPSILON,
        }
    }
}

/// Run common-factor GWAS — a one-factor model with per-SNP effects.
pub fn commonfactor_gwas(
    covstruc: &Covstruc,
    sumstats: &MergedSumstats,
    config: &CommonFactorGwasConfig,
) -> Result<Vec<SnpResult>> {
    let k = covstruc.s.nrows();
    let names: Vec<String> = (0..k).map(|i| format!("V{}", i + 1)).collect();

    // Build common factor model with SNP effect
    let model_str = format!(
        "F1 =~ NA*{} + {}\nF1 ~~ 1*F1\nF1 ~ SNP",
        names[0],
        names[1..].join(" + ")
    );

    let user_config = UserGwasConfig {
        estimation: config.estimation,
        model: model_str,
        parallel: config.parallel,
        gc: config.gc,
        ..Default::default()
    };

    user_gwas(covstruc, sumstats, &user_config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_data(k: usize, n_snps: usize) -> (Covstruc, MergedSumstats) {
        let mut s = Mat::zeros(k, k);
        for i in 0..k {
            s[(i, i)] = 0.3;
            for j in (i + 1)..k {
                s[(i, j)] = 0.15;
                s[(j, i)] = 0.15;
            }
        }
        let z = k * (k + 1) / 2;
        let covstruc = Covstruc {
            v: Mat::<f64>::identity(z, z) * 0.001,
            s,
            i_mat: Mat::<f64>::identity(k, k),
            n: Mat::zeros(1, z),
            m: 100_000.0,
            v_stand: None,
            s_stand: None,
        };

        let sumstats = MergedSumstats {
            snp: (0..n_snps).map(|i| format!("rs{}", i + 1)).collect(),
            chr: vec![1; n_snps],
            bp: (0..n_snps).map(|i| ((i + 1) * 1000) as i64).collect(),
            maf: vec![0.3; n_snps],
            a1: vec!["A".into(); n_snps],
            a2: vec!["G".into(); n_snps],
            betas: (0..k).map(|_| vec![0.01; n_snps]).collect(),
            ses: (0..k).map(|_| vec![0.05; n_snps]).collect(),
            n_traits: k,
        };

        (covstruc, sumstats)
    }

    #[test]
    fn test_user_gwas_small() {
        let (covstruc, sumstats) = make_test_data(3, 5);
        let config = UserGwasConfig {
            model: "F1 =~ NA*V1 + V2 + V3\nF1 ~~ 1*F1\nF1 ~ SNP".to_string(),
            parallel: false,
            ..Default::default()
        };
        let results = user_gwas(&covstruc, &sumstats, &config).unwrap();
        assert_eq!(results.len(), 5);
        // Each SNP should produce some results
        assert!(results[0].params.len() >= 1 || results[0].chisq.is_nan());
    }

    #[test]
    fn test_commonfactor_gwas_small() {
        let (covstruc, sumstats) = make_test_data(3, 3);
        let config = CommonFactorGwasConfig {
            parallel: false,
            ..Default::default()
        };
        let results = commonfactor_gwas(&covstruc, &sumstats, &config).unwrap();
        assert_eq!(results.len(), 3);
    }
}
