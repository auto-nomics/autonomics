//! StandardScaler — compute and apply per-feature standardisation.
//!
//! Stores mean and std for each feature column so that the same transform
//! can be re-applied at inference time.  Serialised as part of `DLModelArtifact`.

use serde::{Deserialize, Serialize};

use crate::tensor::Tensor;

/// Per-feature mean/std computed from training data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StandardScaler {
    pub mean: Vec<f64>,
    pub std: Vec<f64>,
}

impl StandardScaler {
    /// Fit from a `(n_samples × n_features)` tensor.
    pub fn fit(data: &Tensor) -> Self {
        let (nrows, ncols) = data.shape();
        let mut mean = vec![0.0; ncols];
        let mut std = vec![0.0; ncols];

        for j in 0..ncols {
            let mut sum = 0.0;
            for i in 0..nrows {
                sum += data.at(i, j);
            }
            mean[j] = sum / nrows as f64;

            let mut var = 0.0;
            for i in 0..nrows {
                let d = data.at(i, j) - mean[j];
                var += d * d;
            }
            // Population std with epsilon to avoid division by zero.
            std[j] = (var / nrows as f64).sqrt().max(1e-8);
        }

        Self { mean, std }
    }

    /// Transform in-place: returns a new standardised tensor.
    pub fn transform(&self, data: &Tensor) -> Tensor {
        let (nrows, ncols) = data.shape();
        let mut out = Tensor::zeros(nrows, ncols);
        for j in 0..ncols {
            for i in 0..nrows {
                out.set(i, j, (data.at(i, j) - self.mean[j]) / self.std[j]);
            }
        }
        out
    }

    /// Fit + transform in one call.
    pub fn fit_transform(data: &Tensor) -> Self {
        let scaler = Self::fit(data);
        // Return scaler; caller calls transform separately.
        scaler
    }

    /// Number of features.
    pub fn n_features(&self) -> usize {
        self.mean.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scaler_fit_transform() {
        let data = Tensor::from_rows(
            4,
            2,
            &[1.0, 10.0, 2.0, 20.0, 3.0, 30.0, 4.0, 40.0],
        );
        let scaler = StandardScaler::fit(&data);
        let transformed = scaler.transform(&data);

        // Each column should have mean ≈ 0 and std ≈ 1.
        for j in 0..2 {
            let mut mean = 0.0;
            let mut var = 0.0;
            for i in 0..4 {
                mean += transformed.at(i, j);
            }
            mean /= 4.0;
            for i in 0..4 {
                let d = transformed.at(i, j) - mean;
                var += d * d;
            }
            var /= 4.0;
            assert!(mean.abs() < 1e-10, "col {j} mean = {mean}");
            assert!((var.sqrt() - 1.0).abs() < 1e-10, "col {j} std");
        }
    }

    #[test]
    fn test_scaler_constant_column() {
        let data = Tensor::from_rows(3, 1, &[5.0, 5.0, 5.0]);
        let scaler = StandardScaler::fit(&data);
        let transformed = scaler.transform(&data);
        // std clamped to 1e-8, so (5-5)/1e-8 = 0.
        for i in 0..3 {
            assert_eq!(transformed.at(i, 0), 0.0);
        }
    }

    #[test]
    fn test_scaler_serde_roundtrip() {
        let scaler = StandardScaler {
            mean: vec![1.0, 2.0],
            std: vec![0.5, 1.5],
        };
        let json = serde_json::to_string(&scaler).unwrap();
        let restored: StandardScaler = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.mean, vec![1.0, 2.0]);
        assert_eq!(restored.std, vec![0.5, 1.5]);
    }
}
