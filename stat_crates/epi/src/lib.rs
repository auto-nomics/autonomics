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
//! | [`causal`] | IPTW + PSM (propensity score causal inference)               |
//! | [`clpm`]  | Cross-Lagged Panel Model (2-wave longitudinal reciprocal effects) |
//! | [`gbtm`]  | Group-Based Trajectory Modeling (Nagin mixture of polynomials)    |
//! | [`lca`]   | Latent Class Analysis (EM mixture of Bernoullis)                 |
//! | [`ensemble`] | Random Forest classifier (CART + bootstrap + mtry)          |
//! | [`shap`]   | SHAP feature attribution (Saabas path-dependent for trees)     |
//! | [`competing_risk`] | CIF (Aalen-Johansen) + Fine-Gray subdistribution hazard  |
//! | [`multistate`] | Multi-state Markov model (Nelson-Aalen + Aalen-Johansen)  |
//! | [`sem`]    | Structural Equation Modeling (CFA via ML fit function)         |
//! | [`cmest`]  | CMAverse-compatible causal mediation (Valeri/VanderWeele rb)    |

pub mod causal;
pub mod chisq;
pub mod clpm;
pub mod cmest;
pub mod competing_risk;
pub mod ensemble;
pub mod error;
pub mod gbtm;
pub mod lasso;
pub mod lca;
pub mod mediation;
pub mod multistate;
pub mod rcs;
pub mod roc;
pub mod sem;
pub mod shap;
pub mod survival;
pub mod wqs;

pub use error::{EpiError, Result};
