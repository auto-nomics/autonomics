//! External discovery contracts. Counts and cursors belong to individual
//! sources; a merged candidate count is never a global exact total.
use crate::{Article, SearchExpr, SearchField};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DiscoverySort {
    #[default]
    Relevance,
    Newest,
    Oldest,
    CitationCount,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DiscoveryFilters {
    pub year_from: Option<u16>,
    pub year_to: Option<u16>,
    pub publication_types: Vec<String>,
    pub open_access: Option<bool>,
}

/// Native mode is bound to one source. Parameters are allowlisted by its
/// planner; credentials, pagination and endpoint overrides are forbidden.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeSearch {
    pub source: String,
    pub parameters: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DiscoveryRequest {
    pub query: String,
    pub expression: Option<SearchExpr>,
    pub native: Option<NativeSearch>,
    pub sources: Vec<String>,
    pub filters: DiscoveryFilters,
    pub sort: DiscoverySort,
    /// Explicitly permit disclosed provider-specific fuzzy text matching.
    /// Exact identifiers, filters and exclusions must still be preserved.
    pub allow_approximate: bool,
    pub page_size: u16,
    /// Budget for an individual source operation, including retries.
    pub source_timeout_ms: u64,
    /// Budget for this page round across sources, not session lifetime.
    pub deadline_ms: u64,
}
impl Default for DiscoveryRequest {
    fn default() -> Self {
        Self {
            query: String::new(),
            expression: None,
            native: None,
            sources: vec!["pubmed".into(), "openalex".into()],
            filters: Default::default(),
            sort: Default::default(),
            allow_approximate: false,
            page_size: 25,
            source_timeout_ms: 20_000,
            deadline_ms: 30_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourcePagination {
    None,
    Offset,
    Cursor,
    /// Some source orderings require bounded offset paging, others use cursors.
    CursorOrOffset,
    History,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceCapabilities {
    pub source: String,
    pub searchable: bool,
    pub native_query: bool,
    pub fields: Vec<SearchField>,
    pub filters: Vec<String>,
    pub sorts: Vec<DiscoverySort>,
    pub pagination: SourcePagination,
    pub max_page_size: u16,
    pub credentials_configured: bool,
    #[serde(default)]
    pub authentication: SourceAuthentication,
    pub quota: Option<SourceQuota>,
    pub notes: Vec<String>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceAuthentication {
    None,
    Optional,
    Required,
    RequiredForContinuation,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceNotice {
    pub code: String,
    pub message: String,
}

/// Values reported by upstream headers, in its own budget units.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceQuota {
    pub limit: Option<f64>,
    pub remaining: Option<f64>,
    pub reset_unix_seconds: Option<u64>,
    pub observed_at_unix_seconds: u64,
    pub last_http_status: u16,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverySourceRuntime {
    pub source: String,
    pub occupied_slots: usize,
    pub concurrency_limit: usize,
    pub cooldown_remaining_ms: u64,
    pub blocked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourcePlan {
    pub source: String,
    pub page_size: u16,
    /// Public reproducible parameters, never credentials or arbitrary URLs.
    pub parameters: BTreeMap<String, String>,
    pub fingerprint: String,
    pub notices: Vec<SourceNotice>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceFailureKind {
    Unsupported,
    InvalidQuery,
    Timeout,
    Cancelled,
    AuthRequired,
    RateLimited,
    Transport,
    Upstream,
    InvalidResponse,
    CursorExpired,
    Capacity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceFailure {
    pub kind: SourceFailureKind,
    pub message: String,
    pub retryable: bool,
    pub retry_after_seconds: Option<u64>,
}
impl SourceFailure {
    pub fn new(kind: SourceFailureKind, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            kind,
            message: message.into(),
            retryable,
            retry_after_seconds: None,
        }
    }
}
impl std::fmt::Display for SourceFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for SourceFailure {}

/// Opaque cursor is internal to the session, not a user-editable offset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourcePage {
    pub source: String,
    pub articles: Vec<Article>,
    pub total: Option<u64>,
    pub next_cursor: Option<String>,
    pub executed_parameters: BTreeMap<String, String>,
    pub query_translation: Option<String>,
    pub notices: Vec<SourceNotice>,
}

/// One immutable upstream contribution, retained even when records merge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryRecord {
    pub source: String,
    pub article: Article,
    pub source_rank: usize,
    pub query_fingerprint: String,
    pub retrieved_at: String,
    #[serde(default)]
    pub executed_parameters: BTreeMap<String, String>,
    #[serde(default)]
    pub query_translation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldVariant {
    pub source: String,
    pub source_record_id: String,
    pub value: serde_json::Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldConflict {
    pub field: String,
    pub variants: Vec<FieldVariant>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateLink {
    pub candidate_id: String,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryCandidate {
    pub id: String,
    pub fingerprint: String,
    /// First contribution stays visible until the user selects a variant.
    pub article: Article,
    pub records: Vec<DiscoveryRecord>,
    pub conflicts: Vec<FieldConflict>,
    pub related_versions: Vec<CandidateLink>,
    pub possible_duplicates: Vec<String>,
    /// RRF over the obtained candidate ranks, never a global relevance score.
    pub rrf_score: f64,
    pub first_seen: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoverySourcePhase {
    Queued,
    Running,
    WaitingRetry,
    Ready,
    Exhausted,
    Failed,
    Cancelled,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoverySessionPhase {
    Running,
    Idle,
    Cancelled,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverySourceStatus {
    pub source: String,
    pub phase: DiscoverySourcePhase,
    pub plan: Option<SourcePlan>,
    pub failure: Option<SourceFailure>,
    pub total: Option<u64>,
    pub delivered_records: usize,
    pub pages: usize,
    pub has_more: bool,
    pub attempt: usize,
    pub retry_at: Option<String>,
    pub executed_parameters: BTreeMap<String, String>,
    pub query_translation: Option<String>,
    pub notices: Vec<SourceNotice>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverySessionSnapshot {
    pub id: String,
    pub request: DiscoveryRequest,
    pub created_at: String,
    pub expires_at: String,
    pub phase: DiscoverySessionPhase,
    pub sequence: u64,
    pub merged_candidates: usize,
    pub sources: Vec<DiscoverySourceStatus>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DiscoveryEventPayload {
    Started,
    SourceStarted {
        source: String,
        attempt: usize,
    },
    SourceRetrying {
        source: String,
        attempt: usize,
        failure: SourceFailure,
        retry_at: String,
    },
    Batch {
        source: String,
        upserted_ids: Vec<String>,
        removed_ids: Vec<String>,
        merged_candidates: usize,
    },
    SourceFailed {
        source: String,
        failure: SourceFailure,
    },
    SourceCancelled {
        source: String,
    },
    Completed {
        phase: DiscoverySessionPhase,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryEvent {
    pub sequence: u64,
    pub emitted_at: String,
    #[serde(flatten)]
    pub payload: DiscoveryEventPayload,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryEventPage {
    pub events: Vec<DiscoveryEvent>,
    pub sequence: u64,
    /// History eviction is explicit; clients must retrieve a fresh snapshot.
    pub reset_required: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryResultOrder {
    #[default]
    FirstSeen,
    Relevance,
    Newest,
    SourceRank,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryResultPage {
    pub candidates: Vec<DiscoveryCandidate>,
    pub obtained_candidates: usize,
    pub offset: usize,
    pub next_offset: Option<usize>,
    pub sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryImportSelection {
    pub candidate_id: String,
    pub expected_fingerprint: String,
    #[serde(default)]
    pub record_index: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct DiscoveryImportRequest {
    pub selections: Vec<DiscoveryImportSelection>,
    pub collection_id: Option<String>,
    pub library_id: Option<i64>,
    pub fetch_fulltext: bool,
}
/// Materialized from a checked session snapshot, retained after its TTL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryImportRecord {
    pub schema_version: u32,
    pub session_id: String,
    pub candidate_id: String,
    pub fingerprint: String,
    pub selected: DiscoveryRecord,
    pub records: Vec<DiscoveryRecord>,
    pub request: DiscoveryRequest,
    pub plans: Vec<SourcePlan>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryLibraryMatch {
    pub candidate_id: String,
    pub existing: Vec<Article>,
    pub related_existing: Vec<Article>,
    pub differences: Vec<FieldConflict>,
    pub error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryImportPreview {
    pub library_id: i64,
    pub collection_id: Option<String>,
    pub editable: bool,
    pub items: Vec<DiscoveryLibraryMatch>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryImportStatus {
    Imported,
    AlreadyPresent,
    Failed,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryImportOutcome {
    pub candidate_id: String,
    pub status: DiscoveryImportStatus,
    pub article: Option<Article>,
    pub related_article_ids: Vec<String>,
    pub collection_added: bool,
    pub changed: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryStoredProvenance {
    pub imported_at: String,
    pub provenance: DiscoveryImportRecord,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryProvenancePage {
    pub entries: Vec<DiscoveryStoredProvenance>,
    pub total: usize,
    pub next_offset: Option<usize>,
    pub related_article_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryAcquisitionStatus {
    Queued,
    Running,
    Available,
    Unavailable,
    Failed,
    Cancelled,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryAcquisitionItem {
    pub article_id: String,
    pub status: DiscoveryAcquisitionStatus,
    pub error: Option<String>,
}
/// Process-local acquisition progress; stored attachments remain durable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryAcquisitionJob {
    pub id: String,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub cancel_requested: bool,
    pub items: Vec<DiscoveryAcquisitionItem>,
}
