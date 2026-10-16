//! Typed Rust SDK and DAG integration for CZ CELLxGENE Discover.

pub mod client;
pub mod error;
pub mod nodes;
pub mod types;

pub use client::{CellxgeneClient, DEFAULT_ENDPOINT};
pub use error::{CellxgeneError, Result};
pub use types::{Collection, CollectionDetail, Dataset, DatasetAsset, DatasetFilter};
