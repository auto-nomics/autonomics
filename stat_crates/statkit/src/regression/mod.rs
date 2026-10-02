//! Regression primitives: OLS / WLS and binary logistic regression.
//!
//! Both share the same slice-based design-matrix convention: predictors are
//! supplied as parallel `&[&[f64]]` columns, each of length `n`. When
//! `intercept = true` a column of ones is prepended (so `coefficients[0]`
//! is the intercept).
//!
//! - [`ols`]/[`wls`]: normal equations `(Xᵀ W X) β = Xᵀ W y` via faer Cholesky;
//!   p-values from Student-t (`statrs`).
//! - [`logistic`]: IRLS (Newton-Raphson) with Wald z-tests, odds ratios, and
//!   95% CIs; log-likelihood returned for downstream LR tests.

mod cox;
mod logistic;
mod negbin;
mod ols;
mod ordinal_logistic;
mod rlm;
mod rrr;

pub use cox::{CoxResult, cox};
pub use logistic::{LogisticResult, logistic, logistic_weighted};
pub use negbin::{NegbinOptions, NegbinResult, negbin};
pub use ols::{Regression, ols, wls};
pub use ordinal_logistic::{OrdinalLogisticResult, ordinal_logistic};
pub use rlm::{PsiFunction, RlmOptions, RlmResult, rlm};
pub use rrr::{RrrResult, reduced_rank_regression};
