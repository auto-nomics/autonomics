//! Typed Rust SDK and DAG integration for the KPMP Kidney Tissue Atlas.

pub mod client;
pub mod error;
pub mod nodes;
pub mod types;

pub use client::{DEFAULT_ENDPOINT, KpmpClient};
pub use error::{KpmpError, Result};
