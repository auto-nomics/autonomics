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

use bib_types::{Article, IdKind, Identifier};
use bib_types::query::StructuredSearch;

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

    /// Whether this source can handle the given [`IdKind`].
    ///
    /// During a typed [`LiteratureGateway::fetch`], sources that return
    /// `false` are skipped entirely — no network request is made. The
    /// default implementation returns `true` (accepts all kinds);
    /// override for precision so that, e.g., a DOI is not sent to arXiv.
    fn supports(&self, _kind: IdKind) -> bool {
        true
    }

    /// Fetch a single article by its typed [`Identifier`].
    ///
    /// Returns `Ok(None)` when:
    /// - the source does not support `id.kind` (also gated by
    ///   [`supports`](Self::supports)), or
    /// - no record exists for the given value.
    async fn fetch(&self, id: &Identifier) -> Result<Option<Article>>;
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

    fn supports(&self, kind: IdKind) -> bool {
        matches!(kind, IdKind::Pmid | IdKind::Doi)
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

        // 3. EFetch (MEDLINE) for full metadata **including abstracts**.
        //    ESummary is lighter but omits abstracts entirely — every
        //    article came back with abstract_text = None, making stored
        //    records unusable for reading or snippet search.
        let id_str = search_resp.result.id_list.join(",");
        let medline = self
            .client
            .efetch(&eutils::types::EFetchRequest {
                db: "pubmed".into(),
                id: id_str,
                rettype: Some("medline".into()),
                retmode: Some("text".into()),
                retmax: Some(limit as u32),
                retstart: None,
                web_env: None,
                query_key: None,
            })
            .await
            .map_err(|e| Error::Unknown(format!("EFetch: {e}")))?;

        // 4. Convert to Articles.
        let articles = eutils::medline_to_articles(&medline);

        Ok(SourceBatch {
            source: self.name().into(),
            total,
            articles,
        })
    }

    async fn fetch(&self, id: &Identifier) -> Result<Option<Article>> {
        // Resolve to a PMID: a Pmid is used directly; a Doi is resolved
        // via ESearch. Other kinds are not supported by PubMed.
        let pmid = match id.kind {
            IdKind::Pmid => id.value.clone(),
            IdKind::Doi => {
                let term = format!("{}[DOI]", id.value);
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
            }
            _ => return Ok(None),
        };

        let medline = self
            .client
            .efetch(&eutils::types::EFetchRequest {
                db: "pubmed".into(),
                id: pmid,
                rettype: Some("medline".into()),
                retmode: Some("text".into()),
                retmax: Some(1),
                retstart: None,
                web_env: None,
                query_key: None,
            })
            .await
            .map_err(|e| Error::Unknown(format!("EFetch: {e}")))?;

        let articles = eutils::medline_to_articles(&medline);
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

    fn supports(&self, kind: IdKind) -> bool {
        matches!(kind, IdKind::Arxiv)
    }

    async fn search(&self, query: &StructuredSearch, limit: usize) -> Result<SourceBatch> {
        let term = arxiv::query::to_arxiv(query)
            .map_err(|e| Error::Unknown(format!("arxiv query translation: {e}")))?;

        let resp = self
            .client
            .search(&arxiv::types::SearchRequest::new(term).max_results(limit as u32))
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

    async fn fetch(&self, id: &Identifier) -> Result<Option<Article>> {
        if id.kind != IdKind::Arxiv {
            return Ok(None);
        }
        let resp = self
            .client
            .fetch_by_id(&arxiv::types::FetchRequest::new(&id.value))
            .await
            .map_err(|e| Error::Unknown(format!("arxiv fetch: {e}")))?;

        let articles = arxiv::atom_to_articles(&resp.entries);
        Ok(articles.into_iter().next())
    }
}

// ---------------------------------------------------------------------------
// bioRxiv / medRxiv adapter
// ---------------------------------------------------------------------------

/// DOI prefix shared by both bioRxiv and medRxiv.
const BIORXIV_DOI_PREFIX: &str = "10.1101/";

/// [`LiteratureSource`] backed by the bioRxiv details API.
///
/// Both bioRxiv and medRxiv share the same API at `api.biorxiv.org` and
/// the same DOI prefix (`10.1101/`). This adapter handles both servers:
/// fetch tries `biorxiv` first, then `medrxiv`.
///
/// **Limitation**: the bioRxiv API has no keyword-search endpoint, so
/// [`search`](LiteratureSource::search) always returns an empty batch.
/// Use [`fetch`](LiteratureSource::fetch) with a DOI, or search via
/// PubMed (which indexes many preprints) instead.
pub struct BiorxivSource {
    /// Shared `reqwest::Client`. Held as `Arc` so the same connection
    /// pool can be reused by every `BiorxivSource` (and every agent) in
    /// the process. Construct via [`BiorxivSource::with_client`] in
    /// production code; the `new()` zero-arg constructor exists for
    /// backwards compatibility and test fixtures.
    client: Arc<reqwest::Client>,
}

impl Default for BiorxivSource {
    fn default() -> Self {
        Self::new()
    }
}

impl BiorxivSource {
    /// Build a `BiorxivSource` with a fresh, private `reqwest::Client`.
    /// Prefer [`BiorxivSource::with_client`] in production so the
    /// process shares one connection pool.
    pub fn new() -> Self {
        Self::with_client(Arc::new(
            reqwest::Client::builder()
                .user_agent("autonomics-bib-base")
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        ))
    }

    /// Build a `BiorxivSource` backed by a caller-supplied
    /// `reqwest::Client`. Typically called with the
    /// [`crate::BibShared::http`] handle so every agent in a multi-agent
    /// network reuses the same connection pool.
    pub fn with_client(client: Arc<reqwest::Client>) -> Self {
        Self { client }
    }

    /// Fetch paper details from one server (`"biorxiv"` or `"medrxiv"`).
    async fn fetch_from_server(&self, server: &str, doi: &str) -> Result<Option<Article>> {
        let url = format!("https://api.biorxiv.org/details/{server}/{doi}");
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| Error::Unknown(format!("bioRxiv API ({server}): {e}")))?;

        if !resp.status().is_success() {
            return Ok(None);
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| Error::Unknown(format!("bioRxiv JSON parse: {e}")))?;

        let entry = match json
            .get("collection")
            .and_then(|c| c.as_array())
            .filter(|a| !a.is_empty())
            .and_then(|a| a.first())
        {
            Some(e) => e,
            None => return Ok(None),
        };

        Ok(Some(entry_to_article(entry, server)))
    }
}

#[async_trait]
impl LiteratureSource for BiorxivSource {
    fn name(&self) -> &'static str {
        "biorxiv"
    }

    fn supports(&self, kind: IdKind) -> bool {
        matches!(kind, IdKind::Doi | IdKind::Biorxiv)
    }

    async fn search(&self, _query: &StructuredSearch, _limit: usize) -> Result<SourceBatch> {
        // The bioRxiv public API has no keyword-search endpoint.
        Ok(SourceBatch {
            source: self.name().into(),
            total: 0,
            articles: vec![],
        })
    }

    async fn fetch(&self, id: &Identifier) -> Result<Option<Article>> {
        let doi = match id.kind {
            IdKind::Biorxiv | IdKind::Doi => &id.value,
            _ => return Ok(None),
        };

        // Only handle bioRxiv/medRxiv DOIs.
        if !doi.starts_with(BIORXIV_DOI_PREFIX) {
            return Ok(None);
        }

        // Try biorxiv server first, then medrxiv.
        for server in &["biorxiv", "medrxiv"] {
            if let Some(article) = self.fetch_from_server(server, doi).await? {
                return Ok(Some(article));
            }
        }
        Ok(None)
    }
}

/// Parse a bioRxiv API JSON entry into an [`Article`].
fn entry_to_article(entry: &serde_json::Value, server: &str) -> Article {
    use bib_types::convert::{build_article, normalize_doi};
    use bib_types::{ArticleSource, Author, IdKind, Identifier};

    let doi = str_field(entry, "doi");
    let title = str_field(entry, "title");
    let abstract_text = str_field(entry, "abstract");
    let date = str_field(entry, "date");
    let category = str_field(entry, "category");
    let version = str_field(entry, "version");

    // Identifiers: DOI (tagged as biorxiv) + version.
    let mut identifiers = vec![Identifier::new(IdKind::Biorxiv, &doi)];
    // Also keep a clean DOI identifier for cross-source dedup.
    let normalized = normalize_doi(&doi);
    if !normalized.is_empty() {
        identifiers.push(Identifier::doi(normalized));
    }

    // Authors — semicolon-separated: "Smith J;Jones B;"
    let authors_str = str_field(entry, "authors");
    let authors: Vec<Author> = authors_str
        .split(';')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(parse_biorxiv_author)
        .collect();

    let mut article = build_article(
        &doi,
        title.trim(),
        identifiers,
        authors,
        ArticleSource::Biorxiv,
    );

    article.abstract_text = if abstract_text.is_empty() {
        None
    } else {
        Some(abstract_text)
    };

    // Parse date "2024-01-15" → (year, month).
    let (year, month) = parse_biorxiv_date(&date);
    article.year = year;
    article.month = month;

    // Category as pub_type + keyword.
    if !category.is_empty() {
        article.pub_types.push(category.clone());
        article.keywords.push(category);
    }

    // Version as a keyword.
    if !version.is_empty() {
        article.keywords.push(format!("v{version}"));
    }

    // Journal: record the server (bioRxiv or medRxiv).
    article.journal = Some(format!("{server} preprint"));

    article
}

/// Parse a bioRxiv author string (typically "Smith J" — last name + initials).
fn parse_biorxiv_author(name: &str) -> bib_types::Author {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return bib_types::Author {
            last_name: String::new(),
            fore_name: None,
            initials: None,
            affiliation: None,
            orcid: None,
            corresponding: false,
        };
    }

    match trimmed.rsplit_once(' ') {
        Some((given, family)) if !family.is_empty() => {
            let initials: String = given
                .split_whitespace()
                .filter_map(|w| w.chars().next())
                .collect();
            bib_types::Author {
                last_name: family.to_owned(),
                fore_name: Some(given.to_owned()),
                initials: if !initials.is_empty() {
                    Some(initials)
                } else {
                    None
                },
                affiliation: None,
                orcid: None,
                corresponding: false,
            }
        }
        _ => bib_types::Author {
            last_name: trimmed.to_owned(),
            fore_name: None,
            initials: None,
            affiliation: None,
            orcid: None,
            corresponding: false,
        },
    }
}

/// Parse a bioRxiv date string ("2024-01-15") into (year, month).
fn parse_biorxiv_date(s: &str) -> (Option<u16>, Option<u8>) {
    let parts: Vec<&str> = s.split('-').collect();
    let year = parts.first().and_then(|p| p.parse::<u16>().ok());
    let month = parts.get(1).and_then(|p| p.parse::<u8>().ok());
    (year, month)
}

fn str_field(v: &serde_json::Value, key: &str) -> String {
    v.get(key).and_then(|v| v.as_str()).unwrap_or("").to_owned()
}

// ---------------------------------------------------------------------------
// OpenAlex adapter
// ---------------------------------------------------------------------------

/// [`LiteratureSource`] backed by the [OpenAlex REST API](https://api.openalex.org).
///
/// Maps [`StructuredSearch`] → OpenAlex filter syntax via
/// [`openalex::query::to_openalex_filter`], calls `/works`, and converts each
/// [`openalex::types::Work`] into a canonical [`Article`] via
/// [`openalex::work_to_article`].
///
/// The `search` and `fetch` capabilities (works search + get-by-id) are
/// surfaced through the [`LiteratureGateway`]. OpenAlex features that do not
/// fit the [`LiteratureSource`] contract (cross-entity autocomplete) are
/// registered as standalone agent tools via
/// [`bib_extended_registrations`](crate::bib_extended_registrations).
pub struct OpenAlexSource {
    client: Arc<openalex::OpenAlexClient>,
}

impl OpenAlexSource {
    pub fn new(client: Arc<openalex::OpenAlexClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl LiteratureSource for OpenAlexSource {
    fn name(&self) -> &'static str {
        "openalex"
    }

    fn supports(&self, kind: IdKind) -> bool {
        matches!(kind, IdKind::OpenAlex | IdKind::Doi | IdKind::Pmid)
    }

    async fn search(&self, query: &StructuredSearch, limit: usize) -> Result<SourceBatch> {
        let filter = openalex::query::to_openalex_filter(query)
            .map_err(|e| Error::Unknown(format!("openalex query translation: {e}")))?;
        let params = openalex::ListParams::new()
            .with_filter(&filter)
            .with_per_page((limit as u32).clamp(1, 200));
        let resp = self
            .client
            .list_works(&params)
            .await
            .map_err(|e| Error::Unknown(format!("openalex search: {e}")))?;
        let total = resp.meta.count as usize;
        let articles: Vec<Article> = resp.results.iter().map(openalex::work_to_article).collect();
        Ok(SourceBatch {
            source: self.name().into(),
            total,
            articles,
        })
    }

    async fn fetch(&self, id: &Identifier) -> Result<Option<Article>> {
        // Translate the typed identifier into an OpenAlex API ID string.
        let api_id = match id.kind {
            IdKind::OpenAlex => id.value.clone(),
            IdKind::Doi => format!("doi:{}", id.value),
            IdKind::Pmid => format!("pmid:{}", id.value),
            _ => return Ok(None),
        };
        match self.client.get_work(&api_id).await {
            Ok(work) => Ok(Some(openalex::work_to_article(&work))),
            // OpenAlex signals unknown IDs with a 404 → NotFound.
            Err(openalex::OpenAlexError::NotFound(_)) => Ok(None),
            Err(e) => Err(Error::Unknown(format!("openalex fetch: {e}"))),
        }
    }
}

// ---------------------------------------------------------------------------
// Crossref adapter
// ---------------------------------------------------------------------------

/// [`LiteratureSource`] backed by the [Crossref REST API](https://api.crossref.org).
///
/// Maps [`StructuredSearch`] → a [`crossref::client::WorksQuery`] via
/// [`crossref::query::to_crossref_works_query`], calls `GET /works`, and
/// converts each [`crossref::types::Work`] into a canonical [`Article`] via
/// [`crossref::work_to_article`]. `fetch` retrieves a single work by DOI.
///
/// Crossref features that do not fit the [`LiteratureSource`] contract
/// (the type catalogue) are registered as standalone agent tools via
/// [`bib_extended_registrations`](crate::bib_extended_registrations).
pub struct CrossrefSource {
    client: Arc<crossref::CrossrefClient>,
}

impl CrossrefSource {
    pub fn new(client: Arc<crossref::CrossrefClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl LiteratureSource for CrossrefSource {
    fn name(&self) -> &'static str {
        "crossref"
    }

    fn supports(&self, kind: IdKind) -> bool {
        matches!(kind, IdKind::Doi)
    }

    async fn search(&self, query: &StructuredSearch, limit: usize) -> Result<SourceBatch> {
        let q = crossref::query::to_crossref_works_query(query)
            .map_err(|e| Error::Unknown(format!("crossref query translation: {e}")))?
            .with_rows((limit as u32).clamp(1, 100));
        let resp = self
            .client
            .works(&q)
            .await
            .map_err(|e| Error::Unknown(format!("crossref search: {e}")))?;
        let total = resp.message.total_results as usize;
        let articles: Vec<Article> = resp
            .message
            .items
            .iter()
            .map(crossref::work_to_article)
            .collect();
        Ok(SourceBatch {
            source: self.name().into(),
            total,
            articles,
        })
    }

    async fn fetch(&self, id: &Identifier) -> Result<Option<Article>> {
        if id.kind != IdKind::Doi {
            return Ok(None);
        }
        match self.client.works_by_doi(&id.value).await {
            Ok(resp) => Ok(resp.message.map(|w| crossref::work_to_article(&w))),
            // Crossref signals unknown DOIs with HTTP 404.
            Err(crossref::CrossrefError::Status { status: 404, .. }) => Ok(None),
            Err(e) => Err(Error::Unknown(format!("crossref fetch: {e}"))),
        }
    }
}

// ---------------------------------------------------------------------------
// Semantic Scholar adapter
// ---------------------------------------------------------------------------

/// [`LiteratureSource`] backed by the [Semantic Scholar Graph API](
/// https://api.semanticscholar.org).
///
/// Maps [`StructuredSearch`] → an S2 query + filter set via
/// [`semantic_scholar::query::to_s2`], calls `/paper/search`, and converts
/// each [`semantic_scholar::Paper`] into a canonical [`Article`] via
/// [`semantic_scholar::paper_to_article`].
///
/// S2 features that do not fit the [`LiteratureSource`] contract (citation
/// graph traversal, recommendations, author lookup) are registered as
/// standalone agent tools via
/// [`bib_extended_registrations`](crate::bib_extended_registrations).
pub struct S2Source {
    client: Arc<semantic_scholar::S2Client>,
}

impl S2Source {
    pub fn new(client: Arc<semantic_scholar::S2Client>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl LiteratureSource for S2Source {
    fn name(&self) -> &'static str {
        "semantic_scholar"
    }

    fn supports(&self, kind: IdKind) -> bool {
        matches!(
            kind,
            IdKind::S2 | IdKind::Doi | IdKind::Arxiv | IdKind::Pmid
        )
    }

    async fn search(&self, query: &StructuredSearch, limit: usize) -> Result<SourceBatch> {
        let parts = semantic_scholar::query::to_s2(query)
            .map_err(|e| Error::Unknown(format!("s2 query translation: {e}")))?;
        let resp = self
            .client
            .search_paper_filtered(
                &parts.query,
                (limit as u32).clamp(1, 100),
                0,
                &parts.filter,
                None,
            )
            .await
            .map_err(|e| Error::Unknown(format!("s2 search: {e}")))?;
        let total = resp.total.max(0) as usize;
        let articles: Vec<Article> = resp
            .data
            .iter()
            .map(semantic_scholar::paper_to_article)
            .collect();
        Ok(SourceBatch {
            source: self.name().into(),
            total,
            articles,
        })
    }

    async fn fetch(&self, id: &Identifier) -> Result<Option<Article>> {
        // S2 accepts prefixed IDs for cross-system lookups.
        let api_id = match id.kind {
            IdKind::S2 => id.value.clone(),
            IdKind::Doi => format!("DOI:{}", id.value),
            IdKind::Arxiv => format!("ARXIV:{}", id.value),
            IdKind::Pmid => format!("PMID:{}", id.value),
            _ => return Ok(None),
        };
        match self.client.get_paper(&api_id, None).await {
            Ok(paper) => Ok(Some(semantic_scholar::paper_to_article(&paper))),
            // S2 signals unknown paper IDs with HTTP 404.
            Err(semantic_scholar::S2Error::Status { status: 404, .. }) => Ok(None),
            Err(e) => Err(Error::Unknown(format!("s2 fetch: {e}"))),
        }
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
        Self {
            sources: Vec::new(),
        }
    }

    /// Create a gateway pre-loaded with all built-in literature sources:
    /// PubMed (via NCBI E-utilities), arXiv, and bioRxiv/medRxiv.
    ///
    /// **Each call constructs its own `EutilsClient` / `ArxivClient` /
    /// `reqwest::Client`**, so use this only when a single agent needs
    /// its own private clients (e.g. tests, or when you don't have a
    /// [`crate::BibShared`] available). In a multi-agent host prefer
    /// [`with_shared_clients`] so every agent shares one connection
    /// pool and one arXiv rate-limit window.
    pub fn with_default_sources() -> Self {
        Self::new()
            .with_source(Arc::new(PubmedSource::new(Arc::new(
                eutils::EutilsClient::from_env(),
            ))))
            .with_source(Arc::new(ArxivSource::new(Arc::new(
                arxiv::ArxivClient::new(),
            ))))
            .with_source(Arc::new(BiorxivSource::new()))
    }

    /// Create a gateway pre-loaded with all built-in literature sources
    /// backed by caller-supplied shared clients. This is the constructor
    /// the multi-agent host uses: every agent spawned from the same
    /// [`crate::BibShared`] shares one `EutilsClient`, one
    /// `ArxivClient`, and one `reqwest::Client`, so the process has
    /// exactly one connection pool and one arXiv rate-limit window
    /// regardless of agent count.
    pub fn with_shared_clients(
        eutils: Arc<eutils::EutilsClient>,
        arxiv: Arc<arxiv::ArxivClient>,
        http: Arc<reqwest::Client>,
    ) -> Self {
        Self::new()
            .with_source(Arc::new(PubmedSource::new(eutils)))
            .with_source(Arc::new(ArxivSource::new(arxiv)))
            .with_source(Arc::new(BiorxivSource::with_client(http)))
    }

    /// Like [`with_shared_clients`](Self::with_shared_clients) but also
    /// loads the OpenAlex, Crossref, and Semantic Scholar sources.
    ///
    /// This is the constructor used by [`BibShared`](crate::BibShared) when
    /// all six built-in sources are available.
    pub fn with_all_shared_clients(
        eutils: Arc<eutils::EutilsClient>,
        arxiv: Arc<arxiv::ArxivClient>,
        http: Arc<reqwest::Client>,
        openalex_client: Arc<openalex::OpenAlexClient>,
        crossref_client: Arc<crossref::CrossrefClient>,
        s2_client: Arc<semantic_scholar::S2Client>,
    ) -> Self {
        Self::with_shared_clients(eutils, arxiv, http)
            .with_source(Arc::new(OpenAlexSource::new(openalex_client)))
            .with_source(Arc::new(CrossrefSource::new(crossref_client)))
            .with_source(Arc::new(S2Source::new(s2_client)))
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
    pub async fn search(&self, query: &StructuredSearch, limit: usize) -> Vec<SourceBatch> {
        self.search_subset(&self.sources.iter().collect::<Vec<_>>(), query, limit)
            .await
    }

    /// Search a **single** source by name.
    pub async fn search_from(
        &self,
        source_name: &str,
        query: &StructuredSearch,
        limit: usize,
    ) -> Result<SourceBatch> {
        let source = self
            .sources
            .iter()
            .find(|s| s.name() == source_name)
            .ok_or_else(|| Error::NotFound(format!("source '{source_name}' not registered")))?;
        source.search(query, limit).await
    }

    /// Search a **subset** of sources by name, concurrently.
    ///
    /// Unrecognised names are silently skipped. If `names` is `None`,
    /// searches all registered sources (equivalent to [`Self::search`]).
    pub async fn search_named(
        &self,
        names: Option<&[String]>,
        query: &StructuredSearch,
        limit: usize,
    ) -> Vec<SourceBatch> {
        let selected: Vec<_> = match names {
            Some(names) => self
                .sources
                .iter()
                .filter(|s| names.iter().any(|n| n == s.name()))
                .collect(),
            None => self.sources.iter().collect(),
        };
        self.search_subset(&selected, query, limit).await
    }

    /// Internal: dispatch search to a set of source references concurrently.
    async fn search_subset(
        &self,
        sources: &[&Arc<dyn LiteratureSource>],
        query: &StructuredSearch,
        limit: usize,
    ) -> Vec<SourceBatch> {
        let futures: Vec<_> = sources
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
                        source = sources[i].name(),
                        error = %e,
                        "literature source search failed"
                    );
                    None
                }
            })
            .collect()
    }

    /// Fetch a single article by typed [`Identifier`].
    ///
    /// Only sources that [`support`](LiteratureSource::supports) the
    /// identifier's [`IdKind`] are queried, in registration order. The
    /// first hit wins. To target a specific source, use [`Self::fetch_from`].
    pub async fn fetch(&self, id: &Identifier) -> Option<(String, Article)> {
        for source in &self.sources {
            if !source.supports(id.kind) {
                continue;
            }
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
    pub async fn fetch_from(
        &self,
        source_name: &str,
        id: &Identifier,
    ) -> Result<Option<Article>> {
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
