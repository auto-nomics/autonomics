//! Paged discovery adapters. Provider cursors are bound to the planned query
//! and page size, and never used as arbitrary request URLs or credentials.
use crate::query::LiteratureSource;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use bib_types::discovery::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

type PageResult<T> = std::result::Result<T, SourceFailure>;
pub(crate) fn validate_page(plan: &SourcePlan, source: &str, page_size: u16) -> PageResult<()> {
    if plan.source != source || page_size != plan.page_size || !(1..=100).contains(&page_size) {
        return Err(failure(
            SourceFailureKind::InvalidQuery,
            "source or page size differs from the planned query",
        ));
    }
    Ok(())
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub(crate) enum ProviderCursor {
    SemanticScholar {
        fingerprint: String,
        position: crate::discovery_s2::Position,
    },
    Crossref {
        fingerprint: String,
        cursor: String,
        obtained: u64,
    },
    Arxiv {
        fingerprint: String,
        offset: u32,
        total: u64,
    },
    Pubmed {
        fingerprint: String,
        offset: u32,
        total: u64,
        web_env: String,
        query_key: String,
        translation: Option<String>,
    },
    Openalex {
        fingerprint: String,
        cursor: String,
    },
}
pub(crate) fn cursor_encode(cursor: ProviderCursor) -> PageResult<String> {
    let encoded = serde_json::to_vec(&cursor)
        .map(|v| URL_SAFE_NO_PAD.encode(v))
        .map_err(|_| {
            failure(
                SourceFailureKind::InvalidResponse,
                "cannot encode source cursor",
            )
        })?;
    if encoded.len() > 32_768 {
        return Err(failure(
            SourceFailureKind::InvalidResponse,
            "source cursor exceeds the supported size limit",
        ));
    }
    Ok(encoded)
}
pub(crate) fn cursor_decode(text: &str, plan: &SourcePlan) -> PageResult<ProviderCursor> {
    if text.len() > 32_768 {
        return Err(failure(
            SourceFailureKind::CursorExpired,
            "invalid source cursor",
        ));
    }
    let parsed = URL_SAFE_NO_PAD
        .decode(text)
        .ok()
        .and_then(|v| serde_json::from_slice::<ProviderCursor>(&v).ok())
        .ok_or_else(|| failure(SourceFailureKind::CursorExpired, "invalid source cursor"))?;
    let (source, fp) = match &parsed {
        ProviderCursor::SemanticScholar { fingerprint, .. } => ("semantic_scholar", fingerprint),
        ProviderCursor::Crossref { fingerprint, .. } => ("crossref", fingerprint),
        ProviderCursor::Arxiv { fingerprint, .. } => ("arxiv", fingerprint),
        ProviderCursor::Pubmed { fingerprint, .. } => ("pubmed", fingerprint),
        ProviderCursor::Openalex { fingerprint, .. } => ("openalex", fingerprint),
    };
    if source != plan.source || fp != &plan.fingerprint {
        return Err(failure(
            SourceFailureKind::CursorExpired,
            "cursor belongs to another source query or page size",
        ));
    }
    Ok(parsed)
}
fn failure(kind: SourceFailureKind, message: &str) -> SourceFailure {
    SourceFailure::new(kind, message, false)
}
pub(crate) fn status_failure(source: &str, status: u16) -> SourceFailure {
    let (kind, retryable) = match status {
        401 | 403 => (SourceFailureKind::AuthRequired, false),
        429 => (SourceFailureKind::RateLimited, true),
        400 | 422 => (SourceFailureKind::InvalidQuery, false),
        404 | 410 => (SourceFailureKind::CursorExpired, false),
        _ => (SourceFailureKind::Upstream, status >= 500),
    };
    SourceFailure::new(kind, format!("{source} returned HTTP {status}"), retryable)
}
fn eutils_failure(error: eutils::error::EutilsError) -> SourceFailure {
    use eutils::error::EutilsError as E;
    match error {
        E::RateLimited {
            retry_after_seconds,
            ..
        } => {
            let mut e = status_failure("PubMed", 429);
            e.retry_after_seconds = retry_after_seconds;
            e
        }
        E::Status { status, .. } => status_failure("PubMed", status),
        E::Http(error) => SourceFailure::new(
            if error.is_timeout() {
                SourceFailureKind::Timeout
            } else {
                SourceFailureKind::Transport
            },
            "PubMed HTTP request failed",
            true,
        ),
        E::Param(_) => failure(
            SourceFailureKind::InvalidQuery,
            "PubMed rejected the request parameters",
        ),
        E::Parse(_) => failure(
            SourceFailureKind::InvalidResponse,
            "PubMed returned an invalid response",
        ),
    }
}
fn openalex_failure(error: openalex::error::OpenAlexError) -> SourceFailure {
    use openalex::error::OpenAlexError as E;
    match error {
        E::RateLimited {
            retry_after_seconds,
            ..
        } => {
            let mut e = status_failure("OpenAlex", 429);
            e.retry_after_seconds = retry_after_seconds;
            e
        }
        E::Status { status, .. } => status_failure("OpenAlex", status),
        E::Http(error) => SourceFailure::new(
            if error.is_timeout() {
                SourceFailureKind::Timeout
            } else {
                SourceFailureKind::Transport
            },
            "OpenAlex HTTP request failed",
            true,
        ),
        E::Param(_) => failure(
            SourceFailureKind::InvalidQuery,
            "OpenAlex rejected the request parameters",
        ),
        E::Parse(_) => failure(
            SourceFailureKind::InvalidResponse,
            "OpenAlex returned an invalid response",
        ),
        E::NotFound(_) => failure(
            SourceFailureKind::CursorExpired,
            "OpenAlex cursor or resource was not found",
        ),
    }
}

pub(crate) fn unavailable(source: &str) -> SourceCapabilities {
    SourceCapabilities {
        source: source.into(),
        searchable: false,
        native_query: false,
        fields: Vec::new(),
        filters: Vec::new(),
        sorts: Vec::new(),
        pagination: SourcePagination::None,
        max_page_size: 0,
        credentials_configured: false,
        authentication: SourceAuthentication::Unknown,
        quota: None,
        notes: vec!["Discovery paging has not been qualified for this source.".into()],
    }
}
pub(crate) fn pubmed_capabilities(client: &eutils::EutilsClient) -> SourceCapabilities {
    use bib_types::SearchField::*;
    SourceCapabilities{source:"pubmed".into(),searchable:true,native_query:true,fields:vec![Any,Title,Abstract,Creator,Publication,Identifier,Year],filters:vec!["year".into(),"publication_type".into()],sorts:vec![DiscoverySort::Relevance,DiscoverySort::Newest],pagination:SourcePagination::History,max_page_size:100,credentials_configured:client.has_api_key(),authentication:SourceAuthentication::Optional,quota:None,notes:vec!["E-utilities exposes at most the first 10000 PubMed results; narrow larger searches. History cursors can expire upstream.".into()]}
}
pub(crate) fn openalex_capabilities(client: &openalex::OpenAlexClient) -> SourceCapabilities {
    use bib_types::SearchField::*;
    SourceCapabilities{source:"openalex".into(),searchable:true,native_query:true,fields:vec![Any,Title,Abstract,Creator,Identifier,Year],filters:vec!["year".into(),"publication_type".into(),"open_access".into()],sorts:vec![DiscoverySort::Relevance,DiscoverySort::Newest,DiscoverySort::Oldest,DiscoverySort::CitationCount],pagination:SourcePagination::Cursor,max_page_size:100,credentials_configured:client.has_api_key(),authentication:SourceAuthentication::RequiredForContinuation,quota:client.quota(),notes:vec!["Cursor continuation after the first page requires an OpenAlex API key. Cross-field OR/NOT may be unsupported; inspect the source plan.".into()]}
}

pub(crate) async fn pubmed_page(
    client: &eutils::EutilsClient,
    plan: &SourcePlan,
    cursor: Option<&str>,
    page_size: u16,
) -> PageResult<SourcePage> {
    validate_page(plan, "pubmed", page_size)?;
    let (offset, total, web_env, query_key, translation, ids) = if let Some(cursor) = cursor {
        let ProviderCursor::Pubmed {
            offset,
            total,
            web_env,
            query_key,
            translation,
            ..
        } = cursor_decode(cursor, plan)?
        else {
            return Err(failure(
                SourceFailureKind::CursorExpired,
                "invalid PubMed cursor",
            ));
        };
        (
            offset,
            total,
            Some(web_env),
            Some(query_key),
            translation,
            None,
        )
    } else {
        let result = client
            .esearch(&eutils::types::ESearchRequest {
                db: "pubmed".into(),
                term: plan.parameters.get("term").cloned().unwrap_or_default(),
                retmax: Some(u32::from(page_size)),
                retstart: Some(0),
                sort: plan.parameters.get("sort").cloned(),
                usehistory: Some(true),
                ..Default::default()
            })
            .await
            .map_err(eutils_failure)?
            .result;
        let total = result.count.parse::<u64>().map_err(|_| {
            failure(
                SourceFailureKind::InvalidResponse,
                "PubMed returned an invalid total",
            )
        })?;
        (
            0,
            total,
            result.web_env,
            result.query_key,
            result.query_translation,
            Some(result.id_list),
        )
    };
    if offset >= 10_000 || u64::from(offset) > total {
        return Err(failure(
            SourceFailureKind::CursorExpired,
            "PubMed cursor is outside its result window",
        ));
    }
    let articles = if total == 0 {
        Vec::new()
    } else {
        if ids.as_ref().is_some_and(Vec::is_empty) {
            return Err(failure(
                SourceFailureKind::InvalidResponse,
                "PubMed returned a nonzero count without IDs",
            ));
        }
        let text = client
            .efetch(&eutils::types::EFetchRequest {
                db: "pubmed".into(),
                id: ids.as_ref().map(|ids| ids.join(",")).unwrap_or_default(),
                rettype: Some("medline".into()),
                retmode: Some("text".into()),
                retmax: Some(u32::from(page_size)),
                retstart: Some(if ids.is_some() { 0 } else { offset }),
                web_env: if ids.is_none() { web_env.clone() } else { None },
                query_key: if ids.is_none() {
                    query_key.clone()
                } else {
                    None
                },
            })
            .await
            .map_err(eutils_failure)?;
        let articles = eutils::medline_to_articles(&text);
        if articles.is_empty() {
            return Err(failure(
                SourceFailureKind::InvalidResponse,
                "PubMed returned no parseable records for this page",
            ));
        }
        articles
    };
    if articles.len() > usize::from(page_size)
        || ids.as_ref().is_some_and(|ids| ids.len() != articles.len())
    {
        return Err(failure(
            SourceFailureKind::InvalidResponse,
            "PubMed page metadata does not match the returned ID window",
        ));
    }
    // History offsets describe the provider's window, including records
    // removed upstream, rather than the number of metadata objects parsed.
    let delivered = ids.as_ref().map(Vec::len).unwrap_or_else(|| {
        usize::from(page_size).min(total.saturating_sub(u64::from(offset)) as usize)
    }) as u32;
    let next = offset.saturating_add(delivered);
    let next_cursor = if u64::from(next) < total.min(10_000) {
        let (Some(web_env), Some(query_key)) = (web_env, query_key) else {
            return Err(failure(
                SourceFailureKind::InvalidResponse,
                "PubMed did not return a history handle for continuation",
            ));
        };
        Some(cursor_encode(ProviderCursor::Pubmed {
            fingerprint: plan.fingerprint.clone(),
            offset: next,
            total,
            web_env,
            query_key,
            translation: translation.clone(),
        })?)
    } else {
        None
    };
    let mut notices = plan.notices.clone();
    if total > 10_000 {
        notices.push(SourceNotice{code:"provider_limit".into(),message:"PubMed reports more than 10000 matches; only its first 10000 are accessible in this session. Narrow the query to retrieve other records.".into()});
    }
    let mut executed = plan.parameters.clone();
    executed.insert("retstart".into(), offset.to_string());
    executed.insert("retmax".into(), page_size.to_string());
    executed.insert("usehistory".into(), "y".into());
    Ok(SourcePage {
        source: "pubmed".into(),
        articles,
        total: Some(total),
        next_cursor,
        executed_parameters: executed,
        query_translation: translation,
        notices,
    })
}

pub(crate) async fn openalex_page(
    client: &openalex::OpenAlexClient,
    plan: &SourcePlan,
    cursor: Option<&str>,
    page_size: u16,
) -> PageResult<SourcePage> {
    validate_page(plan, "openalex", page_size)?;
    let native_cursor = if let Some(cursor) = cursor {
        if !client.has_api_key() {
            return Err(failure(
                SourceFailureKind::AuthRequired,
                "OpenAlex cursor continuation requires an API key",
            ));
        }
        let ProviderCursor::Openalex { cursor, .. } = cursor_decode(cursor, plan)? else {
            return Err(failure(
                SourceFailureKind::CursorExpired,
                "invalid OpenAlex cursor",
            ));
        };
        cursor
    } else {
        "*".into()
    };
    let mut params = openalex::ListParams::new()
        .with_per_page(u32::from(page_size))
        .with_cursor(&native_cursor);
    for (key, value) in &plan.parameters {
        match key.as_str() {
            "filter" => params = params.with_filter(value),
            "sort" => params = params.with_sort(value),
            _ => {
                params = params
                    .with_scoped_search(key, value)
                    .map_err(openalex_failure)?
            }
        }
    }
    let result = client.list_works(&params).await.map_err(openalex_failure)?;
    let articles = result
        .results
        .iter()
        .map(openalex::work_to_article)
        .collect::<Vec<_>>();
    if articles.len() > usize::from(page_size) {
        return Err(failure(
            SourceFailureKind::InvalidResponse,
            "OpenAlex returned more records than requested",
        ));
    }
    let next_cursor = match result.meta.next_cursor {
        Some(cursor) if !cursor.is_empty() && !articles.is_empty() => {
            Some(cursor_encode(ProviderCursor::Openalex {
                fingerprint: plan.fingerprint.clone(),
                cursor,
            })?)
        }
        _ => None,
    };
    let mut notices = plan.notices.clone();
    if next_cursor.is_some() && !client.has_api_key() {
        notices.push(SourceNotice {
            code: "auth_required_for_more".into(),
            message: "Configure OPENALEX_API_KEY on the backend to continue past this page.".into(),
        });
    }
    let mut executed = plan.parameters.clone();
    executed.insert("cursor".into(), native_cursor);
    executed.insert("per_page".into(), page_size.to_string());
    Ok(SourcePage {
        source: "openalex".into(),
        articles,
        total: Some(result.meta.count),
        next_cursor,
        executed_parameters: executed,
        query_translation: None,
        notices,
    })
}

impl crate::query::LiteratureGateway {
    pub fn discovery_capabilities(&self) -> Vec<SourceCapabilities> {
        self.discovery_sources()
            .iter()
            .map(|source| source.discovery_capabilities())
            .collect()
    }
    pub(crate) fn discovery_sources(&self) -> Vec<Arc<dyn LiteratureSource>> {
        self.sources.clone()
    }
}
