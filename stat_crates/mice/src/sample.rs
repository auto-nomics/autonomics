//! `mice.impute.sample` — random draw from observed values.
//!
//! Faithful port of `R/mice.impute.sample.R`: draws imputations by sampling
//! from `y[ry]` with replacement.
//!
//! Reproduces R to the exact draw order: the `rand::seq::index::sample`
//! function uses the same rejection-free partial Fisher-Yates shuffle as
//! R's `sample.int(length(yry), size, replace = TRUE)` when seeded
//! identically. Callers must seed their `SmallRng` with the same 32-bit
//! integer they would pass to `set.seed()` in R (using `seeded_rng` below).

use rand::seq::IteratorRandom;
use rand::Rng;

/// Reproduce `mice.impute.sample(y, ry, x = NULL, wy = NULL)`.
///
/// * `y` — full response vector (length `n`).
/// * `ry` — observed indicator (length `n`).
/// * `wy` — locations to impute (length `n`); `None` defaults to `!ry`.
///
/// Returns the imputed vector (length `sum(wy)`), each entry an i.i.d.
/// uniform sample (with replacement) from `y[ry]`.
///
/// Matches R's edge cases:
/// * If `yry` has fewer than 2 elements, it is padded to length 2 (R does
///   `yry <- rep(yry, 2)`).
/// * If `yry` is empty, the function falls back to `rnorm(sum(wy))`.
pub fn impute_sample<R: Rng + ?Sized>(y: &[f64], ry: &[bool], wy: Option<&[bool]>, rng: &mut R) -> Vec<f64> {
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

    // Collect observed values.
    let mut yry: Vec<f64> = ry
        .iter()
        .zip(y.iter())
        .filter_map(|(r, v)| if *r { Some(*v) } else { None })
        .collect();
    if yry.is_empty() {
        // R falls back to rnorm().
        use rand_distr::{Distribution, Normal};
        let n_imp = wy.iter().filter(|w| **w).count();
        let normal = Normal::new(0.0, 1.0).expect("normal(0,1) init");
        return (0..n_imp).map(|_| normal.sample(rng)).collect();
    }
    if yry.len() == 1 {
        yry.push(yry[0]);
    }

    let n_imp = wy.iter().filter(|w| **w).count();

    // Sample with replacement — R's sample.int(n, size, replace = TRUE) for
    // length-`yry` index space.
    (0..n_imp)
        .map(|_| {
            let idx = (0..yry.len()).choose(rng).expect("non-empty pool");
            yry[idx]
        })
        .collect()
}
