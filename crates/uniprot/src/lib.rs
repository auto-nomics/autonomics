//! Async Rust SDK for the [UniProt REST API](https://www.uniprot.org/help/programmatic_access).
//!
//! UniProtKB is the central resource for protein sequence and annotation
//! data (~600M entries: reviewed Swiss-Prot + unreviewed TrEMBL), alongside
//! its taxonomy and reference-proteome services and the cross-database ID
//! mapping service.
//!
//! This crate provides:
//!
//! - **[`UniProtClient`]** — an async HTTP client covering UniProtKB
//!   search/stream/entry, taxonomy, proteomes, and ID mapping.
//! - **[`query::Query`]** — a typed builder for UniProtKB query
//!   expressions (`gene:INS AND organism_id:9606 AND reviewed:true`).
//! - **`format`** — Markdown rendering of responses for LLM consumption.
//! - **Agent tools** — pre-built `ToolRegistration`s via
//!   [`uniprot_registrations`].
//!
//! # Quick start (SDK only)
//!
//! ```no_run
//! use uniprot::{UniProtClient, query::Query, types::SearchRequest};
//!
//! # async fn run() -> uniprot::error::Result<()> {
//! let client = UniProtClient::new();
//! let query = Query::new().gene(["INS"]).organism_id(9606).reviewed(true).build()?;
//! let page = client.search(&SearchRequest::new(query).size(5)).await?;
//! for entry in &page.results {
//!     println!("{} — {}", entry.primary_accession, entry.protein_name().unwrap_or("?"));
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Bulk downloads (DAG source nodes)
//!
//! The `/search` endpoints cap pages at 500 entries; for bulk data use
//! [`UniProtClient::stream`], which returns raw TSV/FASTA text ready to
//! write to disk:
//!
//! ```no_run
//! # use uniprot::{UniProtClient, types::Format};
//! # async fn run() -> uniprot::error::Result<()> {
//! let tsv = UniProtClient::new()
//!     .stream("organism_id:9606 AND reviewed:true",
//!             Some(&["accession".into(), "id".into(), "protein_name".into()]),
//!             Format::Tsv)
//!     .await?;
//! let fasta = UniProtClient::new().fasta(&["P01308"]).await?;
//! # Ok(())
//! # }
//! ```
//!
//! Typed [`types::Entry`] values (JSON) suit interactive use and table
//! conversion; raw stream output suits archival and pipelines.
//!
//! # Pagination
//!
//! `/search` endpoints use cursor pagination: read
//! [`types::SearchResults::next_cursor`] from a response and pass it back
//! via [`types::SearchRequest::cursor`], or let
//! [`UniProtClient::search_all`] follow the chain for you.
//!
//! # Rate limits
//!
//! The API is free without a key; UniProt asks for ≤10 requests/second and
//! prefers streaming over paged search for bulk retrieval. The client sets
//! a polite `User-Agent` and reuses one connection pool.
//!
//! # Wiring tools into an agent
//!
//! ```no_run,ignore
//! use uniprot::{UniProtClient, uniprot_registrations};
//! use std::sync::Arc;
//!
//! let client = Arc::new(UniProtClient::new());
//! let tools = uniprot_registrations(client);
//! // pass `tools` to Agent::builder().with_tools(tools)
//! ```

pub mod client;
pub mod error;
pub mod format;
pub mod query;
pub mod tools;
pub mod types;

pub use client::UniProtClient;
pub use error::UniProtError;
pub use tools::uniprot_registrations;
