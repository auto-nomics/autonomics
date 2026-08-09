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
pub mod describe;
pub mod design;
pub mod error;
pub mod family;
pub mod model;
pub mod nonlinear;
pub mod survival;
pub mod test;
pub mod variance;

pub use calibrate::{calibrate_linear, post_stratify, rake, svy_standardize, trim_weights};
pub use describe::{
    Contrast, SurveyStat, SvyRankTest, SvyTtest, svy_ciprop, svy_quantile, svy_ranktest,
    svy_ttest_onesample, svy_ttest_twosample, svycontrast, svymean, svyratio, svytotal, svyvar,
};
pub use design::{LonelyPsu, SurveyDesign, SurveyDesignBuilder};
pub use error::{Result, SurveyError};
pub use family::{Family, FamilySpec, Link};
pub use model::{RegTermTest, SvyGlmFit, reg_term_test, svyglm, svyglm_linear};
pub use nonlinear::{
    SvyIvregFit, SvyLoglinFit, SvyNlsFit, SvyOlrFit, svy_ivreg, svy_loglin, svy_nls, svy_olr,
};
pub use survival::{
    SvyCoxphFit, SvyKm, SvyLogrankTest, SvySurvregFit, svy_coxph, svy_km, svy_logrank, svy_survreg,
};
pub use test::{SvyChisq, svy_chisq};
