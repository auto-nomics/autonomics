//! Generic Nelder-Mead simplex optimiser.
//!
//! A dimension-agnostic port of `scipy.optimize.minimize(…, method='Nelder-Mead')`,
//! matching the solver options used by MTAG's `numerical_omega` and `ss_estimation`:
//!   - `fatol` (absolute function-value convergence)
//!   - `xatol` (absolute parameter convergence)
//!   - `maxiter`
//!
//! The classical coefficients are α=1 (reflection), γ=2 (expansion),
//! ρ=0.5 (contraction), σ=0.5 (shrink).

/// Nelder-Mead minimisation of a generic n-dimensional function.
///
/// - `f` — the objective (minimised).
/// - `x0` — starting point (length = dimensionality `n`).
/// - `initial_step` — initial perturbation for building the simplex.
/// - `xatol` — absolute tolerance on parameter convergence.
/// - `fatol` — absolute tolerance on function-value convergence.
/// - `max_iter` — maximum iterations.
///
/// Returns the best parameter vector found.
pub fn nelder_mead_generic<F>(
    f: F,
    x0: &[f64],
    initial_step: f64,
    xatol: f64,
    fatol: f64,
    max_iter: usize,
) -> Vec<f64>
where
    F: Fn(&[f64]) -> f64,
{
    let n = x0.len();
    debug_assert!(n > 0);

    const ALPHA: f64 = 1.0; // reflection
    const GAMMA: f64 = 2.0; // expansion
    const RHO: f64 = 0.5; // contraction
    const SIGMA: f64 = 0.5; // shrink

    // Build the initial simplex: x0 plus n vertices offset by initial_step.
    let mut simplex: Vec<(Vec<f64>, f64)> = Vec::with_capacity(n + 1);
    simplex.push((x0.to_vec(), f(x0)));
    for i in 0..n {
        let mut x = x0.to_vec();
        x[i] += initial_step;
        let cost = f(&x);
        simplex.push((x, cost));
    }

    for _ in 0..max_iter {
        // Sort by cost ascending (best first).
        simplex.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        // Convergence check: function-value spread and parameter spread.
        let f_spread = simplex[n].1 - simplex[0].1;
        if f_spread.abs() < fatol {
            let mut x_spread = 0.0;
            for i in 0..n {
                let diff = (simplex[1 + i].0[i] - simplex[0].0[i]).abs();
                if diff > x_spread {
                    x_spread = diff;
                }
            }
            if x_spread < xatol {
                break;
            }
        }

        // Centroid of all but worst.
        let mut centroid = vec![0.0f64; n];
        for vertex in simplex.iter().take(n) {
            for (d, sum) in centroid.iter_mut().enumerate() {
                *sum += vertex.0[d];
            }
        }
        for d in 0..n {
            centroid[d] /= n as f64;
        }

        let worst = &simplex[n].0;
        let best_cost = simplex[0].1;
        let second_worst_cost = simplex[n - 1].1;

        // Reflection.
        let xr: Vec<f64> = (0..n)
            .map(|d| centroid[d] + ALPHA * (centroid[d] - worst[d]))
            .collect();
        let fr = f(&xr);

        let new_point = if fr < best_cost {
            // Expansion.
            let xe: Vec<f64> = (0..n)
                .map(|d| centroid[d] + GAMMA * (xr[d] - centroid[d]))
                .collect();
            let fe = f(&xe);
            if fe < fr { (xe, fe) } else { (xr, fr) }
        } else if fr < second_worst_cost {
            // Accept reflection.
            (xr, fr)
        } else {
            // Contraction.
            let dir = if fr < simplex[n].1 { &xr } else { worst };
            let xc: Vec<f64> = (0..n)
                .map(|d| centroid[d] + RHO * (dir[d] - centroid[d]))
                .collect();
            let fc = f(&xc);
            if fc < simplex[n].1 {
                (xc, fc)
            } else {
                // Shrink toward best.
                let best = simplex[0].0.clone();
                for vertex in simplex.iter_mut().take(n + 1).skip(1) {
                    let xs: Vec<f64> = (0..n)
                        .map(|d| best[d] + SIGMA * (vertex.0[d] - best[d]))
                        .collect();
                    let fs = f(&xs);
                    *vertex = (xs, fs);
                }
                continue; // skip worst replacement
            }
        };

        simplex[n] = new_point;
    }

    simplex.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    simplex[0].0.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quadratic() {
        // f(x) = sum((x_i - i)^2), minimum at x = [0, 1, 2, ...]
        let f = |x: &[f64]| {
            x.iter()
                .enumerate()
                .map(|(i, &v)| (v - i as f64).powi(2))
                .sum::<f64>()
        };
        let x0 = vec![5.0, 5.0, 5.0, 5.0];
        let result = nelder_mead_generic(f, &x0, 0.5, 1e-10, 1e-12, 5000);
        for i in 0..4 {
            assert!(
                (result[i] - i as f64).abs() < 1e-4,
                "x[{i}] = {}",
                result[i]
            );
        }
    }

    #[test]
    fn test_rosenbrock_2d() {
        // Rosenbrock: f(x,y) = (1-x)^2 + 100(y-x^2)^2, minimum at (1,1)
        let f = |x: &[f64]| (1.0 - x[0]).powi(2) + 100.0 * (x[1] - x[0].powi(2)).powi(2);
        let x0 = vec![-1.2, 1.0];
        let result = nelder_mead_generic(f, &x0, 0.5, 1e-10, 1e-12, 5000);
        assert!((result[0] - 1.0).abs() < 1e-3, "x = {}", result[0]);
        assert!((result[1] - 1.0).abs() < 1e-3, "y = {}", result[1]);
    }
}
