//! Competing-risk risk-estimation metrics (`crrkit`) — the external-validation
//! scoring layer for cause-1 cumulative-incidence predictions.
//!
//! The chordoma SAP's primary and key-secondary comparisons all reduce to:
//! freeze models `M0..M3`, predict once on an untouched external cohort, and
//! score the three-year cumulative incidence of local recurrence treating
//! pre-recurrence death as a **competing event**, with administrative and
//! loss-to-follow-up censoring handled by inverse-probability weighting.
//!
//! Provided here:
//!
//! * [`ipcw_brier`] — the IPCW Brier score under competing risks (SAP §9.5),
//!   with per-subject contributions and the `G(t*)` / extreme-weight
//!   diagnostics the SAP requires to be reported;
//! * [`ipcw_auc`] — time-dependent AUC whose controls include subjects dead
//!   of the competing cause, properly censoring-weighted (not a plain ROC);
//! * [`grouped_calibration`] / [`calibration_slope`] — competing-risk
//!   calibration against the IPCW-observed event probability;
//! * [`paired_brier_delta_bootstrap`] — patient-level paired bootstrap of
//!   `BS(M_ref) − BS(M_new)` re-estimating the censoring weights inside every
//!   resample.
//!
//! Event-status coding follows the SAP (§5.2): `0` right-censored, `1` the
//! event of interest, `2` the competing event; times are in days.
//!
//! # Relationship to the rest of the workspace
//!
//! `stat_crates/cmprsk` fits the Fine–Gray models whose CIF predictions are
//! scored here; `stat_crates/dl::survival::brier_score` is a single-event
//! IPCW Brier that treats competing events as censoring and **must not** be
//! used for competing-risk endpoints — this crate is its replacement for
//! that purpose.

pub mod auc;
pub mod brier;
pub mod bootstrap;
pub mod calibration;
pub mod censoring;
pub mod error;

pub use auc::{IpcwAuc, ipcw_auc};
pub use brier::{BrierOptions, IpcwBrier, ipcw_brier};
pub use bootstrap::{BootstrapOptions, PairedBootstrapDelta, paired_brier_delta_bootstrap};
pub use calibration::{CalibrationGroup, CalibrationSlope, calibration_slope, grouped_calibration};
pub use censoring::ReverseKmCensoring;
pub use error::{CrrkitError, Result};
