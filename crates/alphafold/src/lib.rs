//! Typed Rust SDK and DAG integration for the AlphaFold Protein Structure Database.

pub mod client;
pub mod error;
pub mod format;
pub mod nodes;
pub mod types;

pub use client::AlphaFoldClient;
pub use error::{AlphaFoldError, Result};
