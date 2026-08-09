//! Async Rust SDK for the [Semantic Scholar Academic Graph API](
//! https://www.semanticscholar.org/product/api).
//!
//! Semantic Scholar provides access to 214M+ papers, 2.49B+ citations, and
//! 79M+ authors — including metadata, abstracts, citation graphs, TLDRs,
//! SPECTER2 embeddings, and paper recommendations.
//!
//! This crate provides:
//!
//! - **[`S2Client`]** — an async HTTP client supporting paper search, paper
//!   details, citations, references, author search, author details, and
//!   recommendations.
//! - **Agent tools** — pre-built [`ToolFunction`] implementations that expose
//!   Semantic Scholar capabilities to an agentik agent.
//!
//! # Quick start (SDK only)
//!
//! ```no_run
//! use semantic_scholar::S2Client;
//!
//! # async fn run() -> semantic_scholar::error::Result<()> {
//! let client = S2Client::new();
//! let resp = client.search_paper("covid vaccination", 10, None).await?;
//! println!("{} results", resp.total);
//! for paper in &resp.data {
//!     println!("  {} — {}", paper.paper_id, paper.title.as_deref().unwrap_or("(untitled)"));
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Authentication
//!
//! The API is usable without authentication (shared rate limit), but an API
//! key can be obtained from Semantic Scholar for higher limits. Pass it via
//! [`S2Client::with_api_key`]. The key is sent as the `x-api-key` header.
//!
//! # Wiring tools into an agent
//!
//! ```no_run,ignore
//! use semantic_scholar::{S2Client, s2_registrations};
//! use std::sync::Arc;
//!
//! let client = Arc::new(S2Client::new());
//! let tools = s2_registrations(client);
//! // pass `tools` to Agent::builder().with_tools(tools)
//! ```

pub mod client;
pub mod convert;
pub mod error;
pub mod format;
pub mod query;
pub mod tools;
pub mod types;

pub use client::S2Client;
pub use client::PaperSearchFilter;
pub use convert::paper_to_article;
pub use error::S2Error;
pub use tools::s2_registrations;
pub use types::{Author, Paper};
