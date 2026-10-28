//! Unified retrieval contracts: one query executed across the local library
//! and external discovery sources, merged into a single ranked candidate
//! list. Counts remain per source; the merged count is the number of
//! distinct candidates obtained, never a global exact total.

use crate::SearchExpr;
use crate::discovery::{
    DiscoveryAcquisitionJob, DiscoveryCandidate, DiscoveryFilters, DiscoveryImportOutcome,
    DiscoveryImportPreview, DiscoverySort, DiscoverySourcePhase, SourceFailure,
};
use crate::search::{SearchHighlight, SearchMode};
use serde::{Deserialize, Serialize};

/// Source name reserved for the local library inside a unified request.
pub const LOCAL_SOURCE: &str = "local";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UnifiedSearchRequest {
    /// Query-language input (`title:foo "exact phrase" AND NOT year:2020`).
    /// Supply either `query` or `expression`, not both.
    pub query: String,
    /// Prebuilt V2 AST. External planners translate the same AST; the local
    /// engine executes it directly. Wins over `query` when both are set is
    /// an error, matching discovery normalization.
    pub expression: Option<SearchExpr>,
    /// `local` plus external source names (e.g. `pubmed`, `openalex`,
    /// `arxiv`, `crossref`, `semantic_scholar`). Empty means
    /// `[local, pubmed, openalex]`.
    pub sources: Vec<String>,
    pub filters: DiscoveryFilters,
    pub sort: DiscoverySort,
    /// Local recall mode only; external sources always run their native
    /// ranking. `hybrid` requires the semantic service to be configured.
    pub mode: SearchMode,
    /// Topic text for local hybrid recall, independent of the hard query.
    pub semantic_query: Option<String>,
    /// Candidates returned after merging, 1–100.
    pub limit: u16,
    /// Budget for one external source operation including retries
    /// (100–120000 ms).
    pub source_timeout_ms: u64,
    /// Wall-clock budget for the whole call (100–120000 ms). Sources still
    /// running when it expires are cancelled and their partial batches, if
    /// any, are kept.
    pub deadline_ms: u64,
}
impl Default for UnifiedSearchRequest {
    fn default() -> Self {
        Self {
            query: String::new(),
            expression: None,
            sources: Vec::new(),
            filters: DiscoveryFilters::default(),
            sort: DiscoverySort::default(),
            mode: SearchMode::default(),
            semantic_query: None,
            limit: 25,
            source_timeout_ms: 20_000,
            deadline_ms: 30_000,
        }
    }
}

/// One local match snippet attached to a merged candidate. Only `local`
/// records produce snippets; external candidates expose their abstract on
/// the article itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnifiedSnippet {
    pub article_id: String,
    pub snippet: String,
    pub highlight_ranges: Vec<SearchHighlight>,
    pub matched_field: Option<String>,
    /// Zotero item key of the article, attachment, or annotation that
    /// matched.
    pub anchor_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedCandidate {
    pub candidate: DiscoveryCandidate,
    /// True when the candidate merged with a local library hit for this
    /// query. False means "not matched by this query", which is not a
    /// full library-membership check.
    pub in_library: bool,
    /// Local article id when `in_library` is true.
    pub library_article_id: Option<String>,
    /// Local snippets, ordered by local rank.
    pub snippets: Vec<UnifiedSnippet>,
    /// Session candidate id for `literature_import`. The merged candidate
    /// in this response has its own id; imports address the session's
    /// candidate, so this is the id (with `import_fingerprint`) to pass.
    /// `None` for local-only candidates — nothing to import.
    #[serde(default)]
    pub import_candidate_id: Option<String>,
    /// Fingerprint matching [`UnifiedCandidate::import_candidate_id`].
    #[serde(default)]
    pub import_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedSourceStatus {
    pub source: String,
    pub local: bool,
    pub phase: DiscoverySourcePhase,
    pub total: Option<u64>,
    pub delivered_records: usize,
    pub failure: Option<SourceFailure>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedSearchResponse {
    /// Present when external sources ran. Keep it to import candidates via
    /// `literature_import`; the session is reclaimed by TTL.
    pub session_id: Option<String>,
    pub candidates: Vec<UnifiedCandidate>,
    /// Distinct merged candidates obtained from the sources that reported
    /// in time — not a global total.
    pub obtained_candidates: usize,
    /// True when every requested source reached a terminal phase within
    /// the budget. False means the deadline expired and running sources
    /// were cancelled; failures are still visible per source.
    pub completed: bool,
    pub sources: Vec<UnifiedSourceStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnifiedImportSelection {
    pub candidate_id: String,
    /// Fingerprint from the search response; a mismatch rejects the write.
    pub expected_fingerprint: String,
    /// Record index within the candidate, selecting a specific source
    /// version.
    #[serde(default)]
    pub record_index: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UnifiedImportRequest {
    pub session_id: String,
    pub selections: Vec<UnifiedImportSelection>,
    pub collection_id: Option<String>,
    pub library_id: Option<i64>,
    pub fetch_fulltext: bool,
    /// Preview matches without writing.
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedImportResponse {
    /// Present (and the only field that matters) for a dry run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<DiscoveryImportPreview>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outcomes: Vec<DiscoveryImportOutcome>,
    pub imported: usize,
    pub failed: usize,
    /// Background full-text acquisition job, when requested and articles
    /// were imported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acquisition: Option<DiscoveryAcquisitionJob>,
}
