//! Unified retrieval: one query executed across the local library (V2
//! lexical/hybrid engine) and the external discovery session engine, merged
//! through the same conservative strong-identity merger the discovery
//! workspace uses. The local library participates as first-class records
//! under the `local` source name — a candidate that also exists externally
//! merges onto one entry carrying both provenances.
//!
//! v1 boundaries, all deliberate:
//! - Synchronous first screen: the call waits at most `deadline_ms`, then
//!   cancels still-running sources and keeps the batches already delivered.
//!   No unified cursor; narrow the query or raise the budget instead.
//! - `in_library` means "matched by this query against the local engine",
//!   which is not a full library-membership check for unmatched externals.
//! - Merged ordering is RRF over obtained ranks (relevance), a year sort,
//!   or first-seen; `citation_count` keeps per-source native order because
//!   citation scales are not comparable across sources.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use sha2::{Digest, Sha256};

use bib_types::Article;
use bib_types::discovery::DiscoveryImportStatus;
use bib_types::discovery::{
    DiscoveryCandidate, DiscoveryEventPayload, DiscoveryImportSelection, DiscoveryRecord,
    DiscoveryRequest, DiscoveryResultOrder, DiscoverySessionPhase, DiscoverySort,
    DiscoverySourcePhase, SourceFailure, SourceFailureKind,
};
use bib_types::search::{SearchFilters, SearchRequestV2, SearchSort};
use bib_types::unified::{
    LOCAL_SOURCE, UnifiedCandidate, UnifiedImportRequest, UnifiedImportResponse,
    UnifiedSearchRequest, UnifiedSearchResponse, UnifiedSnippet, UnifiedSourceStatus,
};

use crate::bib_base::BibBase;
use crate::discovery_merge::CandidateStore;
use crate::discovery_sessions::{DiscoverySessionError, DiscoverySessions};
use crate::query::LiteratureGateway;
use crate::shared::BibShared;
use crate::{Error, Result};

/// One source page is capped at 100 by the discovery contract; harvesting
/// pages in a loop until exhaustion, bounded so a runaway session cannot
/// balloon a single tool call.
const HARVEST_PAGE: usize = 100;
const HARVEST_CAP: usize = 1000;

/// Map discovery errors into the crate error surface, preserving the
/// distinction callers act on (invalid input vs. missing session).
fn session_err(error: DiscoverySessionError) -> Error {
    match error {
        DiscoverySessionError::Invalid(message) => Error::InvalidSearchQuery(message),
        DiscoverySessionError::NotFound => Error::NotFound("discovery session not found".into()),
        other => Error::Unknown(other.to_string()),
    }
}

fn local_sort(sort: DiscoverySort) -> SearchSort {
    match sort {
        DiscoverySort::Relevance => SearchSort::Relevance,
        DiscoverySort::Newest => SearchSort::Newest,
        DiscoverySort::Oldest => SearchSort::Oldest,
        DiscoverySort::CitationCount => SearchSort::CitationCount,
    }
}

struct LocalOutcome {
    records: Vec<DiscoveryRecord>,
    snippets: HashMap<String, UnifiedSnippet>,
    total: u64,
}

/// Query the local V2 engine and shape hits into discovery records so the
/// merger treats the library like any other source.
async fn run_local(
    bib: &Arc<BibBase>,
    request: &UnifiedSearchRequest,
    limit: usize,
) -> std::result::Result<LocalOutcome, SourceFailure> {
    if request.filters.open_access.is_some() {
        return Err(SourceFailure::new(
            SourceFailureKind::Unsupported,
            "local source cannot filter by open_access",
            false,
        ));
    }
    let v2 = SearchRequestV2 {
        query: request.query.clone(),
        expression: request.expression.clone(),
        default_fields: Vec::new(),
        scope: Default::default(),
        sort: local_sort(request.sort),
        filters: SearchFilters {
            year_from: request.filters.year_from,
            year_to: request.filters.year_to,
            publication_types: request.filters.publication_types.clone(),
            ..Default::default()
        },
        include_facets: false,
        include_articles: true,
        page_size: limit as u16,
        cursor: None,
        sources: Vec::new(),
        mode: request.mode,
        semantic_query: request.semantic_query.clone(),
    };
    let fingerprint = {
        let mut hasher = Sha256::new();
        hasher.update(serde_json::to_string(&v2).unwrap_or_default().as_bytes());
        format!("{:x}", hasher.finalize())
    };
    let page = bib.search_v2(v2).await.map_err(|error| {
        SourceFailure::new(
            SourceFailureKind::Upstream,
            format!("local search failed: {error}"),
            false,
        )
    })?;
    let by_id: HashMap<&str, &Article> = page
        .articles
        .iter()
        .map(|article| (article.id.as_str(), article))
        .collect();
    let retrieved_at = Utc::now().to_rfc3339();
    let mut records = Vec::with_capacity(page.hits.len());
    let mut snippets = HashMap::with_capacity(page.hits.len());
    for (rank, hit) in page.hits.iter().enumerate() {
        let article = by_id.get(hit.article_id.as_str()).map_or(
            // Hydration should never miss a page hit; fall back to the
            // hit's own title rather than dropping a local match.
            Article::new(hit.article_id.clone(), hit.title.clone()),
            |article| (*article).clone(),
        );
        snippets
            .entry(hit.article_id.clone())
            .or_insert_with(|| UnifiedSnippet {
                article_id: hit.article_id.clone(),
                snippet: hit.snippet.clone(),
                highlight_ranges: hit.highlight_ranges.clone(),
                matched_field: hit.matched_field.clone(),
                anchor_key: hit.anchor_key.clone(),
            });
        records.push(DiscoveryRecord {
            source: LOCAL_SOURCE.to_string(),
            article,
            source_rank: rank + 1,
            query_fingerprint: fingerprint.clone(),
            retrieved_at: retrieved_at.clone(),
            executed_parameters: [
                ("engine".to_string(), "tantivy-v2".to_string()),
                ("page_size".to_string(), limit.to_string()),
            ]
            .into_iter()
            .collect(),
            query_translation: None,
        });
    }
    Ok(LocalOutcome {
        records,
        snippets,
        total: page.total as u64,
    })
}

/// Wait until every session source reaches a terminal phase, the completion
/// event lands, or the wall-clock budget expires. Returns whether all
/// sources settled on their own (true) or the budget cut them off (false).
async fn wait_for_sources(
    sessions: &DiscoverySessions,
    id: &str,
    budget: Duration,
) -> Result<bool> {
    let start = Instant::now();
    let mut after = 0u64;
    loop {
        let remaining = budget.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            return Ok(false);
        }
        let page = sessions
            .events_after(id, after, remaining.min(Duration::from_millis(500)))
            .await
            .map_err(session_err)?;
        let completed = page
            .events
            .iter()
            .any(|event| matches!(event.payload, DiscoveryEventPayload::Completed { .. }));
        if let Some(last) = page.events.last() {
            after = after.max(last.sequence);
        }
        if completed {
            return Ok(true);
        }
        match sessions.snapshot(id).map_err(session_err)?.phase {
            DiscoverySessionPhase::Running => {}
            DiscoverySessionPhase::Idle => return Ok(true),
            DiscoverySessionPhase::Cancelled => return Ok(false),
        }
    }
}

/// Pull every merged candidate the session produced, paging within the
/// session's 100-per-page contract, plus the session's terminal phase.
fn harvest(
    sessions: &DiscoverySessions,
    id: &str,
) -> Result<(Vec<DiscoveryCandidate>, DiscoverySessionPhase)> {
    let mut candidates = Vec::new();
    let mut offset = 0usize;
    loop {
        let page = sessions
            .results(
                id,
                None,
                DiscoveryResultOrder::FirstSeen,
                offset,
                HARVEST_PAGE,
                None,
            )
            .map_err(session_err)?;
        let produced = page.candidates.len();
        candidates.extend(page.candidates);
        offset += produced;
        if page.next_offset.is_none() || produced == 0 || offset >= HARVEST_CAP {
            break;
        }
    }
    candidates.truncate(HARVEST_CAP);
    let phase = sessions.snapshot(id).map_err(session_err)?.phase;
    Ok((candidates, phase))
}

/// Best year across a candidate's records for newest/oldest ordering;
/// candidates without any year sort last.
fn best_year(candidate: &DiscoveryCandidate) -> Option<u16> {
    candidate
        .records
        .iter()
        .filter_map(|record| record.article.year)
        .max()
}

fn order_by_year(
    mut candidates: Vec<DiscoveryCandidate>,
    ascending: bool,
    limit: usize,
) -> Vec<DiscoveryCandidate> {
    if ascending {
        candidates.sort_by_key(|candidate| {
            (
                best_year(candidate).unwrap_or(u16::MAX),
                candidate.first_seen,
            )
        });
    } else {
        candidates.sort_by_key(|candidate| {
            (
                std::cmp::Reverse(best_year(candidate)),
                candidate.first_seen,
            )
        });
    }
    candidates.truncate(limit);
    candidates
}

/// Execute a unified search. See the module docs for semantics; nothing
/// here mutates library state, and the external session (when one runs) is
/// left alive so [`unified_import`] can act on its candidates until TTL.
pub async fn unified_search(
    bib: &Arc<BibBase>,
    sessions: &DiscoverySessions,
    gateway: &Arc<LiteratureGateway>,
    request: UnifiedSearchRequest,
) -> Result<UnifiedSearchResponse> {
    let has_query = !request.query.trim().is_empty();
    let has_expression = request.expression.is_some();
    if has_query == has_expression {
        return Err(Error::InvalidSearchQuery(
            "supply exactly one of query or expression".into(),
        ));
    }
    let limit = usize::from(request.limit.clamp(1, 100));

    let mut sources = request.sources.clone();
    if sources.is_empty() {
        sources = vec![
            LOCAL_SOURCE.to_string(),
            "pubmed".to_string(),
            "openalex".to_string(),
        ];
    }
    let want_local = sources.iter().any(|name| name == LOCAL_SOURCE);
    let externals: Vec<String> = sources
        .into_iter()
        .filter(|name| name != LOCAL_SOURCE)
        .collect();

    let mut statuses: Vec<UnifiedSourceStatus> = Vec::new();
    let mut local_snippets: HashMap<String, UnifiedSnippet> = HashMap::new();
    let mut records: Vec<DiscoveryRecord> = Vec::new();

    if want_local {
        match run_local(bib, &request, limit).await {
            Ok(outcome) => {
                statuses.push(UnifiedSourceStatus {
                    source: LOCAL_SOURCE.to_string(),
                    local: true,
                    phase: DiscoverySourcePhase::Ready,
                    total: Some(outcome.total),
                    delivered_records: outcome.records.len(),
                    failure: None,
                });
                local_snippets = outcome.snippets;
                records.extend(outcome.records);
            }
            Err(failure) => {
                // A failed local source is terminal: `completed` stays true,
                // the failure is visible in its status.
                statuses.push(UnifiedSourceStatus {
                    source: LOCAL_SOURCE.to_string(),
                    local: true,
                    phase: DiscoverySourcePhase::Failed,
                    total: None,
                    delivered_records: 0,
                    failure: Some(failure),
                });
            }
        }
    }

    let mut session_id: Option<String> = None;
    let mut completed = true;
    // Session candidate lookup by contributing record, so merged unified
    // candidates can be imported through the session they came from.
    let mut session_lookup: HashMap<(String, String), (String, String)> = HashMap::new();
    if !externals.is_empty() {
        let discovery_request = DiscoveryRequest {
            query: request.query.clone(),
            expression: request.expression.clone(),
            native: None,
            sources: externals,
            filters: request.filters.clone(),
            sort: request.sort,
            allow_approximate: false,
            page_size: limit as u16,
            source_timeout_ms: request.source_timeout_ms,
            deadline_ms: request.deadline_ms,
        };
        let created = sessions
            .create(gateway.clone(), discovery_request)
            .map_err(session_err)?;
        let id = created.id.clone();
        let budget = Duration::from_millis(request.deadline_ms.clamp(100, 120_000));
        let settled = wait_for_sources(sessions, &id, budget).await?;
        if !settled {
            // Budget expired: stop the stragglers; delivered batches stay.
            let _ = sessions.cancel(&id, None);
        }
        let (session_candidates, phase) = harvest(sessions, &id)?;
        completed = settled && phase == DiscoverySessionPhase::Idle;
        for status in sessions.snapshot(&id).map_err(session_err)?.sources {
            statuses.push(UnifiedSourceStatus {
                source: status.source,
                local: false,
                phase: status.phase,
                total: status.total,
                delivered_records: status.delivered_records,
                failure: status.failure,
            });
        }
        for candidate in session_candidates {
            for record in &candidate.records {
                session_lookup.insert(
                    (record.source.clone(), record.article.id.clone()),
                    (candidate.id.clone(), candidate.fingerprint.clone()),
                );
            }
            records.extend(candidate.records);
        }
        session_id = Some(id);
    }

    let mut store = CandidateStore::default();
    store.ingest(records);

    let ordered = match request.sort {
        DiscoverySort::Relevance => {
            let (ranked, _) = store.results(None, DiscoveryResultOrder::Relevance, 0, HARVEST_CAP);
            let mut ranked = ranked;
            ranked.truncate(limit);
            ranked
        }
        DiscoverySort::Newest => {
            let (first_seen, _) =
                store.results(None, DiscoveryResultOrder::FirstSeen, 0, HARVEST_CAP);
            order_by_year(first_seen, false, limit)
        }
        DiscoverySort::Oldest => {
            let (first_seen, _) =
                store.results(None, DiscoveryResultOrder::FirstSeen, 0, HARVEST_CAP);
            order_by_year(first_seen, true, limit)
        }
        // Native per-source order, first-seen merge, local metadata first.
        DiscoverySort::CitationCount => {
            let (first_seen, _) =
                store.results(None, DiscoveryResultOrder::FirstSeen, 0, HARVEST_CAP);
            let mut ordered = first_seen;
            ordered.truncate(limit);
            ordered
        }
    };
    let total = store.len();

    let candidates: Vec<UnifiedCandidate> = ordered
        .into_iter()
        .map(|candidate| {
            let local_records: Vec<&DiscoveryRecord> = candidate
                .records
                .iter()
                .filter(|record| record.source == LOCAL_SOURCE)
                .collect();
            let in_library = !local_records.is_empty();
            let library_article_id = local_records
                .first()
                .map(|record| record.article.id.clone());
            let snippets = local_records
                .iter()
                .filter_map(|record| local_snippets.get(&record.article.id).cloned())
                .collect();
            // Import addressing: the first non-local record's session
            // candidate. All non-local records of one merged candidate come
            // from the same session candidate (the session already merged
            // external duplicates before we did).
            let import = candidate
                .records
                .iter()
                .find(|record| record.source != LOCAL_SOURCE)
                .and_then(|record| {
                    session_lookup.get(&(record.source.clone(), record.article.id.clone()))
                });
            let (import_candidate_id, import_fingerprint) = match import {
                Some((id, fingerprint)) => (Some(id.clone()), Some(fingerprint.clone())),
                None => (None, None),
            };
            UnifiedCandidate {
                candidate,
                in_library,
                library_article_id,
                snippets,
                import_candidate_id,
                import_fingerprint,
            }
        })
        .collect();

    // Every requested source failed with a non-cancellation reason: that is
    // an error, never a silent zero. Sources our own budget cancelled are
    // reported through `completed=false` and their statuses instead.
    let failures = statuses
        .iter()
        .filter(|status| {
            status.failure.as_ref().is_some_and(|failure| {
                failure.kind != bib_types::discovery::SourceFailureKind::Cancelled
            })
        })
        .map(|status| {
            format!(
                "{}: {}",
                status.source,
                status
                    .failure
                    .as_ref()
                    .map(|failure| failure.message.clone())
                    .unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    if !statuses.is_empty() && failures.len() == statuses.len() && total == 0 {
        return Err(Error::Unknown(format!(
            "all requested sources failed: {}",
            failures.join("; ")
        )));
    }

    Ok(UnifiedSearchResponse {
        session_id,
        obtained_candidates: total,
        completed,
        candidates,
        sources: statuses,
    })
}

/// Import (or preview) candidates from a session a prior
/// [`unified_search`] created. Thin wrapper over the discovery import
/// path, so provenance, idempotency and full-text acquisition behave
/// exactly like the discovery workspace.
pub async fn unified_import(
    shared: &Arc<BibShared>,
    request: UnifiedImportRequest,
) -> Result<UnifiedImportResponse> {
    if request.selections.is_empty() {
        return Err(Error::InvalidSearchQuery(
            "select at least one candidate to import".into(),
        ));
    }
    if request.fetch_fulltext && shared.file_storage.is_none() {
        return Err(Error::Unknown(
            "full-text acquisition requires configured file storage".into(),
        ));
    }
    let selections: Vec<DiscoveryImportSelection> = request
        .selections
        .iter()
        .map(|selection| DiscoveryImportSelection {
            candidate_id: selection.candidate_id.clone(),
            expected_fingerprint: selection.expected_fingerprint.clone(),
            record_index: selection.record_index,
        })
        .collect();
    let records = shared
        .discovery_sessions
        .import_records(&request.session_id, &selections)
        .map_err(session_err)?;
    if request.dry_run {
        let preview = shared
            .bib
            .preview_discovery_import(records, request.library_id, request.collection_id.clone())
            .await?;
        return Ok(UnifiedImportResponse {
            preview: Some(preview),
            outcomes: Vec::new(),
            imported: 0,
            failed: 0,
            acquisition: None,
        });
    }
    let outcomes = shared
        .bib
        .import_discovery(records, request.library_id, request.collection_id.clone())
        .await?;
    let imported = outcomes
        .iter()
        .filter(|item| item.status == DiscoveryImportStatus::Imported)
        .count();
    let failed = outcomes
        .iter()
        .filter(|item| item.status == DiscoveryImportStatus::Failed)
        .count();
    let articles: Vec<Article> = outcomes
        .iter()
        .filter_map(|item| item.article.clone())
        .collect();
    let acquisition = if request.fetch_fulltext && !articles.is_empty() {
        let reservation = shared
            .discovery_acquisitions
            .reserve()
            .map_err(session_err)?;
        Some(
            reservation
                .start(shared.as_ref().clone(), articles)
                .map_err(session_err)?,
        )
    } else {
        None
    };
    Ok(UnifiedImportResponse {
        preview: None,
        outcomes,
        imported,
        failed,
        acquisition,
    })
}

impl BibShared {
    /// Unified local + external search on the shared handles; see
    /// [`unified_search`].
    pub async fn unified_search(
        &self,
        request: UnifiedSearchRequest,
    ) -> Result<UnifiedSearchResponse> {
        unified_search(&self.bib, &self.discovery_sessions, &self.gateway, request).await
    }

    /// Import candidates from a prior unified search session; see
    /// [`unified_import`].
    pub async fn unified_import(
        &self,
        request: UnifiedImportRequest,
    ) -> Result<UnifiedImportResponse> {
        let shared = Arc::new(self.clone());
        unified_import(&shared, request).await
    }
}
