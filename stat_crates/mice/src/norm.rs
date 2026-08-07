//! `mice.impute.norm` — Bayesian linear regression imputation for numeric `y`.
//!
//! Faithful port of `R/mice.impute.norm.R`. Procedure:
//!
//!   1. Prepend an intercept column to `x` (R: `cbind(1, as.matrix(x))`).
//!   2. Call `.norm.draw(y, ry, x, ...)` to obtain β̂, β*, σ*.
//!   3. Draw `ẑ ~ N(0, 1)` and return `x[wy] · β* + ẑ · σ*`.
//!
//! The function ignores `x` if it's empty (matching R's `as.matrix(NULL)`
//! behaviour — the design becomes just the intercept column).
//!
//! Reproduces R's `mice.impute.norm` output to ~1e-12 in the cross-validation
//! harness.

use rand::Rng;
use rand_distr::{Distribution, Normal};

use crate::error::Result;
use crate::estimice::norm_draw;

/// Reproduce `mice.impute.norm(y, ry, x, wy = NULL, ...)`.
pub fn impute_norm<R: Rng + ?Sized>(
    y: &[f64],
    ry: &[bool],
    x: &[Vec<f64>],
    wy: Option<&[bool]>,
    ridge: f64,
    rng: &mut R,
) -> Result<Vec<f64>> {
    let wy_owned;
    let wy = match wy {
        Some(v) => v,
        None => {
            wy_owned = ry.iter().map(|r| !*r).collect::<Vec<_>>();
            &wy_owned
        }
    };
    debug_assert_eq!(y.len(), ry.len());
    debug_assert_eq!(y.len(), wy.len());

    // Build design matrix with intercept.
    let n = y.len();
    let p = if x.is_empty() { 0 } else { x[0].len() };
    let mut x_full: Vec<Vec<f64>> = Vec::with_capacity(n);
    for i in 0..n {
        let mut row = vec![1.0_f64];
        if p > 0 {
            debug_assert!(x.len() == n, "x must have n rows");
            row.extend_from_slice(&x[i]);
        }
        x_full.push(row);
    }

    // Subset to observed rows.
    let mut y_obs: Vec<f64> = Vec::new();
    let mut x_obs: Vec<Vec<f64>> = Vec::new();
    let mut x_wy: Vec<Vec<f64>> = Vec::new();
    for i in 0..n {
        if ry[i] {
            y_obs.push(y[i]);
            x_obs.push(x_full[i].clone());
        }
        if wy[i] {
            x_wy.push(x_full[i].clone());
        }
    }

    let parm = norm_draw(&y_obs, &x_obs, ridge, "qr", rng)?;
    let p_full = parm.beta.len();

    let normal = Normal::new(0.0, 1.0).expect("normal(0,1) init");
    let mut out = Vec::with_capacity(x_wy.len());
    for row in &x_wy {
        debug_assert_eq!(row.len(), p_full);
        let mut eta = 0.0;
        for j in 0..p_full {
            eta += row[j] * parm.beta[j];
        }
        let z = normal.sample(rng);
        out.push(eta + z * parm.sigma);
    }
    Ok(out)
}
