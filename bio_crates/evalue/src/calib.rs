//! Calibrated estimates for meta-analysis.
//!
//! Port of `MetaUtility::calib_ests` (Wang & Lee 2019), used by the
//! calibrated method in `confounded_meta()`.

/// Compute calibrated estimates from study-level point estimates and SEs.
///
/// Port of `MetaUtility::calib_ests`. Follows Wang & Lee (2019):
/// `calib_i = yi + sei_i^2 * (mean(1/sei) - 1/sei_i) / 2`
pub fn calib_ests(yi: &[f64], sei: &[f64]) -> Vec<f64> {
    let n = yi.len();
    assert_eq!(n, sei.len());

    let mean_inv_sei: f64 = sei.iter().map(|s| 1.0 / s).sum::<f64>() / n as f64;

    yi.iter()
        .enumerate()
        .map(|(i, &y)| y + sei[i] * sei[i] * (mean_inv_sei - 1.0 / sei[i]) / 2.0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calib_basic() {
        let yi = vec![0.1, 0.2, 0.3, 0.4, 0.5];
        let sei = vec![0.05, 0.1, 0.15, 0.1, 0.05];
        let calib = calib_ests(&yi, &sei);
        assert_eq!(calib.len(), 5);
        for v in &calib {
            assert!(v.is_finite());
        }
    }

    #[test]
    fn test_calib_equal_se() {
        // With equal SE, calibration is identity
        let yi = vec![1.0, 2.0, 3.0];
        let sei = vec![0.5, 0.5, 0.5];
        let calib = calib_ests(&yi, &sei);
        for i in 0..3 {
            assert!((calib[i] - yi[i]).abs() < 1e-10, "calib[{i}] = {}, expected {}", calib[i], yi[i]);
        }
    }
}
