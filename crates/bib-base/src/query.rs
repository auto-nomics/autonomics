//! Unified literature query layer.
//!
//! [`LiteratureSource`] abstracts over individual SDK backends (PubMed,
//! arXiv, …). [`LiteratureGateway`] holds a collection of sources and
//! fans out a [`StructuredSearch`] to all of them concurrently, merging
//! results into a single response.
//!
//! Each SDK crate provides:
//! 1. A query translator (`to_entrez`, `to_arxiv`) — `StructuredSearch` → native syntax.
//! 2. A converter (`esummary_to_articles`, `atom_to_articles`) — raw response → `Article`.
//!
//! This module provides adapter structs that wire those two pieces into
//! the uniform `LiteratureSource` interface.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;

use bib_types::query::StructuredSearch;
use bib_types::Article;

use crate::error::{Error, Result};

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

/// One source's contribution to a multi-source search.
#[derive(Debug, Clone, Serialize)]
pub struct SourceBatch {
    /// Source name (e.g. `"pubmed"`, `"arxiv"`).
    pub source: String,
    /// Total matches reported by the source (may exceed `articles.len()`).
    pub total: usize,
    /// Articles returned in this batch.
    pub articles: Vec<Article>,
}

// ---------------------------------------------------------------------------
// LiteratureSource trait
// ---------------------------------------------------------------------------

/// A literature source that supports search and fetch.
///
/// Implementors wrap a specific SDK client and handle:
/// - Translating [`StructuredSearch`] into the source's native query syntax.
/// - Calling the source API.
/// - Converting the raw response into [`Article`] records.
#[async_trait]
pub trait LiteratureSource: Send + Sync {
    /// Human-readable identifier (e.g. `"pubmed"`, `"arxiv"`).
    fn name(&self) -> &'static str;

    /// Search for articles matching the structured query.
    async fn search(&self, query: &StructuredSearch, limit: usize) -> Result<SourceBatch>;

    /// Fetch a single article by its native ID.
    ///
    /// Returns `Ok(None)` if the source does not recognise the ID or no
    /// record is found.
    async fn fetch(&self, id: &str) -> Result<Option<Article>>;
}

// ---------------------------------------------------------------------------
// PubMed adapter (eutils)
// ---------------------------------------------------------------------------

/// [`LiteratureSource`] backed by NCBI E-utilities (PubMed).
pub struct PubmedSource {
    client: Arc<eutils::EutilsClient>,
}

impl PubmedSource {
    pub fn new(client: Arc<eutils::EutilsClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl LiteratureSource for PubmedSource {
    fn name(&self) -> &'static str {
        "pubmed"
    }

    async fn search(&self, query: &StructuredSearch, limit: usize) -> Result<SourceBatch> {
        // 1. Translate StructuredSearch → Entrez query string.
        let term = eutils::query::to_entrez(query)
            .map_err(|e| Error::Unknown(format!("entrez query translation: {e}")))?;

        // 2. ESearch for matching PMIDs.
        let search_resp = self
            .client
            .esearch(&eutils::types::ESearchRequest {
                db: "pubmed".into(),
                term,
                retmax: Some(limit as u32),
                retstart: None,
                sort: Some("relevance".into()),
                usehistory: None,
                web_env: None,
                query_key: None,
                datetype: None,
                reldate: None,
                mindate: None,
                maxdate: None,
            })
            .await
            .map_err(|e| Error::Unknown(format!("ESearch: {e}")))?;

        let total = search_resp
            .result
            .count
            .parse::<usize>()
            .unwrap_or(search_resp.result.id_list.len());

        if search_resp.result.id_list.is_empty() {
            return Ok(SourceBatch {
                source: self.name().into(),
                total: 0,
                articles: vec![],
            });
        }

        // 3. ESummary for metadata.
        let id_str = search_resp.result.id_list.join(",");
        let summary_json = self
            .client
            .esummary(&eutils::types::ESummaryRequest {
                db: "pubmed".into(),
                id: id_str,
                retmax: Some(limit as u32),
                retstart: None,
                version: Some("2.0".into()),
            })
            .await
            .map_err(|e| Error::Unknown(format!("ESummary: {e}")))?;

        // 4. Convert to Articles.
        let articles = eutils::esummary_to_articles(&summary_json);

        Ok(SourceBatch {
            source: self.name().into(),
            total,
            articles,
        })
    }

    async fn fetch(&self, id: &str) -> Result<Option<Article>> {
        // Determine the PMID: if the ID is numeric it's likely already a
        // PMID. Otherwise (DOI, etc.) resolve via ESearch.
        let pmid = if id.chars().all(|c| c.is_ascii_digit()) && !id.is_empty() {
            id.to_owned()
        } else {
            let term = if id.starts_with("10.") {
                format!("{id}[DOI]")
            } else {
                id.to_owned()
            };
            let resp = self
                .client
                .esearch(&eutils::types::ESearchRequest {
                    db: "pubmed".into(),
                    term,
                    retmax: Some(1),
                    retstart: None,
                    sort: None,
                    usehistory: None,
                    web_env: None,
                    query_key: None,
                    datetype: None,
                    reldate: None,
                    mindate: None,
                    maxdate: None,
                })
                .await
                .map_err(|e| Error::Unknown(format!("ESearch: {e}")))?;

            match resp.result.id_list.into_iter().next() {
                Some(p) => p,
                None => return Ok(None),
            }
        };

        let json = self
            .client
            .esummary(&eutils::types::ESummaryRequest {
                db: "pubmed".into(),
                id: pmid,
                retmax: Some(1),
                retstart: None,
                version: Some("2.0".into()),
            })
            .await
            .map_err(|e| Error::Unknown(format!("ESummary: {e}")))?;

        let articles = eutils::esummary_to_articles(&json);
        Ok(articles.into_iter().next())
    }
}

// ---------------------------------------------------------------------------
// arXiv adapter
// ---------------------------------------------------------------------------

/// [`LiteratureSource`] backed by the arXiv API.
pub struct ArxivSource {
    client: Arc<arxiv::ArxivClient>,
}

impl ArxivSource {
    pub fn new(client: Arc<arxiv::ArxivClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl LiteratureSource for ArxivSource {
    fn name(&self) -> &'static str {
        "arxiv"
    }

    async fn search(&self, query: &StructuredSearch, limit: usize) -> Result<SourceBatch> {
        let term = arxiv::query::to_arxiv(query)
            .map_err(|e| Error::Unknown(format!("arxiv query translation: {e}")))?;

        let resp = self
            .client
            .search(
                &arxiv::types::SearchRequest::new(term)
                    .max_results(limit as u32),
            )
            .await
            .map_err(|e| Error::Unknown(format!("arxiv search: {e}")))?;

        let total = resp.total_results as usize;
        let articles = arxiv::atom_to_articles(&resp.entries);

        Ok(SourceBatch {
            source: self.name().into(),
            total,
            articles,
        })
    }

    async fn fetch(&self, id: &str) -> Result<Option<Article>> {
        let resp = self
            .client
            .fetch_by_id(&arxiv::types::FetchRequest::new(id))
            .await
            .map_err(|e| Error::Unknown(format!("arxiv fetch: {e}")))?;

        let articles = arxiv::atom_to_articles(&resp.entries);
        Ok(articles.into_iter().next())
    }
}

// ---------------------------------------------------------------------------
// LiteratureGateway — multi-source dispatcher
// ---------------------------------------------------------------------------

/// Multi-source literature gateway.
///
/// Holds zero or more [`LiteratureSource`]s and dispatches searches to
/// all of them concurrently. Sources are tried in registration order
/// for [`Self::fetch`].
///
/// # Example
///
/// ```no_run
/// # use std::sync::Arc;
/// use bib_base::query::{LiteratureGateway, PubmedSource, ArxivSource};
///
/// let gateway = LiteratureGateway::new()
///     .with_source(Arc::new(PubmedSource::new(Arc::new(eutils::EutilsClient::from_env()))))
///     .with_source(Arc::new(ArxivSource::new(Arc::new(arxiv::ArxivClient::new()))));
/// ```
pub struct LiteratureGateway {
    sources: Vec<Arc<dyn LiteratureSource>>,
}

impl LiteratureGateway {
    /// Create an empty gateway (no sources).
    pub fn new() -> Self {
        Self { sources: Vec::new() }
    }

    /// Register a source.
    pub fn with_source(mut self, source: Arc<dyn LiteratureSource>) -> Self {
        self.sources.push(source);
        self
    }

    /// Register a source (mutable version for incremental building).
    pub fn add_source(&mut self, source: Arc<dyn LiteratureSource>) {
        self.sources.push(source);
    }

    /// Names of all registered sources.
    pub fn source_names(&self) -> Vec<&'static str> {
        self.sources.iter().map(|s| s.name()).collect()
    }

    /// Search across all registered sources concurrently.
    ///
    /// Each source's results are returned as a separate [`SourceBatch`].
    /// Sources that error are logged and skipped — one failing source
    /// does not abort the others.
    pub async fn search(
        &self,
        query: &StructuredSearch,
        limit: usize,
    ) -> Vec<SourceBatch> {
        let futures: Vec<_> = self
            .sources
            .iter()
            .map(|s| async { s.search(query, limit).await })
            .collect();

        let results = futures::future::join_all(futures).await;

        results
            .into_iter()
            .enumerate()
            .filter_map(|(i, res)| match res {
                Ok(batch) => Some(batch),
                Err(e) => {
                    tracing::warn!(
                        source = self.sources[i].name(),
                        error = %e,
                        "literature source search failed"
                    );
                    None
                }
            })
            .collect()
    }

    /// Fetch a single article by ID, trying each source in order.
    ///
    /// Returns the first hit. To try a specific source, use
    /// [`Self::fetch_from`].
    pub async fn fetch(&self, id: &str) -> Option<(String, Article)> {
        for source in &self.sources {
            match source.fetch(id).await {
                Ok(Some(article)) => return Some((source.name().to_owned(), article)),
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!(
                        source = source.name(),
                        error = %e,
                        "literature source fetch failed"
                    );
                    continue;
                }
            }
        }
        None
    }

    /// Fetch from a specific source by name.
    pub async fn fetch_from(&self, source_name: &str, id: &str) -> Result<Option<Article>> {
        let source = self
            .sources
            .iter()
            .find(|s| s.name() == source_name)
            .ok_or_else(|| Error::NotFound(format!("source '{source_name}' not registered")))?;
        source.fetch(id).await
    }
}

impl Default for LiteratureGateway {
    fn default() -> Self {
        Self::new()
    }
}
