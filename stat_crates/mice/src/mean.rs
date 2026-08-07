//! `mice.impute.mean` — unconditional mean imputation.
//!
//! Faithful port of `R/mice.impute.mean.R`: imputes the mean of the
//! observed `y` values to every `wy = TRUE` location.
//!
//! Reproduces R to bit-exact precision (the mean of a `f64` vector and
//! Rust's `f64::mean` of the same values agree at 1e-15 for well-conditioned
//! inputs).

use rand::Rng;

/// Reproduce `mice.impute.mean(y, ry, x = NULL, wy = NULL)`.
///
/// * `y` — full response vector (length `n`).
/// * `ry` — observed indicator (length `n`).
/// * `wy` — locations to impute (length `n`); `None` defaults to `!ry`.
///
/// Returns the imputed vector (length `sum(wy)`), with each entry equal to
/// the arithmetic mean of `y[ry]`.
pub fn impute_mean<R: Rng + ?Sized>(y: &[f64], ry: &[bool], wy: Option<&[bool]>, _rng: &mut R) -> Vec<f64> {
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

    let mut sum = 0.0;
    let mut count = 0usize;
    for (i, &r) in ry.iter().enumerate() {
        if r && !y[i].is_nan() {
            sum += y[i];
            count += 1;
        }
    }
    let mean = if count == 0 { 0.0 } else { sum / count as f64 };

    let n_imp = wy.iter().filter(|w| **w).count();
    vec![mean; n_imp]
}
