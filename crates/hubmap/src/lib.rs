//! Typed Rust SDK and DAG integration for HuBMAP.

pub mod client;
pub mod error;
pub mod nodes;
pub mod types;

pub use client::{DEFAULT_ENTITY_ENDPOINT, DEFAULT_SEARCH_ENDPOINT, HubmapClient};
pub use error::{HubmapError, Result};
