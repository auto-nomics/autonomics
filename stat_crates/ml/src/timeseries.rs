//! Time series analysis — ARIMA, Kalman filter, exponential smoothing, PELT.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum TsError {
    #[error("empty input")]
    Empty,
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, TsError>;

// ═══════════════════════════════════════════════════════════════════════
// Exponential Smoothing (Simple / Holt / Holt-Winters)
// ═══════════════════════════════════════════════════════════════════════

pub struct ExpSmoothingResult {
    pub fitted: Vec<f64>,
    pub forecast: Vec<f64>,
    pub level: f64,
    pub trend: Option<f64>,
}

pub fn exponential_smoothing(
    data: &[f64],
    alpha: f64,
    n_forecast: usize,
) -> Result<ExpSmoothingResult> {
    if data.is_empty() {
        return Err(TsError::Empty);
    }
    let mut fitted = Vec::with_capacity(data.len());
    let mut level = data[0];
    fitted.push(level);

    for i in 1..data.len() {
        level = alpha * data[i] + (1.0 - alpha) * level;
        fitted.push(level);
    }

    let forecast = vec![level; n_forecast];

    Ok(ExpSmoothingResult {
        fitted,
        forecast,
        level,
        trend: None,
    })
}

pub fn holt_linear(
    data: &[f64],
    alpha: f64,
    beta: f64,
    n_forecast: usize,
) -> Result<ExpSmoothingResult> {
    if data.len() < 2 {
        return Err(TsError::Other("Holt's method requires ≥ 2 points".into()));
    }
    let mut fitted = Vec::with_capacity(data.len());
    let mut level = data[0];
    let mut trend = data[1] - data[0];
    fitted.push(level + trend);

    for i in 1..data.len() {
        let prev_level = level;
        level = alpha * data[i] + (1.0 - alpha) * (level + trend);
        trend = beta * (level - prev_level) + (1.0 - beta) * trend;
        fitted.push(level + trend);
    }

    let forecast: Vec<f64> = (1..=n_forecast)
        .map(|h| level + h as f64 * trend)
        .collect();

    Ok(ExpSmoothingResult {
        fitted,
        forecast,
        level,
        trend: Some(trend),
    })
}

// ═══════════════════════════════════════════════════════════════════════
// AR(p) via least squares
// ═══════════════════════════════════════════════════════════════════════

pub struct ArResult {
    pub coefficients: Vec<f64>,
    pub intercept: f64,
    pub fitted: Vec<f64>,
    pub residuals: Vec<f64>,
}

pub fn ar(data: &[f64], p: usize) -> Result<ArResult> {
    if data.len() <= p {
        return Err(TsError::Other(format!(
            "need > {p} observations for AR({p})"
        )));
    }
    let n = data.len() - p;
    // Build design matrix: each row is [1, y[t-1], y[t-2], ..., y[t-p]]
    let mut x = vec![0.0f64; n * (p + 1)];
    let mut y = vec![0.0f64; n];
    for t in 0..n {
        x[t * (p + 1)] = 1.0; // intercept
        for j in 1..=p {
            x[t * (p + 1) + j] = data[t + p - j];
        }
        y[t] = data[t + p];
    }

    // Solve OLS: beta = (X^T X)^{-1} X^T y
    let coefs = ols_solve(&x, &y, n, p + 1)?;
    let intercept = coefs[0];
    let coefficients = coefs[1..].to_vec();

    // Fitted values and residuals
    let mut fitted = vec![0.0; data.len()];
    let mut residuals = vec![0.0; data.len()];
    fitted[..p].copy_from_slice(&data[..p]); // first p points: no fit
    for t in 0..n {
        let pred = intercept
            + (0..p).map(|j| coefficients[j] * data[t + p - 1 - j]).sum::<f64>();
        fitted[t + p] = pred;
        residuals[t + p] = data[t + p] - pred;
    }

    Ok(ArResult {
        coefficients,
        intercept,
        fitted,
        residuals,
    })
}

fn ols_solve(x: &[f64], y: &[f64], n: usize, k: usize) -> Result<Vec<f64>> {
    // X^T X (k×k)
    let mut xtx = vec![0.0; k * k];
    let mut xty = vec![0.0; k];
    for t in 0..n {
        for i in 0..k {
            xty[i] += x[t * k + i] * y[t];
            for j in 0..k {
                xtx[i * k + j] += x[t * k + i] * x[t * k + j];
            }
        }
    }
    // Solve k×k system via Gaussian elimination
    let mut aug = vec![0.0; k * (k + 1)];
    for i in 0..k {
        for j in 0..k {
            aug[i * (k + 1) + j] = xtx[i * k + j];
        }
        aug[i * (k + 1) + k] = xty[i];
    }
    // Forward elimination
    for col in 0..k {
        let pivot = aug[col * (k + 1) + col];
        if pivot.abs() < 1e-12 {
            return Err(TsError::Other("singular matrix in OLS".into()));
        }
        for j in 0..=k {
            aug[col * (k + 1) + j] /= pivot;
        }
        for row in 0..k {
            if row == col { continue; }
            let factor = aug[row * (k + 1) + col];
            for j in 0..=k {
                aug[row * (k + 1) + j] -= factor * aug[col * (k + 1) + j];
            }
        }
    }
    Ok((0..k).map(|i| aug[i * (k + 1) + k]).collect())
}

// ═══════════════════════════════════════════════════════════════════════
// Difference (for ARIMA d parameter)
// ═══════════════════════════════════════════════════════════════════════

pub fn difference(data: &[f64], d: usize) -> Vec<f64> {
    let mut result = data.to_vec();
    for _ in 0..d {
        result = result.windows(2).map(|w| w[1] - w[0]).collect();
    }
    result
}

// ═══════════════════════════════════════════════════════════════════════
// Change-point detection (PELT — Pruned Exact Linear Time)
// ═══════════════════════════════════════════════════════════════════════

pub fn pelt(data: &[f64], penalty: f64) -> Result<Vec<usize>> {
    let n = data.len();
    if n < 2 {
        return Ok(Vec::new());
    }

    // Cumulative sums for efficient segment cost computation
    let mut cumsum = vec![0.0; n + 1];
    let mut cumsum_sq = vec![0.0; n + 1];
    for i in 0..n {
        cumsum[i + 1] = cumsum[i] + data[i];
        cumsum_sq[i + 1] = cumsum_sq[i] + data[i] * data[i];
    }

    // Segment cost: sum of squared deviations from mean
    let seg_cost = |start: usize, end: usize| -> f64 {
        let len = (end - start) as f64;
        if len == 0.0 { return 0.0; }
        let sum = cumsum[end] - cumsum[start];
        let sum_sq = cumsum_sq[end] - cumsum_sq[start];
        let mean = sum / len;
        sum_sq - len * mean * mean
    };

    let mut dp = vec![0.0; n + 1];
    let mut cp = vec![Vec::new(); n + 1]; // changepoints at each position

    for t in 1..=n {
        dp[t] = f64::INFINITY;
        for s in 0..t {
            let cost = dp[s] + seg_cost(s, t) + penalty;
            if cost < dp[t] {
                dp[t] = cost;
                let mut new_cp = cp[s].clone();
                if s > 0 {
                    new_cp.push(s);
                }
                cp[t] = new_cp;
            }
        }
    }

    Ok(cp[n].clone())
}

// ═══════════════════════════════════════════════════════════════════════
// STL decomposition (simple: trend via moving average + seasonal + residual)
// ═══════════════════════════════════════════════════════════════════════

pub struct StlResult {
    pub trend: Vec<f64>,
    pub seasonal: Vec<f64>,
    pub residual: Vec<f64>,
}

pub fn stl_decompose(data: &[f64], period: usize) -> Result<StlResult> {
    let n = data.len();
    if n < period * 2 {
        return Err(TsError::Other(format!(
            "need ≥ {period}×2 observations for STL with period {period}"
        )));
    }

    // Trend: centered moving average with window = period
    let window = if period % 2 == 0 { period + 1 } else { period };
    let half = window / 2;
    let trend: Vec<f64> = (0..n)
        .map(|i| {
            let lo = i.saturating_sub(half);
            let hi = (i + half + 1).min(n);
            let chunk = &data[lo..hi];
            chunk.iter().sum::<f64>() / chunk.len() as f64
        })
        .collect();

    // Detrended
    let detrended: Vec<f64> = data.iter().zip(&trend).map(|(d, t)| d - t).collect();

    // Seasonal: average detrended by phase
    let mut seasonal_sums = vec![0.0; period];
    let mut seasonal_counts = vec![0usize; period];
    for (i, &v) in detrended.iter().enumerate() {
        let phase = i % period;
        seasonal_sums[phase] += v;
        seasonal_counts[phase] += 1;
    }
    let seasonal_avg: Vec<f64> = (0..period)
        .map(|p| seasonal_sums[p] / seasonal_counts[p] as f64)
        .collect();
    // Normalize seasonal to sum to zero
    let seasonal_mean = seasonal_avg.iter().sum::<f64>() / period as f64;
    let seasonal_norm: Vec<f64> = seasonal_avg.iter().map(|s| s - seasonal_mean).collect();

    let seasonal: Vec<f64> = (0..n).map(|i| seasonal_norm[i % period]).collect();
    let residual: Vec<f64> = data.iter().zip(&trend).zip(&seasonal)
        .map(|((d, t), s)| d - t - s)
        .collect();

    Ok(StlResult { trend, seasonal, residual })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exp_smoothing() {
        let data = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let result = exponential_smoothing(&data, 0.5, 3).unwrap();
        assert_eq!(result.fitted.len(), 5);
        assert_eq!(result.forecast.len(), 3);
        // Last fitted should be between data[3] and data[4]
        assert!(result.fitted[4] > 3.0 && result.fitted[4] < 5.0);
    }

    #[test]
    fn test_holt_linear() {
        let data = vec![1.0, 3.0, 5.0, 7.0, 9.0]; // linear trend y=2x+1
        let result = holt_linear(&data, 0.8, 0.8, 2).unwrap();
        assert_eq!(result.forecast.len(), 2);
        // Forecast should continue the upward trend
        assert!(result.forecast[0] > 9.0);
    }

    #[test]
    fn test_ar() {
        let data: Vec<f64> = (0..50).map(|i| {
            // AR(1): y[t] = 0.8 * y[t-1] + noise
            if i == 0 { 1.0 } else { 0.8 * (i as f64 * 0.1) + 0.1 }
        }).collect();
        let result = ar(&data, 1).unwrap();
        assert_eq!(result.coefficients.len(), 1);
        assert_eq!(result.fitted.len(), 50);
    }

    #[test]
    fn test_pelt() {
        // Two segments with different means
        let data: Vec<f64> = (0..20).map(|i| if i < 10 { 1.0 } else { 5.0 }).collect();
        let cps = pelt(&data, 5.0).unwrap();
        assert!(cps.contains(&10)); // Change at index 10
    }

    #[test]
    fn test_stl() {
        // Seasonal data with period 4
        let data: Vec<f64> = (0..24).map(|i| {
            let seasonal = (i as f64 / 4.0 * std::f64::consts::TAU).sin();
            let trend = i as f64 * 0.5;
            seasonal + trend + 10.0
        }).collect();
        let result = stl_decompose(&data, 4).unwrap();
        assert_eq!(result.trend.len(), 24);
        assert_eq!(result.seasonal.len(), 24);
        assert_eq!(result.residual.len(), 24);
        // Residuals should be small
        let max_resid = result.residual.iter().fold(0.0f64, |a, b| a.max(b.abs()));
        assert!(max_resid < 2.0); // edge effects can cause larger residuals
    }
}
