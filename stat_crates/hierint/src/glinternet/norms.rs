//! Norm computations for screening and KKT checking.
//!
//! Port of `compute_norms_*` functions in `c_routines.c`.
//! These compute ‖Xᵢᵀr‖₂ per group (divided by n) to decide which groups
//! should enter the active set.

use super::{Candidates, GlinternetData, Norms};

/// Compute norms for categorical main effects.
///
/// Port of `compute_norms_cat()` in `c_routines.c`.
/// Result: `sqrt(sum(temp[l]^2) / n) / n` per categorical variable.
pub fn compute_norms_cat(data: &GlinternetData, r: &[f64]) -> Vec<f64> {
    let n = data.n;
    let p = data.p_cat;
    let levels = &data.levels;
    let mut result = vec![0.0; p];

    for j in 0..p {
        let n_levels = levels[j];
        let mut temp = vec![0.0; n_levels];
        let xptr = &data.xcat[j * n..(j + 1) * n];
        for i in 0..n {
            temp[xptr[i]] += r[i];
        }
        let mut sum_sq = 0.0;
        for &t in &temp {
            sum_sq += t * t;
        }
        result[j] = (sum_sq / n as f64).sqrt() / n as f64;
    }

    result
}

/// Compute norms for continuous main effects.
///
/// Port of `compute_norms_cont()` in R (matrix multiply).
/// Result: `|Zᵀr| / n` per continuous variable.
pub fn compute_norms_cont(data: &GlinternetData, r: &[f64]) -> Vec<f64> {
    let n = data.n;
    let p = data.p_cont;
    let mut result = vec![0.0; p];

    for j in 0..p {
        let mut dot = 0.0;
        let zptr = &data.z[j * n..(j + 1) * n];
        for i in 0..n {
            dot += zptr[i] * r[i];
        }
        result[j] = dot.abs() / n as f64;
    }

    result
}

/// Compute norms for categorical × categorical interactions.
///
/// Port of `compute_norms_cat_cat()` in `c_routines.c`.
/// Each row of `indices` gives (cat_i, cat_j) — 1-based within cat subset.
pub fn compute_norms_cat_cat(
    data: &GlinternetData,
    r: &[f64],
    indices: &[[usize; 2]],
) -> Vec<f64> {
    let n = data.n;
    let p = indices.len();
    let mut result = vec![0.0; p];

    for (j, &[xi, yi]) in indices.iter().enumerate() {
        let l1 = data.levels[xi - 1];
        let l2 = data.levels[yi - 1];
        let len = l1 * l2;
        let mut temp = vec![0.0; len];
        let xptr = &data.xcat[(xi - 1) * n..xi * n];
        let yptr = &data.xcat[(yi - 1) * n..yi * n];
        for i in 0..n {
            temp[xptr[i] + l1 * yptr[i]] += r[i];
        }
        let mut sum_sq = 0.0;
        for &t in &temp {
            sum_sq += t * t;
        }
        result[j] = (sum_sq / n as f64).sqrt() / n as f64;
    }

    result
}

/// Compute norms for continuous × continuous interactions.
///
/// Port of `compute_norms_cont_cont()` in `c_routines.c`.
/// Uses precomputed contNorms for efficiency.
pub fn compute_norms_cont_cont(
    data: &GlinternetData,
    cont_norms: &[f64],
    r: &[f64],
    indices: &[[usize; 2]],
) -> Vec<f64> {
    let n = data.n;
    let p = indices.len();
    let mut result = vec![0.0; p];

    for (j, &[xi, yi]) in indices.iter().enumerate() {
        let wptr = &data.z[(xi - 1) * n..xi * n];
        let zptr = &data.z[(yi - 1) * n..yi * n];

        let mut mean = 0.0;
        let mut norm_sq = 0.0;
        for i in 0..n {
            let prod = wptr[i] * zptr[i];
            mean += prod;
            norm_sq += prod * prod;
        }
        mean /= n as f64;
        let var = norm_sq - n as f64 * mean * mean;

        let mut dot_r = 0.0;
        for i in 0..n {
            dot_r += r[i] * (wptr[i] * zptr[i] - mean);
        }

        let term = if var.abs() > 1e-30 {
            dot_r * dot_r / var
        } else {
            0.0
        };

        let n_sq = n as f64 * n as f64;
        result[j] = ((n_sq * (cont_norms[xi - 1].powi(2) + cont_norms[yi - 1].powi(2)) + term) / 3.0).sqrt() / n as f64;
    }

    result
}

/// Compute norms for categorical × continuous interactions.
///
/// Port of `compute_norms_cat_cont()` in `c_routines.c`.
pub fn compute_norms_cat_cont(
    data: &GlinternetData,
    cat_norms: &[f64],
    r: &[f64],
    indices: &[[usize; 2]],
) -> Vec<f64> {
    let n = data.n;
    let p = indices.len();
    let mut result = vec![0.0; p];

    for (j, &[xi, zi]) in indices.iter().enumerate() {
        let n_levels = data.levels[xi - 1];
        let xptr = &data.xcat[(xi - 1) * n..xi * n];
        let zptr = &data.z[(zi - 1) * n..zi * n];

        let mut temp = vec![0.0; n_levels];
        for i in 0..n {
            temp[xptr[i]] += zptr[i] * r[i];
        }

        // result[j] = sqrt( (n * catNorms[xi])^2 + sum(temp^2) ) / sqrt(2) / n
        let mut sum_sq = (n as f64 * cat_norms[xi - 1]).powi(2);
        for &t in &temp {
            sum_sq += t * t;
        }
        result[j] = (sum_sq / 2.0).sqrt() / n as f64;
    }

    result
}

/// Compute the maximum lambda value from candidate norms.
pub fn get_lambda_max(candidates: &Candidates) -> f64 {
    candidates.norms.max_norm()
}

/// Generate lambda grid.
///
/// Port of `get_lambda_grid()` in R.
pub fn get_lambda_grid(candidates: &Candidates, n_lambda: usize, lambda_min_ratio: f64) -> Vec<f64> {
    let lambda_max = get_lambda_max(candidates);
    let lambda_min = lambda_min_ratio * lambda_max;

    (0..n_lambda)
        .map(|k| {
            let f = k as f64 / (n_lambda - 1) as f64;
            lambda_max.powf(1.0 - f) * lambda_min.powf(f)
        })
        .collect()
}
