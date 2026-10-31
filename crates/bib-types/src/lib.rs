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
pub mod discovery;
pub mod evidence;
pub mod query;
pub mod search;
pub mod search_parser;
pub use search_parser::{SearchParseError, parse_search_query};
pub mod types;
pub mod unified;
pub use unified::{
    LOCAL_SOURCE, UnifiedCandidate, UnifiedImportRequest, UnifiedImportResponse,
    UnifiedImportSelection, UnifiedSearchRequest, UnifiedSearchResponse, UnifiedSnippet,
    UnifiedSourceStatus,
};

pub use evidence::{EvidenceRecord, EvidenceSet};
pub use query::{BoolOp, StructuredSearch, YearRange};
pub use search::{
    SaveSearchInput, SavedSearch, SearchEvidence, SearchEvidencePage, SearchEvidenceRequest,
    SearchExpr, SearchFacetValue, SearchFacets, SearchField, SearchFilters, SearchFulltextStatus,
    SearchHighlight, SearchIndexStatus, SearchMode, SearchPageV2, SearchRecall, SearchRequestV2,
    SearchRetrieval, SearchScope, SearchSort, SemanticSearchEvidence,
};
pub use types::{
    AddedBy, AnchoredAnnotation, Annotation, AnnotationAnchorProof, AnnotationKind, AnnotationRect,
    AnnotationTextRange, AnnotationUpdate, Article, ArticleRole, ArticleSource, Author, Collection,
    CollectionArticle, CollectionStatus, ExportFormat, ExtractStatus, FetchStatus, FileFormat,
    FullText, FullTextSource, IdKind, Identifier, LibraryEntry, Reference, ResolvedAnchor,
    SearchHit, TextFormat,
};
