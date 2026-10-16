//! Typed Rust SDK and DAG integration for the Human Cell Atlas Azul Data Service.

pub mod client;
pub mod error;
pub mod nodes;
pub mod types;

pub use client::{DEFAULT_ENDPOINT, HcaClient};
pub use error::{HcaError, Result};
