//! `hierint` — Rust port of **hierNet** and **glinternet** for hierarchical interaction discovery.
//!
//! Two methods for learning pairwise interactions with hierarchy constraints:
//!
//! | Method | Regularization | Variable types | Key reference |
//! |--------|---------------|----------------|---------------|
//! | [`glinternet`] | Group-lasso + FISTA | Categorical (arbitrary levels) + continuous + mixed | Lim & Hastie (2015), JCGS |
//! | [`hiernet`] | L1 + ADMM / generalized gradient | Continuous only | Bien et al. (2013), Annals of Stat. |
//!
//! Both enforce **strong hierarchy**: an interaction enters the model only if both
//! associated main effects are present.

pub mod error;
pub mod glinternet;
pub mod hiernet;

pub use error::{HierIntError, Result};
