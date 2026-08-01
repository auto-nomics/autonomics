//! `epi` — epidemiology & clinical biostatistics methods.
//!
//! Built on top of [`statkit`] (regression, descriptive stats), providing the
//! higher-level analysis methods used in observational cohort studies:
//!
//! | Module    | Contents                                                       |
//! |-----------|----------------------------------------------------------------|
//! | [`chisq`] | Pearson χ² test, Bonferroni correction, pairwise comparisons   |
//! | [`roc`]   | ROC/AUC (Mann-Whitney), DeLong test, bootstrap CI              |
//! | [`rcs`]   | Restricted cubic splines with nonlinearity tests               |
//! | [`lasso`] | LASSO logistic regression (coordinate descent, k-fold CV)      |
//! | [`wqs`]   | Weighted Quantile Sum regression (constrained opt, bootstrap)  |
//! | [`survival`] | Kaplan-Meier estimator, log-rank (Mantel-Cox) test          |
//! | [`mediation`] | Causal mediation analysis (VanderWeele decomposition)     |

pub mod chisq;
pub mod error;
pub mod lasso;
pub mod mediation;
pub mod rcs;
pub mod roc;
pub mod survival;
pub mod wqs;

pub use error::{EpiError, Result};
