//! Combinators — closure property of hypothesis tests.
//!
//! Each function takes a slice of component tests (or raw p-values) and
//! returns a new [`HypothesisTest`], preserving the algebra's closure so
//! combinations can be combined further. Methods: Fisher's, Stouffer's,
//! Tippett's, Wilkinson's, plus the Boolean algebra (intersection / union /
//! complement).
//!
//! All combinators reject when any component p-value lies outside `(0, 1]`
//! (Fisher/Stouffer/Tippett/Wilkinson) since `log(0)` diverges; the Boolean
//! combinators accept `0` because they only use `min`/`max`/`1−p`.

pub mod boolean;
pub mod fisher;
pub mod stouffer;
pub mod tippett;
pub mod wilkinson;

use crate::{HypothesisTest, Result};

/// Trait abstracting "something that exposes a p-value", so combinators
/// accept `&HypothesisTest` and `f64` interchangeably (matching R).
pub trait PvalSource {
    fn pval(&self) -> f64;
}

impl PvalSource for HypothesisTest {
    fn pval(&self) -> f64 {
        self.p_value
    }
}

impl PvalSource for f64 {
    fn pval(&self) -> f64 {
        *self
    }
}

pub(crate) fn extract_pvals<S: PvalSource>(srcs: &[S]) -> Result<Vec<f64>> {
    Ok(srcs.iter().map(|s| s.pval()).collect())
}

/// Validate every p-value lies in `(0, 1]`. Used by Fisher/Stouffer/Tippett
/// where `log(p)` or `Φ⁻¹(1−p)` diverges at 0.
pub(crate) fn validate_strict(pvals: &[f64]) -> Result<()> {
    for &p in pvals {
        if !(p > 0.0 && p <= 1.0) {
            return Err(crate::HypoError::PvalOutOfRange(p));
        }
    }
    Ok(())
}
