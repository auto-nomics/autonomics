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
//!   single [`query::LiteratureSource`] trait. Consumed by the DAG evidence
//!   nodes ([`nodes`]) — literature retrieval left the agent tool surface.
//!
//! Depends on [`bib_types`] for the canonical data model.

mod actor;
pub mod annotations;
pub mod bib_base;
mod collection_schema;
pub mod collections;
pub use collections::CollectionAddOutcome;
pub use collections::{CollectionAssignment, CollectionAssignmentOutcome};
pub mod error;
pub mod export;
pub mod extract;
pub mod extraction;
pub mod fulltext;
pub mod http_options;
mod legacy_index;
pub mod library_tools;
mod local_index;
pub mod managed_fulltext;
pub mod mineru;
pub mod nodes;
pub mod oa_fetch;
pub mod pmc_pdf;
pub mod query;
pub mod saved_searches;
pub mod search;
mod search_analyzer;
pub mod search_cancellation;
pub mod search_evidence;
mod search_filters;
mod search_projection;
pub mod shared;
pub mod stored_files;
pub mod zotero_extract;
pub mod zotero_model;
pub use zotero_extract::ZoteroExtractor;

pub use bib_base::{BibBase, SearchEngineMode};
pub use error::{Error, Result};
pub use export::{cite_key, render, render_all, to_bibtex, to_csl_json, to_markdown, to_ris};
pub use extract::{ExtractedImage, ExtractedText, SimpleExtractor, TextExtractor};
pub use extraction::{
    FallbackChainExtractor, backfill_extracted_objects, default_extractor, read_full,
    run_attachment_extraction, run_attachment_extraction_parts, run_extraction,
    run_extraction_parts, spawn_attachment_extraction, spawn_extraction, sweep_pending,
};
pub use fulltext::{FullTextPage, PendingAttachmentExtraction, PendingExtraction};
pub use http_options::BibHttpOptions;
pub use library_tools::bib_all_registrations;
pub use mineru::{
    DEFAULT_API_URL as MINERU_DEFAULT_API_URL, MineruExtractor, MineruZipOutput, markdown_from_zip,
};
pub use oa_fetch::fetch_fulltext_stored;
pub use query::{
    ArxivSource, BiorxivSource, CrossrefSource, LiteratureGateway, LiteratureSource,
    OpenAlexSource, PubmedSource, S2Source, SourceBatch,
};
pub use shared::BibShared;
pub use stored_files::{
    StoredFulltext, VFS_PREFIX, attachment_figure_object_path, extracted_attachment_object_path,
    extracted_object_path, figure_object_path, sanitize_figure_name, scan_image_refs,
    stored_attachment, stored_extracted_text, stored_fulltext, vfs_virtual_path,
};
pub use zotero_model::LINK_MODE_IMPORTED_FILE;
pub use zotero_model::{AttachmentTypeSummary, DeletedAttachment, NewAttachment, StoredAttachment};

/// Convenience: a [`LiteratureGateway`] pre-loaded with PubMed + arXiv + bioRxiv.
///
/// Re-exported so callers don't need to import the `query` module just to
/// get the default source set.
pub fn default_gateway() -> LiteratureGateway {
    LiteratureGateway::with_default_sources()
}

// Re-export bib-types for convenience so downstream code can import
// types and storage from a single crate.
pub use bib_types::*;
