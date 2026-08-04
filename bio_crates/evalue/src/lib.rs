//! `evalue` — a pure-Rust port of the **EValue** R package (v4.1.4)
//! by Mathur, Smith, Ding & VanderWeele.
//!
//! E-values quantify the minimum strength of association (on the risk-ratio
//! scale) that unmeasured confounding, selection bias, or misclassification
//! would need to have with both the exposure and the outcome to fully explain
//! away an observed treatment–outcome association.
//!
//! # References
//!
//! - VanderWeele TJ & Ding P (2017). *Sensitivity analysis in observational
//!   research: Introducing the E-value.* Annals of Internal Medicine 167(4):268–75.
//! - Smith LH & VanderWeele TJ (2019). *Bounding bias due to selection.*
//!   Epidemiology 30(4):509–16.
//! - Mathur MB & VanderWeele TJ (2020a). *Sensitivity analysis for unmeasured
//!   confounding in meta-analyses.* JASA.
//! - Mathur MB et al. (2021). *E-values for effect modification and
//!   approximations for causal interaction.* IJE.

#![allow(clippy::needless_range_loop)]

pub mod bias_spec;
pub mod bootstrap;
pub mod calib;
pub mod effect_mod;
pub mod error;
pub mod evalue;
pub mod math_utils;
pub mod measure;
pub mod meta;
pub mod multi_bound;
pub mod multi_evalue;
pub mod selection;

pub use error::{EvalueError, Result};
