//! Typed Rust SDK and DAG integration for the ClinicalTrials.gov v2 API.

pub mod client;
pub mod error;
pub mod format;
pub mod nodes;
pub mod tools;
pub mod types;

pub use client::ClinicalTrialsClient;
pub use error::{ClinicalTrialsError, Result};
pub use tools::registrations;
