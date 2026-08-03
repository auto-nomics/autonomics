//! Async Rust SDK for the [Europe PMC Articles RESTful API](
//! https://europepmc.org/RestfulWebService).
//!
//! Europe PMC provides access to over 48 million publications from PubMed,
//! Agricola, the European Patents Office, and more — including 10M+ full text
//! articles and 6.5M+ open access articles.
//!
//! This crate provides:
//!
//! - **`EuropePmcClient`** — an async HTTP client supporting search, article
//!   retrieval, citations, references, profile, and database links.
//! - **Agent tools** — pre-built [`ToolFunction`] implementations that expose
//!   Europe PMC capabilities to an agentik agent.
//!
//! # Quick start (SDK only)
//!
//! ```no_run
//! use europepmc::{EuropePmcClient, types::SearchRequest};
//!
//! # async fn run() -> europepmc::error::Result<()> {
//! let client = EuropePmcClient::new();
//! let resp = client
//!     .search(&SearchRequest::new("p53 AND TITLE:cancer"))
//!     .await?;
//! println!("{} results", resp.hit_count);
//! for result in &resp.result_list.results {
//!     println!("  {} — {}", result.id, result.title);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Wiring tools into an agent
//!
//! ```no_run,ignore
//! use europepmc::{EuropePmcClient, europepmc_registrations};
//! use std::sync::Arc;
//!
//! let client = Arc::new(EuropePmcClient::new());
//! let tools = europepmc_registrations(client);
//! // pass `tools` to Agent::builder().with_tools(tools)
//! ```
//!
//! # Rate limits
//!
//! The Europe PMC API is free and does not require an API key, but please be
//! considerate. The client sets a polite `User-Agent` header and reuses HTTP
//! connections via a shared connection pool.

pub mod client;
pub mod convert;
pub mod error;
pub mod format;
pub mod query;
pub mod tools;
pub mod types;

pub use client::EuropePmcClient;
pub use convert::results_to_articles;
pub use error::EuropePmcError;
pub use tools::europepmc_registrations;
