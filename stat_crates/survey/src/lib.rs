//! `survey` — Rust port of Thomas Lumley's R `survey` package (v4.5).
//!
//! Provides design-based estimation for complex survey samples:
//! stratified, cluster-sampled, unequally weighted designs with Taylor-
//! series linearisation variances.
//!
//! ## Status
//!
//! **Layer 0** (design + linearisation) is partially implemented:
//! - [`design::SurveyDesign`] — survey design object (strata, clusters,
//!   weights/probs, FPC, lonely-PSU policy).
//! - [`variance::svy_cprod`] — single-stage stratified cluster variance
//!   (R `svyCprod`).
//! - [`describe::svymean`] / [`describe::svytotal`] — weighted mean and
//!   total with design-based SE.
//!
//! See memory `survey-crate-scope` for the full porting plan.
//!
//! ## Golden validation
//!
//! All computations are validated against R `survey` v4.5 output using the
//! `fpc` test dataset (8 obs, 2 strata). Golden values:
//! - `svymean(~x, withoutfpc)` → mean=5.448148, SE=0.7412683
//! - `svymean(~x, withfpc)` → mean=5.448148, SE=0.6160407

pub mod calibrate;
pub mod design;
pub mod describe;
pub mod error;
pub mod model;
pub mod nonlinear;
pub mod survival;
pub mod test;
pub mod variance;

pub use calibrate::{calibrate_linear, post_stratify, rake, svy_standardize, trim_weights};
pub use design::{LonelyPsu, SurveyDesign, SurveyDesignBuilder};
pub use describe::{svy_ciprop, svy_quantile, svy_ranktest, svycontrast, svymean, svyratio, svytotal, svyvar, svy_ttest_onesample, svy_ttest_twosample, Contrast, SurveyStat, SvyRankTest, SvyTtest};
pub use error::{Result, SurveyError};
pub use model::{reg_term_test, svyglm_linear, RegTermTest, SvyGlmFit};
pub use nonlinear::{svy_nls, svy_ivreg, svy_olr, svy_loglin, SvyNlsFit, SvyIvregFit, SvyOlrFit, SvyLoglinFit};
pub use survival::{svy_km, svy_coxph, svy_logrank, svy_survreg, SvyKm, SvyCoxphFit, SvyLogrankTest, SvySurvregFit};
pub use test::{svy_chisq, SvyChisq};
