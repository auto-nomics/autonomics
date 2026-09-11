//! Typed Rust SDK and DAG integration for the PubChem PUG REST API.

pub mod client;
pub mod error;
pub mod format;
pub mod nodes;
pub mod tools;
pub mod types;

pub use client::PubChemClient;
pub use error::{PubChemError, Result};
pub use tools::registrations;
pub use types::CompoundIdentifierType;
