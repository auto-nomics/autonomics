//! Async Rust SDK for the [OpenAlex REST API](https://api.openalex.org).
//!
//! OpenAlex is an open catalog of the global research system with 270M+
//! works, 90M+ authors, and 100K+ sources. The API is free; an optional
//! API key increases the daily rate-limit budget.
//!
//! This crate provides:
//!
//! - **[`OpenAlexClient`]** — an async HTTP client supporting list, get,
//!   search, autocomplete, and group-by operations across all major entity
//!   types (works, authors, sources, institutions, topics, funders).
//! - **Agent tools** — pre-built [`ToolRegistration`]s that expose OpenAlex
//!   literature-discovery capabilities to an agentik agent.
//!
//! # Quick start (SDK only)
//!
//! ```no_run
//! use openalex::{OpenAlexClient, ListParams};
//!
//! # async fn run() -> openalex::error::Result<()> {
//! let client = OpenAlexClient::new(None);
//! let resp = client
//!     .list_works(
//!         &ListParams::new()
//!             .with_filter("publication_year:2024,is_oa:true,cited_by_count:>100")
//!             .with_sort("cited_by_count:desc")
//!             .with_per_page(10),
//!     )
//!     .await?;
//! println!("{} works found", resp.meta.count);
//! for work in &resp.results {
//!     println!("  {} — {} citations", work.title.as_deref().unwrap_or("?"), work.cited_by_count);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Wiring tools into an agent
//!
//! ```no_run,ignore
//! use openalex::{OpenAlexClient, openalex_registrations};
//! use std::sync::Arc;
//!
//! let client = Arc::new(OpenAlexClient::new(None));
//! let tools = openalex_registrations(client);
//! // pass `tools` to Agent::builder().with_tools(tools)
//! ```
//!
//! # Rate limits
//!
//! Without an API key: $0.10/day budget (~100 list calls). With a free key:
//! $1/day (~1000 list calls). The client sets a polite `User-Agent` header
//! and reuses HTTP connections via a shared connection pool.

pub mod client;
pub mod error;
pub mod format;
pub mod query;
pub mod tools;
pub mod types;

pub use client::{ListParams, OpenAlexClient};
pub use error::OpenAlexError;
pub use tools::openalex_registrations;
pub use types::*;
