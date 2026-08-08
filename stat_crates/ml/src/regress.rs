//! Regression algorithms — Ridge, Lasso, ElasticNet, LARS, PLS.
//!
//! Uses [`linfa-elasticnet`], [`linfa-linear`], [`linfa-lars`], [`linfa-pls`].

use faer::Mat;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RegressError {
    #[error("empty input")]
    Empty,
    #[error("target length ({target}) doesn't match data rows ({rows})")]
    TargetMismatch { target: usize, rows: usize },
    #[error("linfa error: {0}")]
    Linfa(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, RegressError>;

pub use crate::dimred::faer_to_ndarray;

pub struct RegressResult {
    pub predictions: Vec<f64>,
    pub coefficients: Vec<f64>,
    pub intercept: f64,
}

// ═══════════════════════════════════════════════════════════════════════
// Linear Regression (linfa-linear)
// ═══════════════════════════════════════════════════════════════════════

pub fn linear_regression(data: &Mat<f64>, target: &[f64]) -> Result<RegressResult> {
    use linfa::dataset::DatasetBase;
    use linfa::traits::{Fit, Predict};
    use linfa_linear::LinearRegression;

    let (nrows, _ncols) = data.shape();
    if nrows == 0 {
        return Err(RegressError::Empty);
    }
    if target.len() != nrows {
        return Err(RegressError::TargetMismatch { target: target.len(), rows: nrows });
    }

    let x = faer_to_ndarray(data);
    let y = ndarray::Array1::from_vec(target.to_vec());
    let dataset = DatasetBase::new(x, y);

    let model = LinearRegression::new()
        .fit(&dataset)
        .map_err(|e| RegressError::Linfa(e.to_string()))?;

    let predicted = model.predict(dataset.records());
    let predictions: Vec<f64> = predicted.iter().copied().collect();

    // Extract coefficients and intercept
    let params = model.params();
    let coefficients: Vec<f64> = params.iter().copied().collect();

    Ok(RegressResult {
        predictions,
        coefficients,
        intercept: 0.0, // linfa-linear centers internally
    })
}

// ═══════════════════════════════════════════════════════════════════════
// ElasticNet (linfa-elasticnet — covers Ridge/Lasso/ElasticNet)
// ═══════════════════════════════════════════════════════════════════════

pub fn elastic_net(
    data: &Mat<f64>,
    target: &[f64],
    penalty: f64,
    l1_ratio: f64,
    max_iter: usize,
    tol: f64,
) -> Result<RegressResult> {
    use linfa::dataset::DatasetBase;
    use linfa::traits::{Fit, Predict};
    use linfa_elasticnet::ElasticNet;

    let (nrows, _) = data.shape();
    if nrows == 0 {
        return Err(RegressError::Empty);
    }
    if target.len() != nrows {
        return Err(RegressError::TargetMismatch { target: target.len(), rows: nrows });
    }

    let x = faer_to_ndarray(data);
    let y = ndarray::Array1::from_vec(target.to_vec());
    let dataset = DatasetBase::new(x, y);

    let model = ElasticNet::params()
        .penalty(penalty)
        .l1_ratio(l1_ratio)
        .max_iterations(max_iter as u32)
        .tolerance(tol)
        .fit(&dataset)
        .map_err(|e| RegressError::Linfa(e.to_string()))?;

    let predicted = model.predict(dataset.records());
    let predictions: Vec<f64> = predicted.iter().copied().collect();

    // For Ridge (l1_ratio=0), Lasso (l1_ratio=1), ElasticNet (0<l1_ratio<1)
    Ok(RegressResult {
        predictions,
        coefficients: model.hyperplane().to_vec(),
        intercept: model.intercept(),
    })
}

// ═══════════════════════════════════════════════════════════════════════
// LARS (linfa-lars — Least Angle Regression)
// ═══════════════════════════════════════════════════════════════════════

pub fn lars(data: &Mat<f64>, target: &[f64], _n_features: usize) -> Result<RegressResult> {
    use linfa::dataset::DatasetBase;
    use linfa::traits::{Fit, Predict};
    use linfa_lars::Lars;

    let (nrows, _) = data.shape();
    if nrows == 0 {
        return Err(RegressError::Empty);
    }
    if target.len() != nrows {
        return Err(RegressError::TargetMismatch { target: target.len(), rows: nrows });
    }

    let x = faer_to_ndarray(data);
    let y = ndarray::Array1::from_vec(target.to_vec());
    let dataset = DatasetBase::new(x, y);

    let model = Lars::params()
        
        .fit(&dataset)
        .map_err(|e| RegressError::Linfa(e.to_string()))?;

    // LARS doesn't implement Predict directly; compute manually
    let coefs = model.hyperplane();
    let intercept = model.intercept();
    let predictions: Vec<f64> = (0..data.nrows())
        .map(|i| {
            let mut val = intercept;
            for j in 0..data.ncols() {
                val += coefs[j] * data[(i, j)];
            }
            val
        })
        .collect();

    Ok(RegressResult {
        predictions,
        coefficients: coefs.to_vec(),
        intercept,
    })
}

// ═══════════════════════════════════════════════════════════════════════
// PLS (linfa-pls — Partial Least Squares)
// ═══════════════════════════════════════════════════════════════════════

pub fn pls_regression(
    data: &Mat<f64>,
    target: &Mat<f64>,
    n_components: usize,
) -> Result<Vec<Vec<f64>>> {
    use linfa::dataset::DatasetBase;
    use linfa::traits::{Fit, Predict};
    use linfa_pls::PlsRegression;

    let (nrows, _) = data.shape();
    if nrows == 0 {
        return Err(RegressError::Empty);
    }

    let x = faer_to_ndarray(data);
    let y = faer_to_ndarray(target);
    let dataset = DatasetBase::new(x, y);

    let model = PlsRegression::params(n_components)
        .fit(&dataset)
        .map_err(|e| RegressError::Linfa(e.to_string()))?;

    // Predict
    let predicted = model.predict(dataset.records());
    let (p_rows, p_cols) = predicted.dim();
    let result: Vec<Vec<f64>> = (0..p_rows)
        .map(|i| (0..p_cols).map(|j| predicted[(i, j)]).collect())
        .collect();

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::mat_from_row_major;

    #[test]
    fn test_linear_regression() {
        let data = mat_from_row_major(
            5,
            2,
            &[1.0, 2.0, 2.0, 4.0, 3.0, 6.0, 4.0, 8.0, 5.0, 10.0],
        );
        let target = vec![5.0, 10.0, 15.0, 20.0, 25.0]; // y = 5*x1
        let result = linear_regression(&data, &target).unwrap();
        assert_eq!(result.predictions.len(), 5);
        // Predictions should be close to actual
        for (pred, &actual) in result.predictions.iter().zip(&target) {
            assert!((pred - actual).abs() < 1.0);
        }
    }

    #[test]
    fn test_elastic_net() {
        let data = mat_from_row_major(
            10,
            3,
            &[1.0, 0.0, 1.0, 2.0, 0.0, 1.0, 3.0, 0.0, 1.0, 4.0, 0.0, 1.0,
              5.0, 0.0, 1.0, 6.0, 0.0, 1.0, 7.0, 0.0, 1.0, 8.0, 0.0, 1.0,
              9.0, 0.0, 1.0, 10.0, 0.0, 1.0],
        );
        let target: Vec<f64> = (1..=10).map(|x| x as f64 * 2.0).collect();
        let result = elastic_net(&data, &target, 0.01, 0.5, 1000, 1e-4).unwrap();
        assert_eq!(result.predictions.len(), 10);
    }
}
