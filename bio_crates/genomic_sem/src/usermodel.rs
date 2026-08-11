//! User-specified SEM — port of `R/usermodel.R`.
//!
//! Fits a user-specified structural equation model to the LDSC-derived
//! genetic covariance matrix using DWLS or ML estimation with
//! sandwich-corrected standard errors.

use faer::Mat;

use crate::error::Result;
use crate::linalg;
use crate::near_pd;
use crate::sem::{self, EstimationMethod, SemConfig, SemModel, SemResult, parse_model};
use crate::utils::{Covstruc, build_w_from_v_stand, compute_v_stand, smooth_if_needed};

/// Configuration for `usermodel`.
#[derive(Clone, Debug)]
pub struct UserModelConfig {
    pub estimation: EstimationMethod,
    pub model: String,
    pub cfi_calc: bool,
    pub std_lv: bool,
    pub imp_cov: bool,
    pub fix_resid: bool,
    pub toler: f64,
    pub q_factor: bool,
}

impl Default for UserModelConfig {
    fn default() -> Self {
        Self {
            estimation: EstimationMethod::DWLS,
            model: String::new(),
            cfi_calc: true,
            std_lv: false,
            imp_cov: false,
            fix_resid: true,
            toler: f64::EPSILON,
            q_factor: false,
        }
    }
}

/// Results from `usermodel`.
#[derive(Clone, Debug)]
pub struct UserModelResult {
    pub modelfit: ModelFit,
    pub results: Vec<ParamResult>,
}

/// Model fit statistics.
#[derive(Clone, Debug, Default)]
pub struct ModelFit {
    pub chisq: f64,
    pub df: i32,
    pub p_chisq: f64,
    pub aic: f64,
    pub cfi: Option<f64>,
    pub srmr: f64,
}

/// One parameter estimate row.
#[derive(Clone, Debug)]
pub struct ParamResult {
    pub lhs: String,
    pub op: String,
    pub rhs: String,
    pub unstand_est: f64,
    pub unstand_se: f64,
    pub std_genotype: f64,
    pub std_genotype_se: f64,
    pub std_all: f64,
    pub p_value: f64,
}

/// Run the user-specified model.
///
/// This is the main entry point that mirrors GenomicSEM's `usermodel()`.
pub fn usermodel(covstruc: &Covstruc, config: &UserModelConfig) -> Result<UserModelResult> {
    let mut v_ld = covstruc.v.clone();
    let mut s_ld = covstruc.s.clone();
    let k = s_ld.nrows();
    let z = k * (k + 1) / 2;

    // Parse model
    let model = parse_model(&config.model)?;

    // Determine which traits are used in the model
    let s_names: Vec<String> = (0..k).map(|i| format!("V{}", i + 1)).collect();
    let used_traits = find_used_traits(&model, &s_names);

    // Subset S and V to only used traits
    let (s_sub, v_sub, trait_names_sub) = if used_traits.len() < k {
        subset_covstruc(&s_ld, &v_ld, &used_traits)?
    } else {
        (s_ld.clone(), v_ld.clone(), s_names.clone())
    };

    let k_sub = s_sub.nrows();
    let z_sub = k_sub * (k_sub + 1) / 2;

    // Smooth S and V if needed
    let (s_smooth, s_smoothed, s_diff) = smooth_if_needed(&s_sub);
    let (v_smooth, v_smoothed, v_diff) = smooth_if_needed(&v_sub);
    s_ld = s_smooth;
    v_ld = v_smooth;

    // Build weight matrix: W = (diag(V_LD))⁻¹
    let w = {
        let mut wm = Mat::zeros(z_sub, z_sub);
        for i in 0..z_sub {
            let mut val = v_ld[(i, i)];
            if val.abs() < 2e-9 {
                val = 2e-9;
            }
            wm[(i, i)] = 1.0 / val;
        }
        wm
    };

    // Fit unstandardized model
    let sem_config = SemConfig {
        estimation: config.estimation,
        std_lv: config.std_lv,
        fix_resid: config.fix_resid,
        toler: config.toler,
        max_iter: 500,
    };

    let sem_result = sem::fit_sem(&s_ld, &w, &model, &trait_names_sub, &sem_config)?;

    // Compute proper chi-square using V eigenstructure
    let chisq = if v_smoothed {
        sem::compute_chisq(&s_ld, &sem_result.implied, &v_ld)
    } else {
        sem::compute_chisq(&s_ld, &sem_result.implied, &v_ld)
    };

    let df = sem_result.df;
    let npar = sem_result.npar;

    // CFI (simplified — uses independence model approximation)
    let cfi = if config.cfi_calc && df > 0 {
        let null_chisq = compute_independence_chisq(&s_ld, &v_ld);
        let null_df = (k_sub * (k_sub + 1) / 2 - k_sub) as i32;
        Some(sem::compute_cfi(chisq, df, null_chisq, null_df))
    } else {
        None
    };

    // Compute standardized results
    let s_stand = crate::utils::standardize(&s_ld);
    let v_stand = compute_v_stand(&s_ld, &v_ld);
    let w_stand = build_w_from_v_stand(&v_stand);

    let stand_result = sem::fit_sem(&s_stand, &w_stand, &model, &trait_names_sub, &sem_config).ok();

    // Build parameter results
    let results = build_param_results(&sem_result, stand_result.as_ref());

    // Model fit
    let p_chisq = if df > 0 {
        crate::stats::pchisq_sf(chisq, df as f64)
    } else {
        f64::NAN
    };

    let modelfit = ModelFit {
        chisq,
        df,
        p_chisq,
        aic: chisq + 2.0 * npar as f64,
        cfi,
        srmr: sem_result.srmr,
    };

    Ok(UserModelResult { modelfit, results })
}

/// Find which trait indices are used in the model.
fn find_used_traits(model: &SemModel, all_names: &[String]) -> Vec<usize> {
    let mut used = Vec::new();
    for name in all_names {
        let pattern = format!("\\b{}\\b", name);
        let used_in_model = model.lines.iter().any(|line| {
            line.lhs.contains(name) || line.rhs.contains(name)
        });
        if used_in_model {
            if let Some(idx) = all_names.iter().position(|n| n == name) {
                used.push(idx);
            }
        }
        let _ = pattern; // suppress unused
    }
    if used.is_empty() {
        (0..all_names.len()).collect()
    } else {
        used
    }
}

/// Subset S and V matrices to only used traits.
fn subset_covstruc(
    s: &Mat<f64>,
    v: &Mat<f64>,
    indices: &[usize],
) -> Result<(Mat<f64>, Mat<f64>, Vec<String>)> {
    let k = indices.len();
    let z = k * (k + 1) / 2;

    // Subset S
    let mut s_sub = Mat::zeros(k, k);
    for (i, &ri) in indices.iter().enumerate() {
        for (j, &rj) in indices.iter().enumerate() {
            s_sub[(i, j)] = s[(ri, rj)];
        }
    }

    // Build vech index mapping
    let mut idx_map = vec![0usize; z];
    let mut counter = 0usize;
    for (j, &cj) in indices.iter().enumerate() {
        for (i, &ci) in indices.iter().enumerate() {
            if i >= j {
                let orig_idx = if ci >= cj {
                    ci * (ci + 1) / 2 + cj
                } else {
                    cj * (cj + 1) / 2 + ci
                };
                // Actually, vech is column-major lower triangle:
                // index for (row, col) where row >= col is: col * (2*n - col - 1) / 2 + row
                // But our n is the original k, not the subset k
                let n_orig = s.nrows();
                let orig_vech_idx = cj * (2 * n_orig - cj - 1) / 2 + ci;
                idx_map[counter] = orig_vech_idx;
                counter += 1;
            }
        }
    }

    // Subset V
    let mut v_sub = Mat::zeros(z, z);
    for i in 0..z {
        for j in 0..z {
            v_sub[(i, j)] = v[(idx_map[i], idx_map[j])];
        }
    }

    let names: Vec<String> = indices.iter().map(|&i| format!("V{}", i + 1)).collect();
    Ok((s_sub, v_sub, names))
}

/// Compute chi-square for the independence (null) model.
fn compute_independence_chisq(s: &Mat<f64>, v: &Mat<f64>) -> f64 {
    // Independence model: only variances, no covariances
    let n = s.nrows();
    let mut implied = Mat::zeros(n, n);
    for i in 0..n {
        implied[(i, i)] = s[(i, i)];
    }
    sem::compute_chisq(s, &implied, v)
}

/// Build parameter results from unstandardized and standardized fits.
fn build_param_results(
    unstand: &SemResult,
    stand: Option<&SemResult>,
) -> Vec<ParamResult> {
    unstand.params
        .iter()
        .enumerate()
        .filter(|(_, p)| p.free > 0 || p.op == ":=")
        .map(|(i, p)| {
            let se = unstand.se.get(i).copied().unwrap_or(f64::NAN);
            let (std_est, std_se) = if let Some(sr) = stand {
                let s = sr.params.get(i);
                (s.map(|x| x.est).unwrap_or(f64::NAN), sr.se.get(i).copied().unwrap_or(f64::NAN))
            } else {
                (f64::NAN, f64::NAN)
            };

            let p_value = if se.is_finite() && se > 0.0 {
                let z = p.est / se;
                crate::stats::pnorm_two_sided(z)
            } else {
                f64::NAN
            };

            ParamResult {
                lhs: p.lhs.clone(),
                op: p.op.clone(),
                rhs: p.rhs.clone(),
                unstand_est: p.est,
                unstand_se: se,
                std_genotype: std_est,
                std_genotype_se: std_se,
                std_all: std_est,
                p_value,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_covstruc(k: usize) -> Covstruc {
        let mut s = Mat::zeros(k, k);
        for i in 0..k {
            s[(i, i)] = 0.3 + 0.1 * i as f64;
        }
        for i in 0..k {
            for j in (i + 1)..k {
                let val = 0.15 + 0.05 * (i + j) as f64;
                s[(i, j)] = val;
                s[(j, i)] = val;
            }
        }

        let z = k * (k + 1) / 2;
        let v = Mat::<f64>::identity(z, z) * 0.001;
        let i_mat = Mat::identity(k, k);

        Covstruc {
            v,
            s,
            i_mat,
            n: Mat::zeros(1, z),
            m: 100_000.0,
            v_stand: None,
            s_stand: None,
        }
    }

    #[test]
    fn test_usermodel_simple_cfa() {
        let covstruc = make_test_covstruc(3);
        let config = UserModelConfig {
            model: "F1 =~ NA*V1 + V2 + V3\nF1 ~~ 1*F1".to_string(),
            ..Default::default()
        };
        let result = usermodel(&covstruc, &config).unwrap();
        assert!(result.modelfit.df >= 0);
        // Should have at least 3 loading + 3 residual variance params
        assert!(result.results.len() >= 5);
    }

    #[test]
    fn test_usermodel_two_factor() {
        let covstruc = make_test_covstruc(4);
        let config = UserModelConfig {
            model: "F1 =~ NA*V1 + V2\nF2 =~ NA*V3 + V4\nF1 ~~ F2".to_string(),
            ..Default::default()
        };
        let result = usermodel(&covstruc, &config).unwrap();
        assert!(result.results.iter().any(|r| r.op == "=~"));
    }
}
