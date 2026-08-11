//! Generalized least squares summary — port of `R/summaryGLS.R`.

use faer::Mat;

use crate::error::Result;
use crate::linalg;
use crate::stats;

/// GLS regression result.
#[derive(Clone, Debug)]
pub struct GlsResult {
    pub betas: Vec<f64>,
    pub pvals: Vec<f64>,
    pub se: Vec<f64>,
    pub z: Vec<f64>,
    pub names: Vec<String>,
}

/// Run GLS regression: y = Xβ + ε, with Ω as the covariance of ε.
///
/// β = (X'Ω⁻¹X)⁻¹ X'Ω⁻¹y
/// SE = sqrt(diag((X'Ω⁻¹X)⁻¹))
pub fn summary_gls(
    y: &[f64],
    omega: &Mat<f64>,
    predictors: &[Vec<f64>],
    intercept: bool,
) -> Result<GlsResult> {
    let n_pred = predictors.len();

    // Build design matrix X
    let n_cols = if intercept { n_pred + 1 } else { n_pred };
    let n_obs = if n_pred > 0 { predictors[0].len() } else { 0 };

    let mut x_data = vec![vec![0.0f64; n_cols]; n_obs];
    for i in 0..n_obs {
        let mut col = 0;
        if intercept {
            x_data[i][col] = 1.0;
            col += 1;
        }
        for p in 0..n_pred {
            x_data[i][col] = predictors[p][i];
            col += 1;
        }
    }

    // Compute Ω⁻¹
    let omega_inv = linalg::inverse(omega);

    // Build X'Ω⁻¹X and X'Ω⁻¹y
    let mut xt_omega_inv_x = vec![vec![0.0f64; n_cols]; n_cols];
    let mut xt_omega_inv_y = vec![0.0f64; n_cols];

    for i in 0..n_obs {
        for j in 0..n_cols {
            for k in 0..n_cols {
                xt_omega_inv_x[j][k] += x_data[i][j] * omega_inv[(i, i)] * x_data[i][k];
            }
            xt_omega_inv_y[j] += x_data[i][j] * omega_inv[(i, i)] * y[i];
        }
    }

    // Solve for β
    let betas = crate::ldsc::solve_small_pub(&xt_omega_inv_x, &xt_omega_inv_y);

    // SE = sqrt(diag((X'Ω⁻¹X)⁻¹))
    let xtx_inv = invert_small(&xt_omega_inv_x);
    let se: Vec<f64> = (0..n_cols).map(|i| xtx_inv[i][i].max(0.0).sqrt()).collect();

    // Z and P
    let z: Vec<f64> = (0..n_cols).map(|i| betas[i] / se[i].max(1e-15)).collect();
    let pvals: Vec<f64> = z.iter().map(|&zv| stats::pnorm_two_sided(zv)).collect();

    // Names
    let mut names = Vec::new();
    if intercept {
        names.push("b0".to_string());
    }
    for i in 0..n_pred {
        names.push(format!("b{}", i + 1));
    }

    Ok(GlsResult { betas, pvals, se, z, names })
}

fn invert_small(a: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = a.len();
    let mut aug = vec![vec![0.0f64; 2 * n]; n];
    for i in 0..n { for j in 0..n { aug[i][j] = a[i][j]; } aug[i][n + i] = 1.0; }
    for col in 0..n {
        let mut max_row = col;
        let mut max_val = aug[col][col].abs();
        for row in (col + 1)..n { if aug[row][col].abs() > max_val { max_val = aug[row][col].abs(); max_row = row; } }
        if max_val < 1e-15 { continue; }
        aug.swap(col, max_row);
        let pivot = aug[col][col];
        for j in col..(2 * n) { aug[col][j] /= pivot; }
        for row in 0..n { if row != col { let f = aug[row][col]; for j in col..(2 * n) { aug[row][j] -= f * aug[col][j]; } } }
    }
    (0..n).map(|i| (0..n).map(|j| aug[i][n + j]).collect()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_summary_gls_simple() {
        let y = vec![0.1, 0.2, 0.3, 0.4];
        let omega = Mat::<f64>::identity(4, 4);
        let predictors = vec![vec![1.0, 2.0, 3.0, 4.0]];
        let result = summary_gls(&y, &omega, &predictors, true).unwrap();
        assert_eq!(result.betas.len(), 2);
        assert!(result.betas[1] > 0.0); // positive slope
    }
}
