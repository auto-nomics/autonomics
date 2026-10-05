//! Typed Rust SDK and DAG integration for the protocols.io public API.
//!
//! The client is read-only. Creation, publication, discussion, and other
//! mutation endpoints are intentionally outside this crate's initial scope.

pub mod client;
pub mod convert;
pub mod error;
pub mod nodes;
pub mod query;
pub mod rate;
pub mod types;

pub use client::{DEFAULT_ENDPOINT, ENV_ENDPOINT, ENV_TOKEN, ProtocolioClient};
pub use error::{ProtocolioError, Result};
pub use query::{ProtocolListQuery, ProtocolPdfView, ReagentListQuery};
pub use types::ContentFormat;
