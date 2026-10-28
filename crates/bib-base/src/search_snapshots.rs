//! P5 immutable execution captures. Storage is independent of live search rows;
//! callers capture actual library pages or one consistent discovery sequence.
//! Shared by BibShared and the authenticated capture/list/export API.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use bib_types::{Article, SearchMode, SearchPageV2, SearchRequestV2, discovery::*};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, io::Write, sync::Arc};
use tokio::sync::Mutex;
use turso::{
    Connection,
    transaction::{Transaction, TransactionBehavior},
};

#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error("invalid snapshot: {0}")]
    Invalid(String),
    #[error("snapshot conflict: {0}")]
    Conflict(String),
    #[error("snapshot execution unavailable: {0}")]
    Unavailable(String),
    #[error("snapshot budget exceeded: {0}")]
    Budget(String),
    #[error("snapshot corrupt: {0}")]
    Corrupt(String),
    #[error(transparent)]
    Store(#[from] turso::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
type Result<T> = std::result::Result<T, SnapshotError>;
fn invalid(message: &str) -> SnapshotError {
    SnapshotError::Invalid(message.into())
}
pub(crate) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn digest_valid(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SnapshotLimits {
    pub max_snapshots: usize,
    pub max_snapshot_bytes: usize,
    pub max_total_bytes: u64,
}
impl Default for SnapshotLimits {
    fn default() -> Self {
        Self {
            max_snapshots: 1000,
            max_snapshot_bytes: 8 * 1024 * 1024,
            max_total_bytes: 256 * 1024 * 1024,
        }
    }
}
impl SnapshotLimits {
    pub fn validate(&self) -> Result<()> {
        if !(1..=1_000_000).contains(&self.max_snapshots)
            || !(1..=64 * 1024 * 1024).contains(&self.max_snapshot_bytes)
            || !(1..=1024u64.pow(4)).contains(&self.max_total_bytes)
        {
            return Err(invalid("limits outside supported bounds"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotModel {
    pub provider: String,
    pub origin: String,
    pub name: String,
    pub digest: String,
    pub signature: String,
    pub profile: serde_json::Value,
    pub inference: serde_json::Value,
    pub semantic_generation: String,
    pub configuration_generation: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotRuntime {
    pub format_version: u16,
    pub query_contract_version: u16,
    pub application_version: String,
    pub engine_fingerprint: String,
    pub model: Option<SnapshotModel>,
}
impl SnapshotRuntime {
    pub fn current(model: Option<SnapshotModel>) -> Self {
        let mut h = Sha256::new();
        for source in [
            include_bytes!("search.rs").as_slice(),
            include_bytes!("hybrid_search.rs"),
            include_bytes!("local_index.rs"),
            include_bytes!("search_analyzer.rs"),
            include_bytes!("search_filters.rs"),
            include_bytes!("../../../Cargo.lock"),
            include_bytes!("../Cargo.toml"),
        ] {
            h.update(source);
        }
        Self {
            format_version: 1,
            query_contract_version: 2,
            application_version: env!("CARGO_PKG_VERSION").into(),
            engine_fingerprint: format!("{:x}", h.finalize()),
            model,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureTermination {
    End,
    Budget,
    Cancelled,
    Failure,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapturedLibraryPage {
    pub request: SearchRequestV2,
    pub response: SearchPageV2,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SnapshotData {
    Library {
        pages: Vec<CapturedLibraryPage>,
        requested_max_items: usize,
        termination: CaptureTermination,
    },
    Discovery {
        session: DiscoverySessionSnapshot,
        results: DiscoveryResultPage,
        order: DiscoveryResultOrder,
        requested_max_items: usize,
        termination: CaptureTermination,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotCapture {
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub runtime: SnapshotRuntime,
    pub data: SnapshotData,
}

/// Server-owned execution intent. No caller-supplied result rows are accepted.
pub struct LibraryCaptureCommand {
    pub key: String,
    pub name: String,
    pub request: SearchRequestV2,
    pub max_items: usize,
}

/// Capture actual pages before atomically publishing a snapshot. Both callbacks
/// run only after checking for an already committed action. A query/generation
/// failure leaves no partial record; a lost commit response is safe to retry.
pub async fn capture_library<P, PF, E, EF>(
    store: &SnapshotStore,
    command: LibraryCaptureCommand,
    prepare_runtime: P,
    mut execute: E,
) -> Result<StoredSnapshot>
where
    P: FnOnce() -> PF,
    PF: std::future::Future<Output = Result<SnapshotRuntime>>,
    E: FnMut(SearchRequestV2) -> EF,
    EF: std::future::Future<Output = Result<SearchPageV2>>,
{
    validate_bound(command.max_items, 0)?;
    let mut request = command.request;
    if request.cursor.is_some() || !(1..=200).contains(&request.page_size) {
        return Err(invalid(
            "capture must start at the first page with a valid page size",
        ));
    }
    request.include_articles = true;
    request.page_size = request.page_size.min(command.max_items as u16);
    let intent = SnapshotIntent::Library {
        request: request.clone(),
        requested_max_items: command.max_items,
    };
    if let Some(saved) = store
        .find_for_intent(&command.key, &command.name, &intent)
        .await?
    {
        return Ok(saved);
    }
    let limits = store.limits().await?;
    let started_at = Utc::now();
    let runtime = prepare_runtime().await?;
    let mut capture = SnapshotCapture {
        started_at,
        finished_at: Utc::now(),
        runtime,
        data: SnapshotData::Library {
            pages: Vec::new(),
            requested_max_items: command.max_items,
            termination: CaptureTermination::Budget,
        },
    };
    let mut count = 0;
    loop {
        let response = execute(request.clone()).await?;
        count += response.hits.len();
        let total = response.total;
        let next = response.next_cursor.clone();
        let SnapshotData::Library {
            pages, termination, ..
        } = &mut capture.data
        else {
            unreachable!()
        };
        pages.push(CapturedLibraryPage {
            request: request.clone(),
            response,
        });
        if next.is_none() {
            *termination = CaptureTermination::End;
        }
        capture.finished_at = Utc::now();
        // Reject malformed, non-progressing or mixed-generation pages before
        // another query; bound the retained serialized capture after each page.
        capture.coverage()?;
        bounded_json(&capture, limits.max_snapshot_bytes)?;
        let next_page_items = (request.page_size as usize).min(total.saturating_sub(count));
        if next.is_none() || count.saturating_add(next_page_items) > command.max_items {
            break;
        }
        request.cursor = next;
    }
    store.save(&command.key, &command.name, capture).await
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotKind {
    Library,
    Discovery,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotCoverage {
    pub result_basis: String,
    /// Complete relative to the recorded match/candidate set, never the web.
    pub complete_obtained_set: bool,
    pub sources_exhausted: Option<bool>,
    pub captured_items: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SnapshotIntent {
    Library {
        request: SearchRequestV2,
        requested_max_items: usize,
    },
    Discovery {
        session_id: String,
        sequence: u64,
        request: DiscoveryRequest,
        order: DiscoveryResultOrder,
        requested_max_items: usize,
    },
}
impl SnapshotIntent {
    fn bytes(&self, name: &str) -> Result<Vec<u8>> {
        bounded_json(&(name, self), 128 * 1024)
    }
}
impl SnapshotCapture {
    pub fn intent_spec(&self) -> Result<SnapshotIntent> {
        Ok(match &self.data {
            SnapshotData::Library {
                pages,
                requested_max_items,
                ..
            } => SnapshotIntent::Library {
                request: pages
                    .first()
                    .ok_or_else(|| invalid("no captured page"))?
                    .request
                    .clone(),
                requested_max_items: *requested_max_items,
            },
            SnapshotData::Discovery {
                session,
                order,
                requested_max_items,
                ..
            } => SnapshotIntent::Discovery {
                session_id: session.id.clone(),
                sequence: session.sequence,
                request: session.request.clone(),
                order: *order,
                requested_max_items: *requested_max_items,
            },
        })
    }
    fn intent(&self, name: &str) -> Result<Vec<u8>> {
        self.intent_spec()?.bytes(name)
    }
    pub fn articles(&self) -> Vec<Article> {
        match &self.data {
            SnapshotData::Library { pages, .. } => pages
                .iter()
                .flat_map(|page| {
                    let by_id = page
                        .response
                        .articles
                        .iter()
                        .map(|article| (article.id.as_str(), article))
                        .collect::<std::collections::HashMap<_, _>>();
                    page.response.hits.iter().filter_map(move |hit| {
                        by_id
                            .get(hit.article_id.as_str())
                            .map(|article| (*article).clone())
                    })
                })
                .collect(),
            SnapshotData::Discovery { results, .. } => results
                .candidates
                .iter()
                .map(|c| c.article.clone())
                .collect(),
        }
    }
    pub fn coverage(&self) -> Result<SnapshotCoverage> {
        if self.started_at > self.finished_at {
            return Err(invalid("execution time window is reversed"));
        }
        let runtime = &self.runtime;
        if runtime.format_version != 1
            || runtime.query_contract_version != 2
            || runtime.application_version.is_empty()
            || runtime.application_version.len() > 128
            || !digest_valid(&runtime.engine_fingerprint)
        {
            return Err(invalid("unsupported or missing execution fingerprint"));
        }
        match &self.data {
            SnapshotData::Library {
                pages,
                requested_max_items,
                termination,
            } => {
                let first = pages.first().ok_or_else(|| invalid("no captured page"))?;
                let mut intent = first.request.clone();
                intent.cursor = None;
                let hybrid = intent.mode == SearchMode::Hybrid;
                if hybrid != runtime.model.is_some() {
                    return Err(invalid("model provenance must match executed search mode"));
                }
                if let Some(model) = &runtime.model {
                    if model.name.is_empty()
                        || model.name.len() > 256
                        || !digest_valid(&model.digest)
                        || !digest_valid(&model.signature)
                        || model.semantic_generation.is_empty()
                        || model.configuration_generation.is_empty()
                        || model.provider.is_empty()
                        || model.provider.len() > 64
                    {
                        return Err(invalid("invalid model fingerprint"));
                    }
                    let origin = reqwest::Url::parse(&model.origin)
                        .map_err(|_| invalid("invalid model origin"))?;
                    if !matches!(origin.scheme(), "http" | "https")
                        || origin.host_str().is_none()
                        || !origin.username().is_empty()
                        || origin.password().is_some()
                        || origin.query().is_some()
                        || origin.fragment().is_some()
                        || !matches!(origin.path(), "" | "/")
                    {
                        return Err(invalid(
                            "model origin must not include credentials or extra parameters",
                        ));
                    }
                }
                let mut ids = HashSet::new();
                let mut cursor = first.request.cursor.clone();
                for (page_number, page) in pages.iter().enumerate() {
                    if page_number > 0 && cursor.is_none() {
                        return Err(invalid("page appended after cursor exhaustion"));
                    }
                    let mut request = page.request.clone();
                    request.cursor = None;
                    if request != intent
                        || page.request.cursor != cursor
                        || page.response.index_generation != first.response.index_generation
                        || page.response.total != first.response.total
                        || page.response.index_generation.is_empty()
                    {
                        return Err(SnapshotError::Conflict(
                            "pages belong to different queries, cursor chains or generations"
                                .into(),
                        ));
                    }
                    if !page.request.include_articles
                        || page.response.articles.len() != page.response.hits.len()
                        || !(1..=200).contains(&page.request.page_size)
                        || page.response.hits.len() > page.request.page_size as usize
                    {
                        return Err(invalid(
                            "captured pages require complete frozen article metadata",
                        ));
                    }
                    let article_ids = page
                        .response
                        .articles
                        .iter()
                        .map(|a| (a.id.as_str(), a))
                        .collect::<std::collections::HashMap<_, _>>();
                    if article_ids.len() != page.response.hits.len() {
                        return Err(invalid("duplicate captured article metadata"));
                    }
                    for hit in &page.response.hits {
                        if hit.article_id.is_empty()
                            || !hit.score.is_finite()
                            || article_ids
                                .get(hit.article_id.as_str())
                                .is_none_or(|article| article.title != hit.title)
                            || !ids.insert(hit.article_id.as_str())
                        {
                            return Err(invalid(
                                "nonfinite score, missing metadata or duplicate page hit",
                            ));
                        }
                    }
                    validate_bound(*requested_max_items, ids.len())?;
                    match (&runtime.model, &page.response.retrieval) {
                        (Some(model), Some(retrieval))
                            if model.semantic_generation == retrieval.semantic_generation
                                && model.configuration_generation
                                    == retrieval.configuration_generation => {}
                        (None, None) => {}
                        _ => {
                            return Err(SnapshotError::Conflict(
                                "model provenance differs from executed retrieval".into(),
                            ));
                        }
                    }
                    if page.response.hits.is_empty() && page.response.next_cursor.is_some() {
                        return Err(invalid("empty continuation page"));
                    }
                    cursor = page.response.next_cursor.clone();
                }
                if ids.len() > first.response.total
                    || *termination == CaptureTermination::End && cursor.is_some()
                {
                    return Err(invalid("capture end contradicts recorded results"));
                }
                validate_bound(*requested_max_items, ids.len())?;
                Ok(SnapshotCoverage {
                    result_basis: if hybrid {
                        "candidate_pool"
                    } else {
                        "complete_matches"
                    }
                    .into(),
                    complete_obtained_set: first.request.cursor.is_none()
                        && cursor.is_none()
                        && ids.len() == first.response.total,
                    sources_exhausted: None,
                    captured_items: ids.len(),
                })
            }
            SnapshotData::Discovery {
                session,
                results,
                requested_max_items,
                termination,
                ..
            } => {
                if runtime.model.is_some() {
                    return Err(invalid("external discovery does not imply model inference"));
                }
                if session.id.is_empty()
                    || session.sequence != results.sequence
                    || session.merged_candidates != results.obtained_candidates
                {
                    return Err(SnapshotError::Conflict(
                        "discovery capture mixes session sequences".into(),
                    ));
                }
                let requested = session.request.sources.iter().collect::<HashSet<_>>();
                let source_ids = session
                    .sources
                    .iter()
                    .map(|source| &source.source)
                    .collect::<HashSet<_>>();
                if requested.is_empty()
                    || requested.len() != session.request.sources.len()
                    || source_ids.len() != session.sources.len()
                    || requested != source_ids
                {
                    return Err(invalid(
                        "discovery provenance omits or duplicates requested sources",
                    ));
                }
                let end = results
                    .offset
                    .checked_add(results.candidates.len())
                    .ok_or_else(|| invalid("candidate range overflow"))?;
                if end > results.obtained_candidates
                    || results
                        .next_offset
                        .is_some_and(|next| next != end || next >= results.obtained_candidates)
                    || results.next_offset.is_none() && end < results.obtained_candidates
                    || *termination == CaptureTermination::End && results.next_offset.is_some()
                {
                    return Err(invalid("candidate page coverage is inconsistent"));
                }
                let mut ids = HashSet::new();
                for c in &results.candidates {
                    if c.id.is_empty()
                        || c.article.id.is_empty()
                        || !c.rrf_score.is_finite()
                        || !ids.insert(&c.id)
                        || c.records.is_empty()
                    {
                        return Err(invalid("invalid or duplicate discovery candidate"));
                    }
                    for record in &c.records {
                        if !source_ids.contains(&record.source)
                            || record.article.id.is_empty()
                            || record.source_rank == 0
                            || record.query_fingerprint.is_empty()
                            || DateTime::parse_from_rfc3339(&record.retrieved_at).is_err()
                        {
                            return Err(invalid("invalid source contribution provenance"));
                        }
                    }
                }
                validate_bound(*requested_max_items, ids.len())?;
                Ok(SnapshotCoverage {
                    result_basis: "obtained_candidates".into(),
                    complete_obtained_set: results.offset == 0
                        && end == results.obtained_candidates,
                    sources_exhausted: Some(
                        !session.sources.is_empty()
                            && session.sources.iter().all(|s| {
                                s.phase == DiscoverySourcePhase::Exhausted
                                    && !s.has_more
                                    && s.failure.is_none()
                            }),
                    ),
                    captured_items: ids.len(),
                })
            }
        }
    }
}
fn validate_bound(max: usize, count: usize) -> Result<()> {
    if !(1..=10000).contains(&max) || count > max {
        return Err(invalid("captured item count exceeds requested bound"));
    }
    Ok(())
}

struct BoundedJson {
    bytes: Vec<u8>,
    max: usize,
    overflow: bool,
}
impl Write for BoundedJson {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.max.saturating_sub(self.bytes.len()) {
            self.overflow = true;
            return Err(std::io::Error::other("snapshot serialization budget"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub(crate) fn bounded_json(value: &impl Serialize, max: usize) -> Result<Vec<u8>> {
    let mut writer = BoundedJson {
        bytes: Vec::new(),
        max,
        overflow: false,
    };
    let result = serde_json::to_writer(&mut writer, value);
    if writer.overflow {
        return Err(SnapshotError::Budget(format!(
            "serialized capture exceeds {max} bytes"
        )));
    }
    result?;
    Ok(writer.bytes)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotSummary {
    pub kind: SnapshotKind,
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub checksum: String,
    pub payload_bytes: usize,
    pub coverage: SnapshotCoverage,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSnapshot {
    pub summary: SnapshotSummary,
    pub capture: SnapshotCapture,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotPage {
    pub snapshots: Vec<SnapshotSummary>,
    pub next_cursor: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListCursor {
    version: u16,
    store: String,
    high: i64,
    anchor: String,
    before: i64,
}
#[derive(Clone)]
pub struct SnapshotStore {
    conn: Arc<Mutex<Connection>>,
    identity: String,
}
impl SnapshotStore {
    pub async fn open(path: &str) -> Result<Self> {
        let mut builder = turso::Builder::new_local(path);
        if path != ":memory:" {
            builder = builder.experimental_multiprocess_wal(true);
        }
        let db = builder.build().await?;
        let conn = db.connect()?;
        if path != ":memory:" {
            conn.pragma_update("busy_timeout", 5000).await?;
        }
        conn.execute_batch("CREATE TABLE IF NOT EXISTS bibliographySnapshotMeta(key TEXT PRIMARY KEY,value TEXT NOT NULL); CREATE TABLE IF NOT EXISTS bibliographySearchSnapshots(seq INTEGER PRIMARY KEY AUTOINCREMENT,id TEXT NOT NULL UNIQUE,name TEXT NOT NULL,createdAt TEXT NOT NULL,idempotencyKey TEXT NOT NULL UNIQUE,intentHash TEXT NOT NULL,captureHash TEXT NOT NULL,payloadBytes INTEGER NOT NULL,payload BLOB NOT NULL,coverage TEXT NOT NULL);").await?;
        conn.execute(
            "INSERT OR IGNORE INTO bibliographySnapshotMeta(key,value) VALUES ('store_id',?1)",
            turso::params![uuid::Uuid::new_v4().to_string()],
        )
        .await?;
        conn.execute(
            "INSERT OR IGNORE INTO bibliographySnapshotMeta(key,value) VALUES ('limits',?1)",
            turso::params![serde_json::to_string(&SnapshotLimits::default())?],
        )
        .await?;
        let identity = meta(&conn, "store_id").await?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            identity,
        };
        store.limits().await?;
        Ok(store)
    }
    pub async fn limits(&self) -> Result<SnapshotLimits> {
        let conn = self.conn.lock().await;
        read_limits(&conn).await
    }
    pub async fn configure_limits(&self, limits: SnapshotLimits) -> Result<()> {
        limits.validate()?;
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE bibliographySnapshotMeta SET value=?1 WHERE key='limits'",
            turso::params![serde_json::to_string(&limits)?],
        )
        .await?;
        Ok(())
    }
    /// Resolve a committed action before executing a query or requiring a live
    /// discovery session/model. Retrying a lost response returns the original.
    pub async fn find_for_intent(
        &self,
        key: &str,
        name: &str,
        intent: &SnapshotIntent,
    ) -> Result<Option<StoredSnapshot>> {
        let name = validate_action(key, name)?;
        let expected = hash(&intent.bytes(name)?);
        let conn = self.conn.lock().await;
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Deferred).await?;
        let result = async {
            let mut rows = tx
                .query(
                    "SELECT id,intentHash FROM bibliographySearchSnapshots WHERE idempotencyKey=?1",
                    turso::params![key],
                )
                .await?;
            let Some(row) = rows.next().await? else {
                return Ok(None);
            };
            let id: String = row.get(0)?;
            let actual: String = row.get(1)?;
            drop(rows);
            if expected != actual {
                return Err(SnapshotError::Conflict(
                    "idempotency key belongs to another capture intent".into(),
                ));
            }
            read_snapshot(&tx, &id).await
        }
        .await;
        match result {
            Ok(value) => {
                tx.commit().await?;
                Ok(value)
            }
            Err(error) => {
                tx.rollback().await?;
                Err(error)
            }
        }
    }
    /// A discovery session's request is immutable. Resolve its committed action
    /// before requiring that in-memory session to still exist.
    pub async fn find_for_saved_discovery(
        &self,
        key: &str,
        name: &str,
        session_id: &str,
        sequence: u64,
        order: DiscoveryResultOrder,
        max_items: usize,
    ) -> Result<Option<StoredSnapshot>> {
        let name = validate_action(key, name)?;
        validate_bound(max_items, 0)?;
        let conn = self.conn.lock().await;
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Deferred).await?;
        let result = async {
            let mut rows = tx
                .query(
                    "SELECT id FROM bibliographySearchSnapshots WHERE idempotencyKey=?1",
                    turso::params![key],
                )
                .await?;
            let Some(row) = rows.next().await? else {
                return Ok(None);
            };
            let id: String = row.get(0)?;
            drop(rows);
            let saved = read_snapshot(&tx, &id)
                .await?
                .ok_or_else(|| SnapshotError::Corrupt("committed action vanished".into()))?;
            let same = match &saved.capture.data {
                SnapshotData::Discovery {
                    session,
                    order: actual_order,
                    requested_max_items,
                    ..
                } => {
                    saved.summary.name == name
                        && session.id == session_id
                        && session.sequence == sequence
                        && *actual_order == order
                        && *requested_max_items == max_items
                }
                _ => false,
            };
            if !same {
                return Err(SnapshotError::Conflict(
                    "idempotency key belongs to another capture intent".into(),
                ));
            }
            Ok(Some(saved))
        }
        .await;
        match result {
            Ok(value) => {
                tx.commit().await?;
                Ok(value)
            }
            Err(error) => {
                tx.rollback().await?;
                Err(error)
            }
        }
    }
    pub async fn save(
        &self,
        key: &str,
        name: &str,
        capture: SnapshotCapture,
    ) -> Result<StoredSnapshot> {
        let name = validate_action(key, name)?;
        let intent_hash = hash(&capture.intent(name)?);
        let conn = self.conn.lock().await;
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result=async {
            let mut rows=tx.query("SELECT id,intentHash FROM bibliographySearchSnapshots WHERE idempotencyKey=?1",turso::params![key]).await?;
            if let Some(row)=rows.next().await? {let id:String=row.get(0)?;let existing:String=row.get(1)?;drop(rows);if existing!=intent_hash{return Err(SnapshotError::Conflict("idempotency key belongs to another capture intent".into()));}return read_snapshot(&tx,&id).await?.ok_or_else(||SnapshotError::Corrupt("idempotency target vanished".into()));}drop(rows);
            let limits=read_limits(&tx).await?;let coverage=capture.coverage()?;let payload=bounded_json(&capture,limits.max_snapshot_bytes)?;
            let mut rows=tx.query("SELECT COUNT(*),COALESCE(SUM(length(payload)),0) FROM bibliographySearchSnapshots",()).await?;let row=rows.next().await?.ok_or_else(||SnapshotError::Corrupt("missing usage aggregate".into()))?;let count:i64=row.get(0)?;let bytes:i64=row.get(1)?;drop(rows);
            if count<0 || bytes<0{return Err(SnapshotError::Corrupt("negative snapshot usage".into()));}
            if count as u64>=limits.max_snapshots as u64 || (bytes as u64).saturating_add(payload.len() as u64)>limits.max_total_bytes{return Err(SnapshotError::Budget("snapshot count or total payload limit".into()));}
            let summary=SnapshotSummary{kind:match &capture.data {SnapshotData::Library{..}=>SnapshotKind::Library,SnapshotData::Discovery{..}=>SnapshotKind::Discovery},id:uuid::Uuid::new_v4().to_string(),name:name.into(),created_at:Utc::now().to_rfc3339(),checksum:hash(&payload),payload_bytes:payload.len(),coverage};
            tx.execute("INSERT INTO bibliographySearchSnapshots(id,name,createdAt,idempotencyKey,intentHash,captureHash,payloadBytes,payload,coverage) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",turso::params![summary.id.clone(),summary.name.clone(),summary.created_at.clone(),key,intent_hash,summary.checksum.clone(),payload.len() as i64,payload,serde_json::to_string(&summary.coverage)?]).await?;
            Ok(StoredSnapshot{summary,capture})
        }.await;
        match result {
            Ok(value) => {
                tx.commit().await?;
                Ok(value)
            }
            Err(error) => {
                tx.rollback().await?;
                Err(error)
            }
        }
    }
    pub async fn get(&self, id: &str) -> Result<Option<StoredSnapshot>> {
        let conn = self.conn.lock().await;
        read_snapshot(&conn, id).await
    }
    pub async fn delete(&self, id: &str, expected_checksum: &str) -> Result<bool> {
        let conn = self.conn.lock().await;
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result = async {
            // Explicit deletion must also work for damaged capture payloads.
            let mut rows = tx
                .query(
                    "SELECT captureHash FROM bibliographySearchSnapshots WHERE id=?1",
                    turso::params![id],
                )
                .await?;
            let Some(row) = rows.next().await? else {
                return Ok(false);
            };
            let checksum: String = row.get(0)?;
            drop(rows);
            if checksum != expected_checksum {
                return Err(SnapshotError::Conflict("snapshot checksum changed".into()));
            }
            tx.execute(
                "DELETE FROM bibliographySearchSnapshots WHERE id=?1",
                turso::params![id],
            )
            .await?;
            Ok(true)
        }
        .await;
        match result {
            Ok(value) => {
                tx.commit().await?;
                Ok(value)
            }
            Err(error) => {
                tx.rollback().await?;
                Err(error)
            }
        }
    }
    pub async fn list(&self, limit: usize, cursor: Option<&str>) -> Result<SnapshotPage> {
        if !(1..=100).contains(&limit) {
            return Err(invalid("list limit outside 1..100"));
        }
        let decoded = if let Some(cursor) = cursor {
            if cursor.len() > 512 {
                return Err(invalid("invalid list cursor"));
            }
            let bytes = URL_SAFE_NO_PAD
                .decode(cursor)
                .map_err(|_| invalid("invalid list cursor"))?;
            let c: ListCursor =
                serde_json::from_slice(&bytes).map_err(|_| invalid("invalid list cursor"))?;
            if c.version != 1
                || c.store != self.identity
                || c.high < 1
                || c.before < 1
                || c.before > c.high
            {
                return Err(invalid("list cursor belongs to another store or version"));
            }
            Some(c)
        } else {
            None
        };
        let conn = self.conn.lock().await;
        let (high, anchor) = if let Some(c) = &decoded {
            let mut rows = conn
                .query(
                    "SELECT id FROM bibliographySearchSnapshots WHERE seq=?1",
                    turso::params![c.high],
                )
                .await?;
            let actual = rows
                .next()
                .await?
                .map(|row| row.get::<String>(0))
                .transpose()?;
            if actual.as_deref() != Some(c.anchor.as_str()) {
                return Err(SnapshotError::Conflict(
                    "snapshot list changed or was restored; restart pagination".into(),
                ));
            }
            (c.high, c.anchor.clone())
        } else {
            let mut rows = conn
                .query(
                    "SELECT seq,id FROM bibliographySearchSnapshots ORDER BY seq DESC LIMIT 1",
                    (),
                )
                .await?;
            if let Some(row) = rows.next().await? {
                (row.get(0)?, row.get(1)?)
            } else {
                (0, String::new())
            }
        };
        let before = decoded
            .as_ref()
            .map(|c| turso::Value::Integer(c.before))
            .unwrap_or(turso::Value::Null);
        let mut rows=conn.query("SELECT seq,id,name,createdAt,captureHash,payloadBytes,coverage FROM bibliographySearchSnapshots WHERE seq<=?1 AND (?2 IS NULL OR seq<?2) ORDER BY seq DESC LIMIT ?3",turso::params![high,before,(limit+1) as i64]).await?;
        let mut values = Vec::new();
        while let Some(row) = rows.next().await? {
            let seq: i64 = row.get(0)?;
            values.push((seq, summary(&row, 1)?));
        }
        let more = values.len() > limit;
        values.truncate(limit);
        let next_cursor = if more {
            Some(
                URL_SAFE_NO_PAD.encode(serde_json::to_vec(&ListCursor {
                    version: 1,
                    store: self.identity.clone(),
                    high,
                    anchor,
                    before: values
                        .last()
                        .ok_or_else(|| SnapshotError::Corrupt("empty list window".into()))?
                        .0,
                })?),
            )
        } else {
            None
        };
        Ok(SnapshotPage {
            snapshots: values.into_iter().map(|(_, s)| s).collect(),
            next_cursor,
        })
    }
}
fn validate_action<'a>(key: &str, name: &'a str) -> Result<&'a str> {
    let name = name.trim();
    if key.is_empty()
        || key.len() > 128
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
        || name.is_empty()
        || name.chars().count() > 200
        || name.chars().any(char::is_control)
    {
        return Err(invalid("invalid idempotency key or name"));
    }
    Ok(name)
}
async fn meta(conn: &Connection, key: &str) -> Result<String> {
    let mut rows = conn
        .query(
            "SELECT value FROM bibliographySnapshotMeta WHERE key=?1",
            turso::params![key],
        )
        .await?;
    rows.next()
        .await?
        .ok_or_else(|| SnapshotError::Corrupt(format!("missing {key} metadata")))?
        .get(0)
        .map_err(Into::into)
}
async fn read_limits(conn: &Connection) -> Result<SnapshotLimits> {
    let limits: SnapshotLimits = serde_json::from_str(&meta(conn, "limits").await?)?;
    limits.validate()?;
    Ok(limits)
}
fn summary(row: &turso::Row, start: usize) -> Result<SnapshotSummary> {
    let size: i64 = row.get(start + 4)?;
    let checksum: String = row.get(start + 3)?;
    if size < 0 || !digest_valid(&checksum) {
        return Err(SnapshotError::Corrupt(
            "invalid snapshot size/checksum".into(),
        ));
    }
    let coverage: SnapshotCoverage = serde_json::from_str(&row.get::<String>(start + 5)?)?;
    let kind = match coverage.result_basis.as_str() {
        "complete_matches" | "candidate_pool" => SnapshotKind::Library,
        "obtained_candidates" => SnapshotKind::Discovery,
        _ => {
            return Err(SnapshotError::Corrupt(
                "unknown captured result basis".into(),
            ));
        }
    };
    Ok(SnapshotSummary {
        kind,
        id: row.get(start)?,
        name: row.get(start + 1)?,
        created_at: row.get(start + 2)?,
        checksum,
        payload_bytes: size as usize,
        coverage,
    })
}
async fn read_snapshot(conn: &Connection, id: &str) -> Result<Option<StoredSnapshot>> {
    let mut rows=conn.query("SELECT id,name,createdAt,captureHash,payloadBytes,coverage,payload,intentHash FROM bibliographySearchSnapshots WHERE id=?1",turso::params![id]).await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    let summary = summary(&row, 0)?;
    let bytes: Vec<u8> = row.get(6)?;
    if bytes.len() != summary.payload_bytes || hash(&bytes) != summary.checksum {
        return Err(SnapshotError::Corrupt(
            "capture checksum or length mismatch".into(),
        ));
    }
    let capture: SnapshotCapture = serde_json::from_slice(&bytes)?;
    if hash(&capture.intent(&summary.name)?) != row.get::<String>(7)? {
        return Err(SnapshotError::Corrupt(
            "stored intent differs from captured request/name".into(),
        ));
    }
    if capture.coverage()? != summary.coverage {
        return Err(SnapshotError::Corrupt(
            "stored coverage differs from capture".into(),
        ));
    }
    Ok(Some(StoredSnapshot { summary, capture }))
}
