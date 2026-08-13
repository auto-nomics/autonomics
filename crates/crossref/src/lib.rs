//! Async Rust SDK for the [Crossref REST API](https://www.crossref.org/documentation/retrieve-metadata/rest-api/).
//!
//! Crossref exposes the scholarly metadata deposited by 20 000+ members —
//! bibliographic records, funding data, license information, ORCID/ROR IDs,
//! abstracts, and citation counts for 150M+ DOIs.
//!
//! This crate provides:
//!
//! - **[`CrossrefClient`]** — an async HTTP client covering all REST API
//!   resource components: works, journals, members, funders, prefixes, types,
//!   and licenses.
//! - **Agent tools** — [`ToolFunction`] implementations that expose Crossref
//!   capabilities to an agentik agent. These return LLM-friendly Markdown
//!   (unstructured output).
//! - **DAG nodes** — `DagNode` source nodes that emit tabular DataFrames for
//!   structured-data pipelines (sink to CSV / Iceberg, join, filter, …).
//!
//! # Quick start (SDK only)
//!
//! ```no_run
//! # use crossref::CrossrefClient;
//! # async fn run() -> crossref::error::Result<()> {
//! let client = CrossrefClient::builder().build();
//! let resp = client.works_by_doi("10.1037/0003-066X.59.1.29").await?;
//! if let Some(work) = resp.message {
//!     println!("{}", work.title.first().unwrap_or(&"(untitled)".into()));
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Polite pool & rate limits
//!
//! The API is free and requires no registration. To get directed to the faster
//! "polite pool", include a contact email via [`CrossrefClientBuilder::mailto`]
//! (recommended). Rate limits are advertised via `X-Rate-Limit-*` headers.

pub mod client;
pub mod convert;
pub mod error;
pub mod format;
pub mod nodes;
pub mod query;
pub mod tools;
pub mod types;

pub use client::{CrossrefClient, CrossrefClientBuilder};
pub use convert::work_to_article;
pub use error::{CrossrefError, Result};
pub use tools::crossref_extended_registrations;
pub use tools::crossref_registrations;
