//! Typed Rust SDK and DAG integration for EBI Single Cell Expression Atlas.

pub mod client;
pub mod error;
pub mod nodes;
pub mod types;

pub use client::{DEFAULT_ENDPOINT, SceaClient};
pub use error::{Result, SceaError};
pub use types::{Experiment, ExperimentFilter, ExperimentPageContent, MarkerGene};
