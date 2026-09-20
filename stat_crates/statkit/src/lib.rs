//! `statkit` — foundational statistics primitives for the autonomics workspace.
//!
//! Provides the shared statistical building blocks used by higher-level method
//! crates (e.g. `epi`) and by data-engine nodes:
//!
//! | Module           | Contents                                                |
//! |------------------|---------------------------------------------------------|
//! | [`descriptive`]  | Mean, variance, rank, quantile, correlation              |
//! | [`regression`]   | OLS / WLS / logistic regression with inference           |
//!
//! Distribution functions (Normal, Student-t, χ², F) are provided by the
//! external [`statrs`](https://docs.rs/statrs) crate rather than hand-written,
//! keeping this layer thin and well-tested. Matrix operations use [`faer`].

pub mod descriptive;
pub mod error;
pub mod icc;
pub mod regression;

pub use error::{Result, StatError};
pub use icc::{IccResult, icc_2_1, icc_2_1_level};
