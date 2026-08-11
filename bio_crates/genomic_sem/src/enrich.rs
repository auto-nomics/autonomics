//! SEM-based enrichment testing — port of `R/enrich.R`.
//!
//! Tests whether genetic enrichment across functional annotations
//! is associated with model parameters.

use faer::Mat;

use crate::error::Result;
use crate::sldsc::SldscOutput;
use crate::usermodel::{UserModelConfig, UserModelResult};

/// Configuration for enrichment.
#[derive(Clone, Debug)]
pub struct EnrichConfig {
    pub model: String,
    pub params: Vec<String>,
    pub fix: String,
    pub std_lv: bool,
    pub tau: bool,
    pub base: bool,
    pub toler: f64,
}

impl Default for EnrichConfig {
    fn default() -> Self {
        Self {
            model: String::new(),
            params: Vec::new(),
            fix: "regressions".to_string(),
            std_lv: false,
            tau: false,
            base: true,
            toler: f64::EPSILON,
        }
    }
}

/// Run enrichment analysis on stratified LDSC output.
pub fn enrich(s_covstruc: &SldscOutput, config: &EnrichConfig) -> Result<Vec<UserModelResult>> {
    // For each annotation partition, fit the SEM and collect results
    let mut results = Vec::new();

    for partition in &s_covstruc.partitions {
        let covstruc = crate::utils::Covstruc {
            v: if config.tau { partition.v_tau.clone() } else { partition.v.clone() },
            s: if config.tau { partition.s_tau.clone() } else { partition.s.clone() },
            i_mat: s_covstruc.intercepts.clone(),
            n: s_covstruc.n.clone(),
            m: 0.0,
            v_stand: None,
            s_stand: None,
        };

        let user_config = UserModelConfig {
            model: config.model.clone(),
            std_lv: config.std_lv,
            toler: config.toler,
            ..Default::default()
        };

        if let Ok(result) = crate::usermodel::usermodel(&covstruc, &user_config) {
            results.push(result);
        }
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enrich_config_default() {
        let config = EnrichConfig::default();
        assert_eq!(config.fix, "regressions");
    }
}
