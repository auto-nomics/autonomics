//! Core bibliographic data types for the `autonomics` workspace.
//!
//! This crate is intentionally free of I/O and external API dependencies.
//! It defines the canonical [`Article`] model and all related types
//! (authors, identifiers, collections, full-text metadata, search hits,
//! structured query) that are shared across:
//!
//! - `bib-base` — Turso storage layer
//! - `eutils` — PubMed SDK (uses [`Article`] in its conversion layer)
//! - `gwascatalog-sdk` — GWAS Catalog SDK (same)
//!
//! Keeping the types in a leaf crate prevents circular dependencies:
//! `eutils` can depend on `bib-types` without pulling in storage code.

pub mod convert;
pub mod query;
pub mod types;

pub use query::{BoolOp, StructuredSearch, YearRange};
pub use types::{
    AddedBy, Annotation, AnnotationKind, Article, ArticleRole, ArticleSource, Author, Collection,
    CollectionArticle, CollectionStatus, ExportFormat, FetchStatus, FileFormat, FullText,
    FullTextSource, IdKind, Identifier, LibraryEntry, Reference, SearchHit,
};
