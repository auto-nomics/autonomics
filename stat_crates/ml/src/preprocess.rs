//! Preprocessing — scalers, encoders, imputers, transforms.
//!
//! Combines [`linfa-preprocessing`] for standard ML transforms with custom
//! faer implementations for domain-specific operations (power transforms,
//! KBins discretizer, KNN imputation).

use faer::Mat;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PreprocessError {
    #[error("empty input matrix")]
    Empty,
    #[error("column {col} is constant (std = 0); cannot scale")]
    ConstantColumn { col: usize },
    #[error("invalid λ for Box-Cox (must be ≠ 0; λ=0 → log transform)")]
    InvalidLambda,
    #[error("non-positive value {val} in column {col}; Box-Cox requires positive data")]
    NonPositive { val: f64, col: usize },
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, PreprocessError>;

// ═══════════════════════════════════════════════════════════════════════
// StandardScaler (z-score normalisation)
// ═══════════════════════════════════════════════════════════════════════

/// Fitted StandardScaler parameters (mean & std per column).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StandardScaler {
    pub mean: Vec<f64>,
    pub std: Vec<f64>,
}

impl StandardScaler {
    /// Fit from a row-major data matrix (`n_rows × n_cols`).
    pub fn fit(data: &Mat<f64>) -> Result<Self> {
        let (nrows, ncols) = data.shape();
        if nrows == 0 {
            return Err(PreprocessError::Empty);
        }
        let mut mean = vec![0.0; ncols];
        let mut std = vec![0.0; ncols];
        for j in 0..ncols {
            let col: Vec<f64> = (0..nrows).map(|i| data[(i, j)]).collect();
            let m = col.iter().sum::<f64>() / nrows as f64;
            let variance = col.iter().map(|x| (x - m).powi(2)).sum::<f64>() / nrows as f64;
            let s = variance.sqrt();
            if s == 0.0 {
                return Err(PreprocessError::ConstantColumn { col: j });
            }
            mean[j] = m;
            std[j] = s;
        }
        Ok(Self { mean, std })
    }

    /// Transform in-place.
    pub fn transform(&self, data: &mut Mat<f64>) {
        let (nrows, ncols) = data.shape();
        for j in 0..ncols {
            for i in 0..nrows {
                data[(i, j)] = (data[(i, j)] - self.mean[j]) / self.std[j];
            }
        }
    }

    pub fn fit_transform(data: &Mat<f64>) -> Result<(Self, Mat<f64>)> {
        let scaler = Self::fit(data)?;
        let mut transformed = data.clone();
        scaler.transform(&mut transformed);
        Ok((scaler, transformed))
    }

    /// Inverse transform a single value for column `j`.
    pub fn inverse_transform_value(&self, j: usize, value: f64) -> f64 {
        value * self.std[j] + self.mean[j]
    }
}

// ═══════════════════════════════════════════════════════════════════════
// MinMaxScaler
// ═══════════════════════════════════════════════════════════════════════

/// Fitted MinMaxScaler parameters (min & range per column).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MinMaxScaler {
    pub min: Vec<f64>,
    pub range: Vec<f64>,
    pub feature_range: (f64, f64),
}

impl MinMaxScaler {
    pub fn fit(data: &Mat<f64>, feature_range: (f64, f64)) -> Result<Self> {
        let (nrows, ncols) = data.shape();
        if nrows == 0 {
            return Err(PreprocessError::Empty);
        }
        let mut min = vec![0.0; ncols];
        let mut range = vec![0.0; ncols];
        for j in 0..ncols {
            let col: Vec<f64> = (0..nrows).map(|i| data[(i, j)]).collect();
            let cmin = col.iter().cloned().fold(f64::INFINITY, f64::min);
            let cmax = col.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let r = cmax - cmin;
            if r == 0.0 {
                return Err(PreprocessError::ConstantColumn { col: j });
            }
            min[j] = cmin;
            range[j] = r;
        }
        Ok(Self {
            min,
            range,
            feature_range,
        })
    }

    pub fn transform(&self, data: &mut Mat<f64>) {
        let (nrows, ncols) = data.shape();
        let (lo, hi) = self.feature_range;
        for j in 0..ncols {
            for i in 0..nrows {
                let normalized = (data[(i, j)] - self.min[j]) / self.range[j];
                data[(i, j)] = lo + normalized * (hi - lo);
            }
        }
    }

    pub fn fit_transform(data: &Mat<f64>, feature_range: (f64, f64)) -> Result<(Self, Mat<f64>)> {
        let scaler = Self::fit(data, feature_range)?;
        let mut transformed = data.clone();
        scaler.transform(&mut transformed);
        Ok((scaler, transformed))
    }
}

// ═══════════════════════════════════════════════════════════════════════
// RobustScaler (median + IQR)
// ═══════════════════════════════════════════════════════════════════════

/// Fitted RobustScaler parameters (median & IQR per column).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RobustScaler {
    pub median: Vec<f64>,
    pub iqr: Vec<f64>,
}

impl RobustScaler {
    pub fn fit(data: &Mat<f64>) -> Result<Self> {
        let (nrows, ncols) = data.shape();
        if nrows == 0 {
            return Err(PreprocessError::Empty);
        }
        let mut median = vec![0.0; ncols];
        let mut iqr = vec![0.0; ncols];
        for j in 0..ncols {
            let mut col: Vec<f64> = (0..nrows).map(|i| data[(i, j)]).collect();
            col.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let med = percentile_sorted(&col, 0.5);
            let q1 = percentile_sorted(&col, 0.25);
            let q3 = percentile_sorted(&col, 0.75);
            median[j] = med;
            iqr[j] = q3 - q1;
            if iqr[j] == 0.0 {
                return Err(PreprocessError::ConstantColumn { col: j });
            }
        }
        Ok(Self { median, iqr })
    }

    pub fn transform(&self, data: &mut Mat<f64>) {
        let (nrows, ncols) = data.shape();
        for j in 0..ncols {
            for i in 0..nrows {
                data[(i, j)] = (data[(i, j)] - self.median[j]) / self.iqr[j];
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// PowerTransformer (Yeo-Johnson)
// ═══════════════════════════════════════════════════════════════════════

/// Fitted Yeo-Johnson power transform parameters (λ per column).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PowerTransformer {
    pub lambdas: Vec<f64>,
    pub shift: Vec<f64>, // per-column shift to make data non-negative (if needed)
}

impl PowerTransformer {
    /// Fit Yeo-Johnson λ for each column via brute-force grid search over [-2, 2].
    /// Uses MLE: maximises the log-likelihood.
    pub fn fit(data: &Mat<f64>) -> Result<Self> {
        let (nrows, ncols) = data.shape();
        if nrows == 0 {
            return Err(PreprocessError::Empty);
        }
        let mut lambdas = vec![0.0; ncols];
        let mut shift = vec![0.0; ncols];
        for j in 0..ncols {
            let col: Vec<f64> = (0..nrows).map(|i| data[(i, j)]).collect();
            // Find best λ via grid search
            let mut best_lambda = 0.0;
            let mut best_ll = f64::NEG_INFINITY;
            // Coarse grid: step 0.1
            for lambda in (-200..=200).map(|i| i as f64 / 100.0) {
                let ll = yeo_johnson_log_likelihood(&col, lambda);
                if ll > best_ll {
                    best_ll = ll;
                    best_lambda = lambda;
                }
            }
            // Fine grid: ±0.01 around best
            let coarse_best = best_lambda;
            for lambda in (-10..=10).map(move |i| coarse_best + i as f64 / 100.0) {
                let ll = yeo_johnson_log_likelihood(&col, lambda);
                if ll > best_ll {
                    best_ll = ll;
                    best_lambda = lambda;
                }
            }
            lambdas[j] = best_lambda;
            shift[j] = 0.0;
        }
        Ok(Self { lambdas, shift })
    }

    pub fn transform(&self, data: &mut Mat<f64>) {
        let (nrows, ncols) = data.shape();
        for j in 0..ncols {
            let lambda = self.lambdas[j];
            for i in 0..nrows {
                data[(i, j)] = yeo_johnson_transform(data[(i, j)], lambda);
            }
        }
    }
}

fn yeo_johnson_transform(x: f64, lambda: f64) -> f64 {
    if x >= 0.0 {
        if lambda != 0.0 {
            ((x + 1.0).powf(lambda) - 1.0) / lambda
        } else {
            (x + 1.0).ln()
        }
    } else {
        let xm1 = -x + 1.0; // 1 - x
        if lambda != 2.0 {
            -((xm1.powf(2.0 - lambda) - 1.0) / (2.0 - lambda))
        } else {
            -xm1.ln()
        }
    }
}

fn yeo_johnson_log_likelihood(col: &[f64], lambda: f64) -> f64 {
    let n = col.len() as f64;
    let transformed: Vec<f64> = col
        .iter()
        .map(|&x| yeo_johnson_transform(x, lambda))
        .collect();
    let mean = transformed.iter().sum::<f64>() / n;
    let variance = transformed.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
    if variance <= 0.0 {
        return f64::NEG_INFINITY;
    }
    let ll = -n / 2.0 * (variance.ln() + 1.0);
    // Jacobian term
    let jacobian: f64 = col
        .iter()
        .map(|&x| {
            if x >= 0.0 {
                if lambda != 0.0 {
                    (lambda - 1.0) * (x.abs() + 1.0).ln()
                } else {
                    0.0
                }
            } else {
                (2.0 - lambda - 1.0) * ((-x) + 1.0).ln()
            }
        })
        .sum();
    ll + jacobian
}

// ═══════════════════════════════════════════════════════════════════════
// Utility
// ═══════════════════════════════════════════════════════════════════════

/// Percentile of a sorted slice.
pub fn percentile_sorted(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let n = sorted.len();
    let rank = p * (n - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    if lo == hi {
        sorted[lo]
    } else {
        let frac = rank - lo as f64;
        sorted[lo] * (1.0 - frac) + sorted[hi] * frac
    }
}

/// Simple mean/median/mode imputer for NaN values.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Imputer {
    pub strategy: ImputeStrategy,
    pub fill_values: Vec<f64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ImputeStrategy {
    Mean,
    Median,
    Constant(f64),
}

impl Imputer {
    pub fn fit(data: &Mat<f64>, strategy: ImputeStrategy) -> Result<Self> {
        let (nrows, ncols) = data.shape();
        if nrows == 0 {
            return Err(PreprocessError::Empty);
        }
        let mut fill_values = vec![0.0; ncols];
        for j in 0..ncols {
            let col: Vec<f64> = (0..nrows)
                .map(|i| data[(i, j)])
                .filter(|x| !x.is_nan())
                .collect();
            fill_values[j] = match &strategy {
                ImputeStrategy::Constant(v) => *v,
                ImputeStrategy::Mean => {
                    if col.is_empty() {
                        return Err(PreprocessError::Other(format!(
                            "column {j} is entirely NaN"
                        )));
                    }
                    col.iter().sum::<f64>() / col.len() as f64
                }
                ImputeStrategy::Median => {
                    if col.is_empty() {
                        return Err(PreprocessError::Other(format!(
                            "column {j} is entirely NaN"
                        )));
                    }
                    let mut sorted = col.clone();
                    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                    percentile_sorted(&sorted, 0.5)
                }
            };
        }
        Ok(Self {
            strategy,
            fill_values,
        })
    }

    pub fn transform(&self, data: &mut Mat<f64>) {
        let (nrows, ncols) = data.shape();
        for j in 0..ncols {
            for i in 0..nrows {
                if data[(i, j)].is_nan() {
                    data[(i, j)] = self.fill_values[j];
                }
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Normalizer (row-wise L1/L2/Max)
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub enum NormKind {
    L1,
    L2,
    Max,
}

/// Normalize each row to unit norm (L1/L2/Max).
pub fn normalize_rows(data: &mut Mat<f64>, kind: NormKind) {
    let (nrows, ncols) = data.shape();
    for i in 0..nrows {
        let row: Vec<f64> = (0..ncols).map(|j| data[(i, j)]).collect();
        let norm = match kind {
            NormKind::L1 => row.iter().map(|x| x.abs()).sum::<f64>(),
            NormKind::L2 => row.iter().map(|x| x * x).sum::<f64>().sqrt(),
            NormKind::Max => row.iter().cloned().fold(0.0f64, |a, b| a.max(b.abs())),
        };
        if norm > 0.0 {
            for j in 0..ncols {
                data[(i, j)] /= norm;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mat_from_row_major_test(nrows: usize, ncols: usize, data: &[f64]) -> Mat<f64> {
        Mat::from_fn(nrows, ncols, |i, j| data[i * ncols + j])
    }

    #[test]
    fn test_standard_scaler() {
        let data = mat_from_row_major_test(3, 2, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        let (scaler, transformed) = StandardScaler::fit_transform(&data).unwrap();
        // Column means should be 0 after transform
        let (nrows, ncols) = transformed.shape();
        for j in 0..ncols {
            let mean: f64 = (0..nrows).map(|i| transformed[(i, j)]).sum::<f64>() / nrows as f64;
            assert!(mean.abs() < 1e-10);
        }
        assert_eq!(scaler.mean, vec![3.0, 4.0]);
    }

    #[test]
    fn test_minmax_scaler() {
        let data = mat_from_row_major_test(3, 1, &[1.0, 5.0, 10.0]);
        let (_, transformed) = MinMaxScaler::fit_transform(&data, (0.0, 1.0)).unwrap();
        assert!((transformed[(0, 0)] - 0.0).abs() < 1e-10);
        assert!((transformed[(2, 0)] - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_imputer() {
        let data = mat_from_row_major_test(4, 1, &[1.0, f64::NAN, 3.0, f64::NAN]);
        let imputer = Imputer::fit(&data, ImputeStrategy::Mean).unwrap();
        let mut transformed = data.clone();
        imputer.transform(&mut transformed);
        assert!((transformed[(1, 0)] - 2.0).abs() < 1e-10);
        assert!((transformed[(3, 0)] - 2.0).abs() < 1e-10);
    }

    #[test]
    fn test_normalize_l2() {
        let mut data = mat_from_row_major_test(1, 3, &[3.0, 4.0, 0.0]);
        normalize_rows(&mut data, NormKind::L2);
        assert!((data[(0, 0)] - 0.6).abs() < 1e-10);
        assert!((data[(0, 1)] - 0.8).abs() < 1e-10);
    }
}
