//! FUSION TWAS file reader — port of `R/read_fusion.R`.
//!
//! Reads FUSION weight files and computes effect sizes and SEs for
//! transcriptome-wide association analysis (TWAS).

/// FUSION TWAS data for one trait.
#[derive(Clone, Debug)]
pub struct FusionData {
    pub panel: Vec<String>,
    pub gene: Vec<String>,
    pub hsq: Vec<f64>,
    pub beta: Vec<f64>,
    pub se: Vec<f64>,
}

/// Configuration for `read_fusion`.
#[derive(Clone, Debug, Default)]
pub struct FusionConfig {
    pub trait_names: Vec<String>,
    pub binary: Vec<bool>,
    pub n: Vec<f64>,
    pub perm: bool,
}

/// Compute effect and SE from FUSION TWAS Z-score and HSQ.
///
/// For binary traits:
///   effect = Z / sqrt(N/4 * HSQ)
///   SE = 1 / sqrt(N/4 * HSQ)
///
/// For continuous traits:
///   effect = Z / sqrt(N * HSQ)
///   SE = |effect / Z|
pub fn compute_effect_se(z: f64, hsq: f64, n: f64, is_binary: bool) -> (f64, f64) {
    if is_binary {
        let denom = ((n / 4.0) * hsq).max(1e-10);
        let effect = z / denom.sqrt();
        let se = 1.0 / denom.sqrt();
        (effect, se)
    } else {
        let denom = (n * hsq).max(1e-10);
        let effect = z / denom.sqrt();
        let se = (effect / z).abs();
        (effect, se)
    }
}

/// Convert to liability scale for binary traits.
///
/// `effect_liab = effect / sqrt(effect² * HSQ + π²/3)`
pub fn to_liability(effect: f64, hsq: f64) -> f64 {
    let denom = (effect * effect * hsq + std::f64::consts::PI * std::f64::consts::PI / 3.0).sqrt();
    if denom > 0.0 { effect / denom } else { effect }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_effect_se_binary() {
        let (effect, se) = compute_effect_se(2.0, 0.1, 100_000.0, true);
        let expected_denom: f64 = (100_000.0_f64 / 4.0 * 0.1).sqrt();
        assert!((effect - 2.0 / expected_denom).abs() < 1e-10);
        assert!((se - 1.0 / expected_denom).abs() < 1e-10);
    }

    #[test]
    fn test_compute_effect_se_continuous() {
        let (effect, se) = compute_effect_se(3.0, 0.05, 50_000.0, false);
        let expected_denom: f64 = (50_000.0_f64 * 0.05).sqrt();
        assert!((effect - 3.0 / expected_denom).abs() < 1e-10);
        assert!((se - (effect / 3.0).abs()).abs() < 1e-10);
    }

    #[test]
    fn test_to_liability() {
        let val = to_liability(0.5, 0.1);
        // Should scale the effect
        assert!(val.abs() < 0.5);
    }
}
