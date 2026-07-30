//! Async Rust SDK for the [Open Targets Platform GraphQL API](
//! https://platform.opentargets.org/api).
//!
//! The API is public (no authentication). This crate provides:
//!
//! - **`OpenTargetsClient`** — an async HTTP/GraphQL client with typed helpers
//!   for targets, diseases, drugs, studies, variants, search, and associations.
//! - **Agent tools** — pre-built [`ToolFunction`] implementations that expose
//!   the same capabilities to an agentik agent as LLM-friendly Markdown.
//!
//! # Quick start (SDK only)
//!
//! ```no_run
//! # use opentargets::OpenTargetsClient;
//! # #[tokio::main] async fn main() -> opentargets::Result<()> {
//! let client = OpenTargetsClient::new();
//! let brca1 = client.target("ENSG00000012048").await?.unwrap();
//! println!("{} — {}", brca1.approved_symbol, brca1.approved_name);
//!
//! for assoc in client.associated_diseases_all("ENSG00000012048").await? {
//!     println!("{:.3}\t{}\t{}", assoc.score, assoc.disease.id, assoc.disease.name);
//! }
//! # Ok(()) }
//! ```
//!
//! # Wiring tools into an agent
//!
//! ```no_run,ignore
//! use opentargets::{OpenTargetsClient, opentargets_registrations};
//! use std::sync::Arc;
//!
//! let client = Arc::new(OpenTargetsClient::new());
//! let tools = opentargets_registrations(client);
//! // pass `tools` to Agent::builder().with_tools(tools)
//! ```
//!
//! [`ToolFunction`]: agentik_core::tools::ToolFunction

pub mod associations;
pub mod client;
pub mod error;
pub mod format;
pub mod search;
pub mod tools;
pub mod types;

pub use associations::{AssociatedDisease, AssociatedTarget, AssociationPage, Pagination};
pub use client::{DEFAULT_ENDPOINT, OpenTargetsClient};
pub use error::{OpenTargetsError, Result};
pub use search::{SearchResult, SearchResults};
pub use tools::opentargets_registrations;
pub use types::{
    ApiVersion, DataVersion, Disease, Drug, GenomicLocation, Meta, Sample, ScoredComponent, Study,
    Target, Variant,
};
