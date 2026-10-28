//! P5 bounded forward/backward citation and recommendation lists. Transport is
//! supplied by BibShared's S2 client; no independent request pool is created.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use bib_types::{
    Article,
    discovery::{SourceFailure, SourceFailureKind},
};
use semantic_scholar::{
    S2Client, S2Error,
    relation_types::{PaperRelation, RecommendationPool, RelationPaper},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExplorationKind {
    References,
    Citations,
    Similar { pool: RecommendationPool },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplorationRequest {
    /// Canonical S2 ID or an exact identifier supported by the discovery source.
    pub seed: String,
    pub kind: ExplorationKind,
    pub page_size: u16,
    pub cursor: Option<String>,
    /// Whole operation budget, including shared pacing and Retry-After waits.
    #[serde(default = "default_deadline")]
    pub deadline_ms: u64,
}
fn default_deadline() -> u64 {
    15_000
}
#[derive(Debug, thiserror::Error)]
pub enum ExplorationError {
    #[error("invalid exploration request: {0}")]
    Invalid(String),
    #[error("exploration cursor belongs to a different request")]
    CursorExpired,
    #[error("upstream exploration failed")]
    Upstream(SourceFailure),
}
type Result<T> = std::result::Result<T, ExplorationError>;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CitationEdge {
    pub citing_paper_id: String,
    pub cited_paper_id: String,
    pub source: String,
    pub retrieved_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplorationItem {
    /// Raw upstream position, including unresolved entries.
    pub position: u32,
    pub article: Option<Article>,
    pub unresolved: Option<serde_json::Value>,
    pub citation: Option<CitationEdge>,
    pub contexts: Option<Vec<String>>,
    pub intents: Option<Vec<String>>,
    pub is_influential: Option<bool>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplorationPage {
    pub seed: String,
    pub kind: ExplorationKind,
    pub source: String,
    pub retrieved_at: String,
    pub items: Vec<ExplorationItem>,
    pub unresolved_records: usize,
    pub next_cursor: Option<String>,
    /// Graph offsets are live upstream windows, not an immutable snapshot.
    pub upstream_snapshot: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u16,
    binding: String,
    offset: u32,
}
const SOURCE: &str = "semantic_scholar";
const FIELDS: &str = "paperId,title,abstract,year,venue,authors,externalIds,publicationDate,journal,citationCount,isOpenAccess,openAccessPdf";
fn valid_id(id: &str) -> bool {
    id.len() == 40 && id.bytes().all(|b| b.is_ascii_hexdigit())
}
fn invalid(message: &str) -> ExplorationError {
    ExplorationError::Invalid(message.into())
}
fn bad() -> ExplorationError {
    ExplorationError::Upstream(SourceFailure::new(
        SourceFailureKind::InvalidResponse,
        "Invalid exploration metadata",
        false,
    ))
}
fn upstream(error: S2Error) -> ExplorationError {
    let (kind, retryable, retry_after_seconds) = match error {
        S2Error::RateLimited {
            retry_after_seconds,
        } => (SourceFailureKind::RateLimited, true, retry_after_seconds),
        S2Error::Status {
            status: 401 | 403, ..
        } => (SourceFailureKind::AuthRequired, false, None),
        S2Error::Status { status: 404, .. } => (SourceFailureKind::InvalidQuery, false, None),
        S2Error::Status {
            status: 500..=599, ..
        } => (SourceFailureKind::Upstream, true, None),
        S2Error::Http(error) if error.is_timeout() => (SourceFailureKind::Timeout, true, None),
        S2Error::Http(_) => (SourceFailureKind::Transport, true, None),
        S2Error::Param(_) => (SourceFailureKind::InvalidQuery, false, None),
        _ => (SourceFailureKind::InvalidResponse, false, None),
    };
    let message = match kind {
        SourceFailureKind::RateLimited => retry_after_seconds.map_or_else(
            || "Semantic Scholar rate limit reached. Retry later.".into(),
            |seconds| {
                format!("Semantic Scholar rate limit reached. Retry after {seconds} seconds.")
            },
        ),
        SourceFailureKind::AuthRequired => {
            "Semantic Scholar credentials are required or were rejected. Check source settings."
                .into()
        }
        SourceFailureKind::InvalidQuery => {
            "Semantic Scholar could not resolve the requested paper identifier.".into()
        }
        SourceFailureKind::Timeout => "Semantic Scholar request timed out. Retry this list.".into(),
        SourceFailureKind::Transport => "Could not reach Semantic Scholar. Retry this list.".into(),
        SourceFailureKind::Upstream => {
            "Semantic Scholar is temporarily unavailable. Retry later.".into()
        }
        _ => {
            "Semantic Scholar returned invalid exploration data. Refresh or retry this list.".into()
        }
    };
    let mut failure = SourceFailure::new(kind, message, retryable);
    failure.retry_after_seconds = retry_after_seconds;
    ExplorationError::Upstream(failure)
}

pub async fn explore(client: &S2Client, request: ExplorationRequest) -> Result<ExplorationPage> {
    if !(100..=120_000).contains(&request.deadline_ms) {
        return Err(invalid("deadline_ms must be between 100 and 120000"));
    }
    tokio::time::timeout(
        std::time::Duration::from_millis(request.deadline_ms),
        explore_inner(client, request),
    )
    .await
    .map_err(|_| {
        ExplorationError::Upstream(SourceFailure::new(
            SourceFailureKind::Timeout,
            "Exploration deadline exceeded",
            true,
        ))
    })?
}

async fn explore_inner(
    client: &S2Client,
    mut request: ExplorationRequest,
) -> Result<ExplorationPage> {
    if request.seed.len() > 1024 || !(1..=100).contains(&request.page_size) {
        return Err(invalid(
            "supported exact identifier and page_size 1..100 are required",
        ));
    }
    if request
        .cursor
        .as_ref()
        .is_some_and(|value| value.len() > 2048)
    {
        return Err(ExplorationError::CursorExpired);
    }
    let seed = crate::discovery_s2::identifier(&request.seed).map_err(|_| {
        invalid("unsupported seed identifier; use S2, DOI, PMID or unversioned arXiv")
    })?;
    // Clone retains shared transport, pacing and cooldown. At most one exact
    // lookup plus one page request, with no hidden automatic retry sequence.
    let client = client.clone().with_max_retries(0);
    request.seed = if valid_id(&seed) {
        seed.to_ascii_lowercase()
    } else {
        let paper = client
            .get_paper(&seed, Some(FIELDS))
            .await
            .map_err(upstream)?;
        if !crate::discovery_s2::valid(&paper) || !crate::discovery_s2::exact(&paper, &seed) {
            return Err(bad());
        }
        paper.paper_id.to_ascii_lowercase()
    };
    let binding = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(
                &request.seed,
                request.kind,
                request.page_size,
                FIELDS,
                SOURCE
            ))
            .map_err(|_| bad())?
        )
    );
    let offset = if let Some(value) = &request.cursor {
        if value.len() > 2048 {
            return Err(ExplorationError::CursorExpired);
        }
        let cursor: Cursor = URL_SAFE_NO_PAD
            .decode(value)
            .ok()
            .and_then(|raw| serde_json::from_slice(&raw).ok())
            .ok_or(ExplorationError::CursorExpired)?;
        if cursor.version != 1 || cursor.binding != binding || cursor.offset == 0 {
            return Err(ExplorationError::CursorExpired);
        }
        cursor.offset
    } else {
        0
    };
    let mut items = Vec::new();
    let retrieved_at;
    let next;
    match request.kind {
        ExplorationKind::References | ExplorationKind::Citations => {
            let relation = if request.kind == ExplorationKind::References {
                PaperRelation::References
            } else {
                PaperRelation::Citations
            };
            let fields = format!("{FIELDS},contexts,intents,isInfluential");
            let page = client
                .get_relations(
                    &request.seed,
                    relation,
                    request.page_size.into(),
                    offset,
                    Some(&fields),
                )
                .await
                .map_err(upstream)?;
            retrieved_at = chrono::Utc::now().to_rfc3339();
            next = page.next;
            for (index, record) in page.data.into_iter().enumerate() {
                let (article, unresolved, citation) = match record.paper(relation) {
                    Some(RelationPaper::Resolved(paper)) => {
                        if !valid_id(&paper.paper_id)
                            || paper
                                .title
                                .as_ref()
                                .is_none_or(|title| title.trim().is_empty())
                            || paper.year.is_some_and(|year| !(1..=9999).contains(&year))
                        {
                            return Err(bad());
                        }
                        let id = paper.paper_id.to_ascii_lowercase();
                        let (citing, cited) = if relation == PaperRelation::References {
                            (request.seed.clone(), id)
                        } else {
                            (id, request.seed.clone())
                        };
                        (
                            Some(semantic_scholar::paper_to_article(paper)),
                            None,
                            Some(CitationEdge {
                                citing_paper_id: citing,
                                cited_paper_id: cited,
                                source: SOURCE.into(),
                                retrieved_at: retrieved_at.clone(),
                            }),
                        )
                    }
                    Some(RelationPaper::Unresolved(value)) => {
                        (None, Some(serde_json::Value::Object(value.clone())), None)
                    }
                    None => (None, Some(serde_json::Value::Null), None),
                };
                items.push(ExplorationItem {
                    position: offset + index as u32,
                    article,
                    unresolved,
                    citation,
                    contexts: record.contexts,
                    intents: record.intents,
                    is_influential: record.is_influential,
                });
            }
        }
        ExplorationKind::Similar { pool } => {
            if request.cursor.is_some() {
                return Err(invalid("recommendations have no continuation"));
            }
            let page = client
                .recommendations_from(&request.seed, pool, request.page_size.into(), Some(FIELDS))
                .await
                .map_err(upstream)?;
            retrieved_at = chrono::Utc::now().to_rfc3339();
            next = None;
            let mut seen = std::collections::HashSet::new();
            for (index, paper) in page.recommended_papers.into_iter().enumerate() {
                if !valid_id(&paper.paper_id)
                    || paper
                        .title
                        .as_ref()
                        .is_none_or(|title| title.trim().is_empty())
                    || paper.year.is_some_and(|year| !(1..=9999).contains(&year))
                    || !seen.insert(paper.paper_id.to_ascii_lowercase())
                {
                    return Err(bad());
                }
                items.push(ExplorationItem {
                    position: index as u32,
                    article: Some(semantic_scholar::paper_to_article(&paper)),
                    unresolved: None,
                    citation: None,
                    contexts: None,
                    intents: None,
                    is_influential: None,
                });
            }
        }
    }
    let next_cursor = next
        .map(|offset| {
            serde_json::to_vec(&Cursor {
                version: 1,
                binding,
                offset,
            })
            .map(|raw| URL_SAFE_NO_PAD.encode(raw))
        })
        .transpose()
        .map_err(|_| bad())?;
    Ok(ExplorationPage {
        seed: request.seed,
        kind: request.kind,
        source: SOURCE.into(),
        retrieved_at,
        unresolved_records: items.iter().filter(|item| item.article.is_none()).count(),
        items,
        next_cursor,
        upstream_snapshot: false,
    })
}
