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

pub mod chisq;
pub mod error;
pub mod rcs;
pub mod roc;

pub use error::{EpiError, Result};
