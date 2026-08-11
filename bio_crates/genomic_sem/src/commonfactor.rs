//! Common factor model — port of `R/commonfactor.R`.
//!
//! Fits a single-factor model to the LDSC-derived genetic covariance matrix.

use faer::Mat;

use crate::error::{GenomicSemError, Result};
use crate::sem::{self, EstimationMethod, SemConfig, parse_model};
use crate::utils::{Covstruc, compute_v_stand, build_w_from_v_stand, smooth_if_needed};
use crate::usermodel::{ModelFit, ParamResult, UserModelResult};

/// Configuration for `commonfactor`.
#[derive(Clone, Debug)]
pub struct CommonFactorConfig {
    pub estimation: EstimationMethod,
}

impl Default for CommonFactorConfig {
    fn default() -> Self {
        Self { estimation: EstimationMethod::DWLS }
    }
}

/// Run a common factor model with k indicators.
///
/// Generates the lavaan syntax for a one-factor model, then delegates to
/// `usermodel`.
pub fn commonfactor(
    covstruc: &Covstruc,
    config: &CommonFactorConfig,
) -> Result<UserModelResult> {
    let k = covstruc.s.nrows();
    if k <= 2 {
        return Err(GenomicSemError::InvalidInput(
            "Common factor model requires at least 3 traits (df > 0)".to_string(),
        ));
    }

    // Build model syntax: F1 =~ NA*V1 + V2 + ... + Vk \n F1 ~~ 1*F1
    let names: Vec<String> = (0..k).map(|i| format!("V{}", i + 1)).collect();
    let model_str = format!(
        "F1 =~ NA*{} + {}\nF1 ~~ 1*F1",
        names[0],
        names[1..].join(" + ")
    );

    let user_config = crate::usermodel::UserModelConfig {
        estimation: config.estimation,
        model: model_str,
        cfi_calc: true,
        std_lv: false,
        imp_cov: false,
        fix_resid: true,
        toler: f64::EPSILON,
        q_factor: false,
    };

    crate::usermodel::usermodel(covstruc, &user_config)
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
    fn test_commonfactor_basic() {
        let covstruc = make_test_covstruc(4);
        let config = CommonFactorConfig::default();
        let result = commonfactor(&covstruc, &config).unwrap();
        assert!(result.modelfit.df > 0);
        // Should have factor loadings
        assert!(result.results.iter().any(|r| r.op == "=~"));
    }
}
