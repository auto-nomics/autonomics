//! Async Rust SDK for the [RCSB PDB](https://www.rcsb.org/) REST APIs.
//!
//! The Data, Search, and file services are public and require no API key.
//! This crate provides:
//!
//! - [`RcsbClient`] for entry/polymer/assembly metadata, search, and structure downloads.
//! - Arrow-emitting DAG source nodes under [`nodes`].
//! - Markdown-oriented agent tools under [`tools`].
//!
//! ```no_run
//! # use rcsb::{RcsbClient, search::SearchRequest};
//! # async fn run() -> rcsb::Result<()> {
//! let client = RcsbClient::new();
//! let entry = client.entry("4HHB").await?;
//! let hits = client.search(&SearchRequest::full_text("hemoglobin").rows(5)).await?;
//! let cif = client.structure_text("4HHB", rcsb::StructureFormat::Mmcif).await?;
//! # Ok(())
//! # }
//! ```

pub mod client;
pub mod error;
pub mod format;
pub mod nodes;
pub mod search;
pub mod tools;
pub mod types;

pub use client::{RcsbClient, StructureFormat};
pub use error::{RcsbError, Result};
pub use search::{
    LogicalOperator, SearchQuery, SearchRequest, SearchResponse, SearchResult, SearchReturnType,
};
pub use tools::rcsb_registrations;
