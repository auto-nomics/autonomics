//! Async Rust SDK for the [arXiv API](https://info.arxiv.org/help/api/index.html).
//!
//! This crate provides:
//!
//! - **`ArxivClient`** — an async HTTP client for the arXiv API.
//!   Supports searching papers by query, fetching by arXiv ID, and
//!   paging through results.
//! - **Agent tools** — pre-built [`ToolFunction`] implementations that expose
//!   arXiv search and fetch capabilities to an agentik agent.
//!
//! # Quick start (SDK only)
//!
//! ```no_run
//! use arxiv::{ArxivClient, types::SearchRequest};
//!
//! # async fn run() -> arxiv::error::Result<()> {
//! let client = ArxivClient::new();
//! let resp = client
//!     .search(&SearchRequest::new("au:Hinton AND ti:neural network"))
//!     .await?;
//! println!("{} results", resp.total_results);
//! for entry in &resp.entries {
//!     println!("  {} — {}", entry.arxiv_id, entry.title);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Wiring tools into an agent
//!
//! ```no_run,ignore
//! use arxiv::{ArxivClient, arxiv_registrations};
//! use std::sync::Arc;
//!
//! let client = Arc::new(ArxivClient::new());
//! let tools = arxiv_registrations(client);
//! // pass `tools` to Agent::builder().with_tools(tools)
//! ```
//!
//! # Rate limits
//!
//! arXiv recommends waiting at least 3 seconds between consecutive API calls.
//! The client enforces this automatically by sleeping before each request when
//! the previous request was less than 3 seconds ago.

pub mod client;
pub mod convert;
pub mod error;
pub mod format;
pub mod query;
pub mod tools;
pub mod types;

pub use client::ArxivClient;
pub use convert::atom_to_articles;
pub use error::ArxivError;
pub use tools::arxiv_registrations;
