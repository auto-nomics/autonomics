//! FISTA solver for the group-lasso — port of `gl_solver()` in `fista.c`.
//!
//! Implements FISTA (Fast Iterative Shrinkage-Thresholding Algorithm) with:
//! - Adaptive step size via Barzilai-Borwein formula
//! - Backtracking line search with majorization check
//! - Adaptive momentum restart (O'Donoghue & Candes 2012)
//! - KKT-based convergence checking

use super::transform;
use super::{ActiveSet, Family, GlinternetData};

/// Solve group-lasso for a given lambda using FISTA.
///
/// Returns (intercept, beta_nonzero, residual, objective, group_sizes).
///
/// The returned beta excludes the intercept (intercept is separate).
/// Beta is laid out matching the active set's group structure.
pub fn group_lasso(
    data: &GlinternetData,
    y: &[f64],
    active: &ActiveSet,
    betahat_init: &[f64], // [intercept, beta...]
    lambda: f64,
    family: Family,
    tol: f64,
    max_iter: usize,
) -> (f64, Vec<f64>, Vec<f64>, f64, Vec<usize>) {
    let n = data.n;
    let group_sizes = active.group_sizes(&data.levels);
    let beta_len: usize = group_sizes.iter().sum();

    // Extract initial values
    let mut intercept = if betahat_init.is_empty() {
        if family.is_gaussian() { y.iter().sum::<f64>() / n as f64 } else { 0.0 }
    } else {
        betahat_init[0]
    };

    let mut beta: Vec<f64> = if betahat_init.len() > 1 {
        betahat_init[1..].to_vec()
    } else {
        vec![0.0; beta_len]
    };

    // Ensure beta has correct length
    if beta.len() != beta_len {
        beta = vec![0.0; beta_len];
    }

    let mut linear = vec![0.0; n];
    let mut residual = vec![0.0; n];

    // Compute initial linear predictor
    transform::x_times_beta(data, active, &beta, &mut linear);
    transform::update_intercept(y, &linear, &mut intercept, &mut residual, n, family);

    // FISTA state
    let mut gradient = vec![0.0; beta_len];
    let mut gradient_old = vec![0.0; beta_len];
    let mut intermediate = beta.clone();
    let mut intermediate_old = beta.clone();
    let mut beta_old = beta.clone();

    let mut theta = 1.0f64;
    let mut step_size = 1.0f64;
    let alpha = 0.1; // backtracking factor

    let mut obj = 0.0;
    let mut converged = false;

    for iter in 0..max_iter {
        // Save old gradient
        gradient_old.copy_from_slice(&gradient);

        // Compute gradient from residual
        gradient.fill(0.0);
        gradient.copy_from_slice(&transform::compute_gradient(data, active, &residual));

        // Check convergence via KKT
        converged = transform::check_convergence(&beta, &gradient, &group_sizes, lambda, tol);
        if converged {
            break;
        }

        // Save old intermediate
        intermediate_old.copy_from_slice(&intermediate);

        // Compute adaptive step size (Barzilai-Borwein)
        if iter > 0 {
            let mut norm_beta_sq = 0.0;
            let mut norm_grad_sq = 0.0;
            for i in 0..beta_len {
                let db = beta[i] - beta_old[i];
                let dg = gradient[i] - gradient_old[i];
                norm_beta_sq += db * db;
                norm_grad_sq += dg * dg;
            }
            if norm_grad_sq > 0.0 {
                step_size = (norm_beta_sq / norm_grad_sq).sqrt();
            }
        }

        // Optimize step with backtracking
        step_size = optimize_step(
            data,
            y,
            &residual,
            &linear,
            n,
            &group_sizes,
            &mut intercept,
            &beta,
            &mut intermediate,
            &gradient,
            &mut step_size,
            lambda,
            alpha,
            active,
            family,
        );

        // Check if momentum restart is needed
        let theta_old = update_theta(&beta, &intermediate, &intermediate_old, &theta);

        // Update momentum
        theta = (1.0 + (1.0 + 4.0 * theta_old * theta_old).sqrt()) / 2.0;
        let momentum = (theta_old - 1.0) / theta;

        // Update beta: β = intermediate + momentum * (intermediate - intermediate_old)
        beta_old.copy_from_slice(&beta);
        for i in 0..beta_len {
            beta[i] = intermediate[i] + momentum * (intermediate[i] - intermediate_old[i]);
        }

        // Recompute linear predictor and residual
        linear.fill(0.0);
        transform::x_times_beta(data, active, &beta, &mut linear);
        transform::update_intercept(y, &linear, &mut intercept, &mut residual, n, family);
    }

    // Compute final objective
    obj = transform::compute_objective(
        y, &residual, &linear, intercept, &beta, &group_sizes, lambda, n, family,
    );

    (intercept, beta, residual, obj, group_sizes)
}

/// Backtracking line search for step size.
///
/// Port of `optimize_step()` in `fista.c`.
fn optimize_step(
    data: &GlinternetData,
    y: &[f64],
    residual: &[f64],
    linear: &[f64],
    n: usize,
    group_sizes: &[usize],
    intercept: &mut f64,
    beta: &[f64],
    beta_updated: &mut Vec<f64>,
    gradient: &[f64],
    step_size: &mut f64,
    lambda: f64,
    alpha: f64,
    active: &ActiveSet,
    family: Family,
) -> f64 {
    let beta_len: usize = group_sizes.iter().sum();
    let mut step = *step_size;

    // Current loglik
    let loglik = transform::compute_loglik(y, linear, *intercept, n, family);

    let mut delta = vec![0.0f64; beta_len];
    let mut new_linear = vec![0.0; n];

    loop {
        // Compute update with current step
        *beta_updated = transform::compute_update(beta, gradient, group_sizes, step, lambda);

        let mut grad_dot_delta = 0.0;
        let mut delta_sq_norm = 0.0;
        for i in 0..beta_len {
            delta[i] = beta_updated[i] - beta[i];
            grad_dot_delta += gradient[i] * delta[i];
            delta_sq_norm += delta[i] * delta[i];
        }

        // Compute new linear predictor for delta
        new_linear.fill(0.0);
        if family.is_gaussian() {
            // For gaussian: compute loglik of residual ~ X*delta (intercept=0)
            transform::x_times_beta(data, active, &delta, &mut new_linear);
            let loglik_updated = transform::compute_loglik(residual, &new_linear, 0.0, n, family);
            if loglik_updated <= loglik + grad_dot_delta + delta_sq_norm / (2.0 * step) + 1e-12 {
                break;
            }
        } else {
            // For logistic: compute full prediction
            transform::x_times_beta(data, active, beta_updated, &mut new_linear);
            let loglik_updated = transform::compute_loglik(y, &new_linear, *intercept, n, family);
            if loglik_updated <= loglik + grad_dot_delta + delta_sq_norm / (2.0 * step) + 1e-12 {
                break;
            }
        }

        step *= alpha;
        if step < 1e-20 {
            break;
        }
    }

    *step_size = step;
    step
}

/// Momentum restart check.
///
/// Port of `update_theta()` in `fista.c`.
/// If the gradient at the new point points opposite to the momentum direction,
/// reset theta to 1 (restart momentum).
fn update_theta(
    beta: &[f64],
    intermediate: &[f64],
    intermediate_old: &[f64],
    theta: &f64,
) -> f64 {
    let mut value = 0.0;
    for i in 0..beta.len() {
        value += (beta[i] - intermediate[i]) * (intermediate[i] - intermediate_old[i]);
    }
    if value > 0.0 { 1.0 } else { *theta }
}
