//! Async Rust SDK for the [bioRxiv/medRxiv API](https://api.biorxiv.org/).
//!
//! Both `api.medrxiv.org` and `api.biorxiv.org` serve the same API; the
//! `{server}` path segment (`medrxiv` or `biorxiv`) selects the content pool.
//! This crate unifies both under a single client.
//!
//! This crate provides:
//!
//! - **`BiorxivClient`** — an async HTTP client supporting:
//!   - **Details by DOI** — fetch all versions of a manuscript.
//!   - **Details by date range** — browse papers posted in an interval.
//!   - **Details by recency** — most recent *N* papers or last *N* days.
//!   - **Auto-pagination** — [`BiorxivClient::details_all`] follows cursors.
//!   - **Pub mappings** — preprint DOI → published journal DOI.
//! - **Agent tools** — pre-built [`ToolFunction`] implementations that expose
//!   browsing and structured-search capabilities to an agentik agent.
//!
//! # Important limitation
//!
//! The bioRxiv/medRxiv API has **no keyword search**. The `biorxiv_search`
//! tool fetches by date range and applies keyword/title/author filters
//! client-side. For exhaustive browsing, use `biorxiv_details` with
//! `date_range`.
//!
//! # Quick start (SDK only)
//!
//! ```no_run
//! use biorxiv::{BiorxivClient, types::Server};
//! use chrono::NaiveDate;
//!
//! # async fn run() -> biorxiv::error::Result<()> {
//! let client = BiorxivClient::new();
//! let resp = client
//!     .details_by_date(
//!         Server::Medrxiv,
//!         NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
//!         NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(),
//!         0,
//!     )
//!     .await?;
//! println!("{} papers", resp.collection.len());
//! for entry in &resp.collection {
//!     println!("  {} — {}", entry.doi, entry.title);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Wiring tools into an agent
//!
//! ```no_run,ignore
//! use biorxiv::{BiorxivClient, biorxiv_registrations};
//! use std::sync::Arc;
//!
//! let client = Arc::new(BiorxivClient::new());
//! let tools = biorxiv_registrations(client);
//! // pass `tools` to Agent::builder().with_tools(tools)
//! ```

pub mod client;
pub mod convert;
pub mod error;
pub mod format;
pub mod query;
pub mod tools;
pub mod types;

pub use client::BiorxivClient;
pub use convert::entries_to_articles;
pub use error::BiorxivError;
pub use tools::biorxiv_registrations;
pub use types::Server;
