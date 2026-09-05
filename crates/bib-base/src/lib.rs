//! Bibliography management system: Turso storage + unified literature query.
//!
//! This crate provides two layers:
//!
//! - **Storage** ([`BibBase`]) — Turso (libSQL) CRUD for articles,
//!   collections, full-text records, and indexed lexical search. Depends only
//!   on `bib-types` + `turso`.
//!
//! - **Query** ([`query::LiteratureGateway`]) — unified entry point that
//!   searches PubMed and arXiv concurrently through a
//!   single [`query::LiteratureSource`] trait. Exposed as agent tools
//!   ([`lit_search`](tools::LitSearchTool), [`lit_fetch`](tools::LitFetchTool)).
//!
//! Depends on [`bib_types`] for the canonical data model.

pub mod annotations;
pub mod bib_base;
pub mod collections;
pub use collections::CollectionAddOutcome;
pub mod error;
pub mod export;
pub mod extract;
pub mod fulltext;
pub mod http_options;
pub mod import;
pub mod journal_metrics;
pub mod library_tools;
pub mod oa_fetch;
pub mod parse_hub;
pub mod query;
pub mod shared;
pub mod stored_files;
pub mod tools;

pub use bib_base::{BibBase, ListParams, SortField, SortOrder};
pub use error::{Error, Result};
pub use export::{cite_key, render, render_all, to_bibtex, to_csl_json, to_markdown, to_ris};
pub use extract::{ExtractedText, OcrFallbackExtractor, SimpleExtractor, TextExtractor};
pub use fulltext::{FullTextPage, FullTextParseStatus};
pub use parse_hub::{ParseEvent, ParseHub};
pub use http_options::BibHttpOptions;
pub use journal_metrics::{JournalMetrics, enrich_journal_metrics, journal_key_of};
pub use library_tools::bib_all_registrations;
pub use library_tools::bib_extended_registrations;
pub use oa_fetch::{try_fetch_fulltext, try_fetch_fulltext_with};
pub use query::{
    ArxivSource, BiorxivSource, CrossrefSource, LiteratureGateway, LiteratureSource,
    OpenAlexSource, PubmedSource, S2Source, SourceBatch,
};
pub use shared::BibShared;
pub use stored_files::{StoredFulltext, VFS_PREFIX, stored_fulltext, vfs_virtual_path};

/// Convenience: a [`LiteratureGateway`] pre-loaded with PubMed + arXiv + bioRxiv.
///
/// Re-exported so callers don't need to import the `query` module just to
/// get the default source set.
pub fn default_gateway() -> LiteratureGateway {
    LiteratureGateway::with_default_sources()
}
pub use tools::{LitFetchTool, LitSearchTool, bib_query_registrations};

// Re-export bib-types for convenience so downstream code can import
// types and storage from a single crate.
pub use bib_types::*;
