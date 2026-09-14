//! Typed Rust SDK and DAG integration for the InterPro REST API.

pub mod client;
pub mod error;
pub mod format;
pub mod nodes;
pub mod tools;
pub mod types;

pub use client::InterProClient;
pub use error::{InterProError, Result};
pub use tools::registrations;
