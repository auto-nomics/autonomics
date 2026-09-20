//! Rust port of the R package [`cmprsk`](https://cran.r-project.org/package=cmprsk)
//! (Bob Gray, version 2.2-12) — subdistribution analysis of competing risks.
//!
//! Two analyses are provided, mirroring the reference package:
//!
//! * [`crr`] — Fine & Gray (1999) proportional **subdistribution** hazards
//!   regression, with the robust sandwich variance, Schoenfeld-type score
//!   residuals, the Breslow-type baseline hazard, [`predict_crr`] and
//!   [`summary_crr`].
//! * [`cuminc`] — nonparametric cumulative incidence functions with Aalen-type
//!   variances, and Gray's (1988) stratified k-sample test comparing them
//!   between groups.
//!
//! The numerical kernels are transliterated from the package's Fortran 77
//! (`src/crr.f`, `src/cincsub.f`, `src/crstm.f`, `src/tpoi.f`) rather than
//! re-derived, so results agree with R to near machine precision. See
//! `stat_crates/cmprsk/tests/cross_validation.rs` for the golden-fixture suite.
//!
//! # Example
//!
//! ```no_run
//! use cmprsk::{CrrInput, CrrOptions, TimeFunctions, crr};
//!
//! let ftime = vec![1.0, 2.0, 3.0, 4.0, 5.0];
//! let fstatus = vec![1.0, 0.0, 2.0, 1.0, 0.0];
//! let cov1: Vec<Vec<f64>> = vec![vec![0.1], vec![0.9], vec![0.4], vec![0.7], vec![0.2]];
//! let names = vec!["x1".to_string()];
//!
//! let fit = crr(
//!     &CrrInput {
//!         ftime: &ftime,
//!         fstatus: &fstatus,
//!         cov1: &cov1,
//!         cov1_names: &names,
//!         cov2: &[],
//!         cov2_names: &[],
//!         tf: TimeFunctions::None,
//!         cengroup: None,
//!     },
//!     &CrrOptions::default(),
//! )?;
//! println!("{:?}", fit.coef);
//! # Ok::<(), cmprsk::CmprskError>(())
//! ```

pub mod crr;
pub mod cuminc;
pub mod error;
pub mod kernels;
pub mod km;
pub mod linalg;
pub mod predict;
pub mod ridge;
pub mod summary;

pub use crr::{CrrFit, CrrInput, CrrOptions, TimeFn, TimeFunctions, crr};
pub use cuminc::{CumincCurve, CumincOptions, CumincResult, GrayTest, cuminc, timepoints};
pub use error::{CmprskError, Result};
pub use predict::{CrrPrediction, baseline_cif, predict_crr};
pub use ridge::{CrrRidgeFit, CrrRidgeOptions, crr_ridge, predict_crr_ridge};
pub use summary::{CoefRow, CrrSummary, summary_crr};
