//! Model-implied genetic correlation matrix — port of `R/rgmodel.R`.
//!
//! Estimates a saturated genetic correlation matrix using SEM, providing
//! the model-implied R matrix and its sampling covariance V_R.

use faer::Mat;

use crate::error::Result;
use crate::linalg;
use crate::utils::Covstruc;
use crate::usermodel::{ModelFit, ParamResult, UserModelResult};

/// Result from `rgmodel`.
#[derive(Clone, Debug)]
pub struct RgModelResult {
    /// Observed genetic covariance matrix.
    pub s: Mat<f64>,
    /// Sampling covariance matrix.
    pub v: Mat<f64>,
    /// Genetic correlation matrix R.
    pub r: Mat<f64>,
    /// Sampling covariance of R.
    pub v_r: Mat<f64>,
    /// Model results from the saturated model fit.
    pub model_results: Option<UserModelResult>,
}

/// Run the rgmodel analysis.
///
/// Computes the genetic correlation matrix R and its sampling covariance V_R
/// from the LDSC output. Optionally fits a saturated SEM model.
pub fn rgmodel(covstruc: &Covstruc, fit_model: bool) -> Result<RgModelResult> {
    let s = covstruc.s.clone();
    let v = covstruc.v.clone();
    let k = s.nrows();

    // Compute genetic correlation matrix R = cov2cor(S)
    let r = linalg::cov2cor(&s);

    // Compute sampling covariance of R via delta method:
    // V_R = D⁻¹ · V · D⁻¹ where D = diag(sqrt(diag(S)))
    let s_diag_sqrt: Vec<f64> = (0..k).map(|i| s[(i, i)].max(0.0).sqrt()).collect();
    let z = k * (k + 1) / 2;
    let mut scale_o = vec![0.0f64; z];
    let mut idx = 0;
    for j in 0..k {
        for i in j..k {
            // For correlation: r_ij = s_ij / (sd_i * sd_j)
            // d r_ij / d s_ij = 1 / (sd_i * sd_j)
            let denom = s_diag_sqrt[i] * s_diag_sqrt[j];
            scale_o[idx] = if denom > 0.0 { 1.0 / denom } else { 0.0 };
            idx += 1;
        }
    }

    let mut v_r = Mat::zeros(z, z);
    for i in 0..z {
        for j in 0..z {
            v_r[(i, j)] = v[(i, j)] * scale_o[i] * scale_o[j];
        }
    }

    // Optionally fit saturated model
    let model_results = if fit_model {
        // Saturated model: all variances and covariances freely estimated
        let names: Vec<String> = (0..k).map(|i| format!("V{}", i + 1)).collect();
        let mut model_str = String::new();
        for i in 0..k {
            for j in i..k {
                if i == j {
                    model_str.push_str(&format!("{} ~~ {}\n", names[i], names[j]));
                } else {
                    model_str.push_str(&format!("{} ~~ {}\n", names[i], names[j]));
                }
            }
        }
        let config = crate::usermodel::UserModelConfig {
            model: model_str,
            ..Default::default()
        };
        crate::usermodel::usermodel(covstruc, &config).ok()
    } else {
        None
    };

    Ok(RgModelResult {
        s,
        v,
        r,
        v_r,
        model_results,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_covstruc(k: usize) -> Covstruc {
        let mut s = Mat::zeros(k, k);
        for i in 0..k {
            s[(i, i)] = 0.3 + 0.1 * i as f64;
            for j in (i + 1)..k {
                let val = 0.15;
                s[(i, j)] = val;
                s[(j, i)] = val;
            }
        }
        let z = k * (k + 1) / 2;
        Covstruc {
            v: Mat::<f64>::identity(z, z) * 0.001,
            s,
            i_mat: Mat::identity(k, k),
            n: Mat::zeros(1, z),
            m: 100_000.0,
            v_stand: None,
            s_stand: None,
        }
    }

    #[test]
    fn test_rgmodel_basic() {
        let covstruc = make_test_covstruc(3);
        let result = rgmodel(&covstruc, false).unwrap();
        // R should be a correlation matrix (diagonal = 1)
        for i in 0..3 {
            assert!((result.r[(i, i)] - 1.0).abs() < 1e-10);
        }
        // V_R should have positive diagonal
        for i in 0..6 {
            assert!(result.v_r[(i, i)] >= 0.0);
        }
    }
}
