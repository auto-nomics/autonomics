//! Unweighted ordinal logistic regression (proportional-odds model).
//!
//! The cumulative-logit model is
//! `P(Y ≤ j | X) = sigmoid(α_j − Xβ)` for `j = 0, .., K−2`. Parameters are
//! estimated jointly with Newton-Raphson using expected Fisher information.

use faer::linalg::solvers::{DenseSolveCore, Llt, Solve};
use faer::{Mat, Side};
use statrs::distribution::{ContinuousCDF, Normal};

use crate::error::{Result, StatError};

const MAX_ITER: usize = 100;
const TOL: f64 = 1.0e-7;
const PROB_EPS: f64 = 1.0e-300;
const Z_975: f64 = 1.959963984540054;

/// Result of an unweighted proportional-odds logistic regression.
#[derive(Debug, Clone)]
pub struct OrdinalLogisticResult {
    /// Predictor slope coefficients.
    pub coefficients: Vec<f64>,
    /// Ordered thresholds/cutpoints, with `levels.len() - 1` entries.
    pub thresholds: Vec<f64>,
    /// Standard errors for slopes followed by thresholds.
    pub std_errors: Vec<f64>,
    /// Wald z-statistics for slopes followed by thresholds.
    pub z_stats: Vec<f64>,
    /// Two-sided Wald p-values for slopes followed by thresholds.
    pub p_values: Vec<f64>,
    /// `exp(coefficient)` for slopes and `exp(threshold)` for cutpoints.
    pub odds_ratios: Vec<f64>,
    /// Lower bounds of 95% confidence intervals on the odds-ratio scale.
    pub odds_ratio_ci_lower: Vec<f64>,
    /// Upper bounds of 95% confidence intervals on the odds-ratio scale.
    pub odds_ratio_ci_upper: Vec<f64>,
    /// Maximized log-likelihood.
    pub log_likelihood: f64,
    /// Model-based covariance of slopes followed by thresholds.
    pub covariance: Vec<Vec<f64>>,
    /// Number of complete observations.
    pub n_obs: usize,
    /// Number of ordered outcome levels.
    pub n_levels: usize,
    /// Whether Newton-Raphson converged.
    pub converged: bool,
    /// Number of Newton-Raphson iterations performed.
    pub n_iter: usize,
}

impl OrdinalLogisticResult {
    /// Concatenated slopes followed by thresholds.
    pub fn all_coefficients(&self) -> Vec<f64> {
        self.coefficients
            .iter()
            .chain(self.thresholds.iter())
            .copied()
            .collect()
    }
}

/// Fit an unweighted proportional-odds ordinal logistic model.
///
/// `y_ord` contains zero-based integer codes in the range `0..K`. `predictors`
/// are parallel columns, each with the same length as `y_ord`. The model has
/// no separate intercept: its ordered thresholds play that role.
pub fn ordinal_logistic(predictors: &[&[f64]], y_ord: &[usize]) -> Result<OrdinalLogisticResult> {
    let n = y_ord.len();
    let p = predictors.len();
    if n == 0 {
        return Err(StatError::EmptyInput);
    }
    if p == 0 {
        return Err(StatError::InvalidInput(
            "ordinal logistic regression requires at least one predictor".to_string(),
        ));
    }
    for predictor in predictors {
        if predictor.len() != n {
            return Err(StatError::LengthMismatch {
                a: n,
                b: predictor.len(),
            });
        }
        if predictor.iter().any(|value| !value.is_finite()) {
            return Err(StatError::Numerical(
                "ordinal logistic predictors must be finite".to_string(),
            ));
        }
    }

    let n_levels = y_ord.iter().copied().max().unwrap_or_default() + 1;
    if n_levels < 2 {
        return Err(StatError::InvalidInput(
            "ordinal logistic outcome must contain at least two levels".to_string(),
        ));
    }
    if y_ord.iter().any(|&level| level >= n_levels) {
        return Err(StatError::InvalidInput(
            "ordinal logistic outcome codes must be contiguous zero-based integers".to_string(),
        ));
    }
    let n_thresholds = n_levels - 1;
    let n_params = p + n_thresholds;
    if n < n_params {
        return Err(StatError::InsufficientData {
            min: n_params,
            actual: n,
        });
    }

    let mut level_counts = vec![0_usize; n_levels];
    for &level in y_ord {
        level_counts[level] += 1;
    }
    if level_counts.contains(&0) {
        return Err(StatError::InvalidInput(
            "ordinal logistic outcome codes must cover every level from 0 to K-1".to_string(),
        ));
    }
    let mut parameters = vec![0.0; n_params];
    let mut cumulative_count = 0_usize;
    for threshold in 0..n_thresholds {
        cumulative_count += level_counts[threshold];
        let proportion = cumulative_count as f64 / n as f64;
        let clamped = proportion.clamp(1.0e-6, 1.0 - 1.0e-6);
        parameters[p + threshold] = (clamped / (1.0 - clamped)).ln();
    }

    let mut converged = false;
    let mut n_iter = 0_usize;
    for iteration in 0..MAX_ITER {
        n_iter = iteration + 1;
        let mut score = vec![0.0; n_params];
        let mut information = vec![vec![0.0; n_params]; n_params];

        for (observation, &level) in y_ord.iter().enumerate() {
            let linear_predictor: f64 = (0..p)
                .map(|predictor_index| {
                    parameters[predictor_index] * predictors[predictor_index][observation]
                })
                .sum();
            let cumulative: Vec<f64> = (0..n_thresholds)
                .map(|threshold| sigmoid(parameters[p + threshold] - linear_predictor))
                .collect();
            let outcome_probability = cell_probability(level, &cumulative, n_levels).max(PROB_EPS);
            let log_cell_probability = outcome_probability.ln();
            if !log_cell_probability.is_finite() {
                return Err(StatError::Numerical(
                    "ordinal logistic likelihood underflow".to_string(),
                ));
            }

            let predictor_row: Vec<f64> = predictors
                .iter()
                .map(|predictor| predictor[observation])
                .collect();
            let derivatives = cell_derivatives(&cumulative, &predictor_row, n_levels, p);
            for row in 0..n_params {
                score[row] += derivatives[level][row] / outcome_probability;
            }
            for possible_level in 0..n_levels {
                let probability =
                    cell_probability(possible_level, &cumulative, n_levels).max(PROB_EPS);
                for row in 0..n_params {
                    for column in 0..n_params {
                        information[row][column] += derivatives[possible_level][row]
                            * derivatives[possible_level][column]
                            / (probability * probability);
                    }
                }
            }
        }

        let information_matrix = Mat::from_fn(n_params, n_params, |row, column| {
            information[row][column] + 1.0e-8 * (row == column) as usize as f64
        });
        let score_matrix = Mat::from_fn(n_params, 1, |row, _| score[row]);
        let cholesky = Llt::new(information_matrix.as_ref(), Side::Lower)
            .ok()
            .ok_or(StatError::SingularMatrix)?;
        let delta = cholesky.solve(&score_matrix);
        let max_delta = (0..n_params)
            .map(|row| delta[(row, 0)].abs())
            .fold(f64::MIN, f64::max);
        if !max_delta.is_finite() {
            return Err(StatError::Numerical(
                "ordinal logistic Newton step diverged".to_string(),
            ));
        }
        for (parameter, update) in parameters
            .iter_mut()
            .zip((0..n_params).map(|row| delta[(row, 0)]))
        {
            *parameter += update;
        }
        if max_delta < TOL {
            converged = true;
            break;
        }
    }

    let coefficients = parameters[..p].to_vec();
    let thresholds = parameters[p..].to_vec();
    if thresholds.windows(2).any(|window| window[0] >= window[1]) {
        return Err(StatError::Numerical(
            "ordinal logistic fit produced non-increasing thresholds".to_string(),
        ));
    }

    let mut log_likelihood = 0.0;
    let mut information = vec![vec![0.0; n_params]; n_params];
    for (observation, &level) in y_ord.iter().enumerate() {
        let linear_predictor: f64 = (0..p)
            .map(|predictor_index| {
                coefficients[predictor_index] * predictors[predictor_index][observation]
            })
            .sum();
        let cumulative: Vec<f64> = (0..n_thresholds)
            .map(|threshold| sigmoid(thresholds[threshold] - linear_predictor))
            .collect();
        let outcome_probability = cell_probability(level, &cumulative, n_levels).max(PROB_EPS);
        log_likelihood += outcome_probability.ln();

        let predictor_row: Vec<f64> = predictors
            .iter()
            .map(|predictor| predictor[observation])
            .collect();
        let derivatives = cell_derivatives(&cumulative, &predictor_row, n_levels, p);
        for possible_level in 0..n_levels {
            let probability = cell_probability(possible_level, &cumulative, n_levels).max(PROB_EPS);
            for row in 0..n_params {
                for column in 0..n_params {
                    information[row][column] += derivatives[possible_level][row]
                        * derivatives[possible_level][column]
                        / (probability * probability);
                }
            }
        }
    }

    let information_matrix = Mat::from_fn(n_params, n_params, |row, column| {
        information[row][column] + 1.0e-8 * (row == column) as usize as f64
    });
    let cholesky = Llt::new(information_matrix.as_ref(), Side::Lower)
        .ok()
        .ok_or(StatError::SingularMatrix)?;
    let covariance_matrix = cholesky.inverse();
    let covariance = (0..n_params)
        .map(|row| {
            (0..n_params)
                .map(|column| covariance_matrix[(row, column)])
                .collect::<Vec<f64>>()
        })
        .collect::<Vec<_>>();

    let all_coefficients = parameters.clone();
    let std_errors = (0..n_params)
        .map(|index| covariance[index][index].max(0.0).sqrt())
        .collect::<Vec<_>>();
    let z_stats = (0..n_params)
        .map(|index| all_coefficients[index] / std_errors[index])
        .collect::<Vec<_>>();
    let normal = Normal::new(0.0, 1.0)
        .map_err(|error| StatError::Numerical(format!("standard normal: {error}")))?;
    let p_values = z_stats
        .iter()
        .map(|z| 2.0 * normal.sf(z.abs()))
        .collect::<Vec<_>>();
    let odds_ratios = all_coefficients.iter().map(|value| value.exp()).collect();
    let odds_ratio_ci_lower = (0..n_params)
        .map(|index| (all_coefficients[index] - Z_975 * std_errors[index]).exp())
        .collect();
    let odds_ratio_ci_upper = (0..n_params)
        .map(|index| (all_coefficients[index] + Z_975 * std_errors[index]).exp())
        .collect();

    Ok(OrdinalLogisticResult {
        coefficients,
        thresholds,
        std_errors,
        z_stats,
        p_values,
        odds_ratios,
        odds_ratio_ci_lower,
        odds_ratio_ci_upper,
        log_likelihood,
        covariance,
        n_obs: n,
        n_levels,
        converged,
        n_iter,
    })
}

fn cell_probability(level: usize, cumulative: &[f64], n_levels: usize) -> f64 {
    if level == 0 {
        cumulative[0]
    } else if level == n_levels - 1 {
        1.0 - cumulative[level - 1]
    } else {
        cumulative[level] - cumulative[level - 1]
    }
}

fn cell_derivatives(
    cumulative: &[f64],
    predictors: &[f64],
    n_levels: usize,
    n_predictors: usize,
) -> Vec<Vec<f64>> {
    let n_params = n_predictors + n_levels - 1;
    let mut derivatives = vec![vec![0.0; n_params]; n_levels];
    for level in 0..n_levels {
        if level < n_levels - 1 {
            let weight = cumulative[level] * (1.0 - cumulative[level]);
            derivatives[level][n_predictors + level] = weight;
            for predictor_index in 0..n_predictors {
                derivatives[level][predictor_index] -= weight * predictors[predictor_index];
            }
        }
        if level > 0 {
            let previous = level - 1;
            let weight = cumulative[previous] * (1.0 - cumulative[previous]);
            derivatives[level][n_predictors + previous] -= weight;
            for predictor_index in 0..n_predictors {
                derivatives[level][predictor_index] += weight * predictors[predictor_index];
            }
        }
    }
    derivatives
}

fn sigmoid(value: f64) -> f64 {
    if value >= 0.0 {
        1.0 / (1.0 + (-value).exp())
    } else {
        let exponential = value.exp();
        exponential / (1.0 + exponential)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_positive_slope_and_ordered_thresholds() {
        let x = vec![-3.0, -2.0, -1.0, 0.0, 1.0, 2.0, 3.0];
        let y = vec![1, 0, 2, 0, 2, 1, 2];
        let fit = ordinal_logistic(&[&x], &y).unwrap();

        assert!(fit.converged);
        assert_eq!(fit.n_levels, 3);
        assert!(fit.coefficients[0] > 0.25);
        assert!(
            fit.thresholds
                .windows(2)
                .all(|window| window[0] < window[1])
        );
        assert!(fit.log_likelihood.is_finite());
        assert!(fit.std_errors.iter().all(|value| value.is_finite()));
        assert!(fit.p_values.iter().all(|value| value.is_finite()));
    }

    #[test]
    fn rejects_one_outcome_level() {
        let x = vec![1.0, 2.0];
        assert!(ordinal_logistic(&[&x], &[0, 0]).is_err());
    }

    #[test]
    fn rejects_noncontiguous_outcome_codes() {
        let x = vec![1.0, 2.0, 3.0];
        assert!(ordinal_logistic(&[&x], &[0, 2, 2]).is_err());
    }
}
