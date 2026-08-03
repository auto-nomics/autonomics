//! Bibliography management system: Turso storage + unified literature query.
//!
//! This crate provides two layers:
//!
//! - **Storage** ([`BibBase`]) — Turso (libSQL) CRUD for articles,
//!   collections, full-text records, and LIKE-based search. Depends only
//!   on `bib-types` + `turso`.
//!
//! - **Query** ([`query::LiteratureGateway`]) — unified entry point that
//!   searches PubMed and arXiv concurrently through a
//!   single [`query::LiteratureSource`] trait. Exposed as agent tools
//!   ([`lit_search`](tools::LitSearchTool), [`lit_fetch`](tools::LitFetchTool)).
//!
//! Depends on [`bib_types`] for the canonical data model.

pub mod bib_base;
pub mod collections;
pub mod error;
pub mod fulltext;
pub mod library_tools;
pub mod query;
pub mod tools;

pub use bib_base::BibBase;
pub use error::{Error, Result};
pub use library_tools::bib_all_registrations;
pub use query::{ArxivSource, LiteratureGateway, LiteratureSource, PubmedSource, SourceBatch};
pub use tools::{bib_query_registrations, LitFetchTool, LitSearchTool};

// Re-export bib-types for convenience so downstream code can import
// types and storage from a single crate.
pub use bib_types::*;
