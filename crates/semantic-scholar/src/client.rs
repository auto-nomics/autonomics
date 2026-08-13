use reqwest::Client;

use crate::error::{Result, S2Error};
use crate::types::*;

/// Base URL for the Semantic Scholar Academic Graph API.
const GRAPH_BASE: &str = "https://api.semanticscholar.org/graph/v1";

/// Base URL for the Semantic Scholar Recommendations API.
const RECO_BASE: &str = "https://api.semanticscholar.org/recommendations/v1";

/// Resolve the S2 Graph endpoint via the resource catalog.
fn graph_base() -> String {
    resource_catalog::endpoint_or("endpoint.s2_graph", GRAPH_BASE)
}

/// Resolve the S2 Recommendations endpoint via the resource catalog.
fn reco_base() -> String {
    resource_catalog::endpoint_or("endpoint.s2_reco", RECO_BASE)
}

// ===========================================================================
// Client
// ===========================================================================

/// An async client for the [Semantic Scholar Academic Graph API](
/// https://www.semanticscholar.org/product/api).
///
/// The API is free and open. Without an API key, requests share a pool-wide
/// rate limit. With a key (obtained from Semantic Scholar), the limit is
/// 1 request/second for the introductory tier.
///
/// # Endpoints covered
///
/// | Method | API path |
/// |--------|----------|
/// | [`search_paper`](Self::search_paper) | `GET /paper/search` |
/// | [`search_paper_bulk`](Self::search_paper_bulk) | `GET /paper/search/bulk` |
/// | [`get_paper`](Self::get_paper) | `GET /paper/{paper_id}` |
/// | [`get_papers_batch`](Self::get_papers_batch) | `POST /paper/batch` |
/// | [`get_citations`](Self::get_citations) | `GET /paper/{paper_id}/citations` |
/// | [`get_references`](Self::get_references) | `GET /paper/{paper_id}/references` |
/// | [`autocomplete`](Self::autocomplete) | `GET /paper/autocomplete` |
/// | [`search_authors`](Self::search_authors) | `GET /author/search` |
/// | [`get_author`](Self::get_author) | `GET /author/{author_id}` |
/// | [`get_author_papers`](Self::get_author_papers) | `GET /author/{author_id}/papers` |
/// | [`recommendations`](Self::recommendations) | `GET /recommendations/v1/papers/forpaper/{paper_id}` |
///
/// # Example
///
/// ```no_run
/// use semantic_scholar::S2Client;
///
/// let client = S2Client::new();
/// ```
#[derive(Debug, Clone)]
pub struct S2Client {
    client: Client,
    api_key: Option<String>,
}

impl Default for S2Client {
    fn default() -> Self {
        Self::new()
    }
}

impl S2Client {
    /// Create a new client with default settings (no API key).
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .user_agent("semantic-scholar-rs-sdk/0.1 (+https://www.semanticscholar.org)")
                .build()
                .expect("reqwest client builder"),
            api_key: None,
        }
    }

    /// Provide an API key for higher rate limits.
    ///
    /// The key is sent as the `x-api-key` header on every request.
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// Provide a custom `reqwest::Client` (e.g. for timeouts, proxy).
    pub fn with_client(client: Client) -> Self {
        Self {
            client,
            api_key: None,
        }
    }

    // -----------------------------------------------------------------------
    // Core request helpers
    // -----------------------------------------------------------------------

    /// Maximum number of retries on HTTP 429.
    const MAX_RETRIES: u32 = 5;

    /// Initial backoff delay for 429 retries (milliseconds).
    const INITIAL_BACKOFF_MS: u64 = 2000;

    /// Execute a GET request that returns JSON, with automatic retry on 429.
    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        base: &str,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<T> {
        let url = build_url(base, path, params);
        let mut backoff = Self::INITIAL_BACKOFF_MS;
        for attempt in 0..=Self::MAX_RETRIES {
            let mut req = self.client.get(&url);
            if let Some(ref key) = self.api_key {
                req = req.header("x-api-key", key);
            }
            let resp = req.send().await?;
            let status = resp.status().as_u16();
            let body = resp.text().await?;
            if status == 429 && attempt < Self::MAX_RETRIES {
                tokio::time::sleep(std::time::Duration::from_millis(backoff)).await;
                backoff *= 2;
                continue;
            }
            if !(200..300).contains(&status) {
                return Err(S2Error::Status { status, body });
            }
            return serde_json::from_str(&body).map_err(Into::into);
        }
        unreachable!()
    }

    /// Execute a POST request with a JSON body that returns JSON, with
    /// automatic retry on 429.
    async fn post_json<T: serde::de::DeserializeOwned>(
        &self,
        base: &str,
        path: &str,
        params: &[(&str, String)],
        body: &impl serde::Serialize,
    ) -> Result<T> {
        let url = build_url(base, path, params);
        let mut backoff = Self::INITIAL_BACKOFF_MS;
        for attempt in 0..=Self::MAX_RETRIES {
            let mut req = self.client.post(&url).json(body);
            if let Some(ref key) = self.api_key {
                req = req.header("x-api-key", key);
            }
            let resp = req.send().await?;
            let status = resp.status().as_u16();
            let body_text = resp.text().await?;
            if status == 429 && attempt < Self::MAX_RETRIES {
                tokio::time::sleep(std::time::Duration::from_millis(backoff)).await;
                backoff *= 2;
                continue;
            }
            if !(200..300).contains(&status) {
                return Err(S2Error::Status {
                    status,
                    body: body_text,
                });
            }
            return serde_json::from_str(&body_text).map_err(Into::into);
        }
        unreachable!()
    }

    // ===================================================================
    // Paper search
    // ===================================================================

    /// Relevance search for papers.
    ///
    /// `query` is a plain-text string matched against the paper's title and
    /// abstract. No special query syntax is supported (use
    /// [`search_paper_bulk`](Self::search_paper_bulk) for boolean queries).
    ///
    /// `limit` must be ≤ 100. `fields` defaults to [`DEFAULT_PAPER_FIELDS`].
    pub async fn search_paper(
        &self,
        query: &str,
        limit: u32,
        fields: Option<&str>,
    ) -> Result<PaperSearchResponse> {
        let mut params: Vec<(&str, String)> = vec![
            ("query", query.to_owned()),
            ("limit", limit.min(100).to_string()),
        ];
        if let Some(f) = fields {
            params.push(("fields", f.to_owned()));
        } else {
            params.push(("fields", DEFAULT_PAPER_FIELDS.to_owned()));
        }
        self.get_json(&graph_base(), "/paper/search", &params).await
    }

    /// Relevance search with full filter support.
    ///
    /// Pass filters via [`PaperSearchFilter`].
    pub async fn search_paper_filtered(
        &self,
        query: &str,
        limit: u32,
        offset: u32,
        filter: &PaperSearchFilter,
        fields: Option<&str>,
    ) -> Result<PaperSearchResponse> {
        let mut params: Vec<(&str, String)> = vec![
            ("query", query.to_owned()),
            ("limit", limit.min(100).to_string()),
            ("offset", offset.to_string()),
        ];
        if let Some(f) = fields {
            params.push(("fields", f.to_owned()));
        } else {
            params.push(("fields", DEFAULT_PAPER_FIELDS.to_owned()));
        }
        if let Some(ref y) = filter.year {
            params.push(("year", y.clone()));
        }
        if let Some(ref v) = filter.venue {
            params.push(("venue", v.clone()));
        }
        if let Some(ref fos) = filter.fields_of_study {
            params.push(("fieldsOfStudy", fos.clone()));
        }
        if let Some(ref pt) = filter.publication_types {
            params.push(("publicationTypes", pt.clone()));
        }
        if filter.open_access_pdf {
            params.push(("openAccessPdf", String::new()));
        }
        if let Some(mc) = filter.min_citation_count {
            params.push(("minCitationCount", mc.to_string()));
        }
        self.get_json(&graph_base(), "/paper/search", &params).await
    }

    /// Bulk search with boolean query syntax and continuation token.
    ///
    /// Supports `+` (AND), `|` (OR), `-` (negate), `"` (phrase), `*` (prefix),
    /// `~N` (fuzzy). Up to 1,000 results per call. Use the returned `token`
    /// to continue fetching.
    pub async fn search_paper_bulk(
        &self,
        query: &str,
        token: Option<&str>,
        fields: Option<&str>,
        sort: Option<&str>,
    ) -> Result<PaperBulkResponse> {
        let mut params: Vec<(&str, String)> = vec![("query", query.to_owned())];
        if let Some(t) = token {
            params.push(("token", t.to_owned()));
        }
        if let Some(f) = fields {
            params.push(("fields", f.to_owned()));
        } else {
            params.push(("fields", BULK_PAPER_FIELDS.to_owned()));
        }
        if let Some(s) = sort {
            params.push(("sort", s.to_owned()));
        }
        self.get_json(&graph_base(), "/paper/search/bulk", &params)
            .await
    }

    // ===================================================================
    // Paper details
    // ===================================================================

    /// Get details about a single paper.
    ///
    /// `paper_id` accepts: S2 SHA, `CorpusId:<id>`, `DOI:<doi>`,
    /// `ARXIV:<id>`, `PMID:<id>`, `PMCID:<id>`, `ACL:<id>`, `MAG:<id>`,
    /// `URL:<url>`.
    pub async fn get_paper(&self, paper_id: &str, fields: Option<&str>) -> Result<Paper> {
        let path = format!("/paper/{}", urlencode(paper_id));
        let mut params: Vec<(&str, String)> = Vec::new();
        if let Some(f) = fields {
            params.push(("fields", f.to_owned()));
        } else {
            params.push(("fields", DEFAULT_PAPER_FIELDS.to_owned()));
        }
        self.get_json(&graph_base(), &path, &params).await
    }

    /// Get details for multiple papers (up to 500) in a single request.
    pub async fn get_papers_batch(
        &self,
        ids: &[&str],
        fields: Option<&str>,
    ) -> Result<Vec<Option<Paper>>> {
        let mut params: Vec<(&str, String)> = Vec::new();
        if let Some(f) = fields {
            params.push(("fields", f.to_owned()));
        } else {
            params.push(("fields", DEFAULT_PAPER_FIELDS.to_owned()));
        }
        let body = serde_json::json!({ "ids": ids });
        self.post_json(&graph_base(), "/paper/batch", &params, &body)
            .await
    }

    // ===================================================================
    // Citations / References
    // ===================================================================

    /// Get papers that cite this paper.
    pub async fn get_citations(
        &self,
        paper_id: &str,
        limit: u32,
        offset: u32,
        fields: Option<&str>,
    ) -> Result<CitationBatchResponse> {
        let path = format!("/paper/{}/citations", urlencode(paper_id));
        let mut params: Vec<(&str, String)> = vec![
            ("limit", limit.min(1000).to_string()),
            ("offset", offset.to_string()),
        ];
        if let Some(f) = fields {
            params.push(("fields", f.to_owned()));
        } else {
            params.push(("fields", DEFAULT_CITATION_FIELDS.to_owned()));
        }
        self.get_json(&graph_base(), &path, &params).await
    }

    /// Get papers referenced by this paper.
    pub async fn get_references(
        &self,
        paper_id: &str,
        limit: u32,
        offset: u32,
        fields: Option<&str>,
    ) -> Result<ReferenceBatchResponse> {
        let path = format!("/paper/{}/references", urlencode(paper_id));
        let mut params: Vec<(&str, String)> = vec![
            ("limit", limit.min(1000).to_string()),
            ("offset", offset.to_string()),
        ];
        if let Some(f) = fields {
            params.push(("fields", f.to_owned()));
        } else {
            params.push(("fields", DEFAULT_CITATION_FIELDS.to_owned()));
        }
        self.get_json(&graph_base(), &path, &params).await
    }

    // ===================================================================
    // Autocomplete
    // ===================================================================

    /// Suggest paper query completions for interactive search.
    pub async fn autocomplete(&self, query: &str) -> Result<serde_json::Value> {
        let params: Vec<(&str, String)> = vec![("query", query.to_owned())];
        self.get_json(&graph_base(), "/paper/autocomplete", &params)
            .await
    }

    // ===================================================================
    // Author search
    // ===================================================================

    /// Search for authors by name.
    pub async fn search_authors(
        &self,
        query: &str,
        limit: u32,
        offset: u32,
        fields: Option<&str>,
    ) -> Result<AuthorSearchResponse> {
        let mut params: Vec<(&str, String)> = vec![
            ("query", query.to_owned()),
            ("limit", limit.min(1000).to_string()),
            ("offset", offset.to_string()),
        ];
        if let Some(f) = fields {
            params.push(("fields", f.to_owned()));
        } else {
            params.push(("fields", DEFAULT_AUTHOR_FIELDS.to_owned()));
        }
        self.get_json(&graph_base(), "/author/search", &params)
            .await
    }

    /// Get details about a single author.
    pub async fn get_author(&self, author_id: &str, fields: Option<&str>) -> Result<Author> {
        let path = format!("/author/{}", urlencode(author_id));
        let mut params: Vec<(&str, String)> = Vec::new();
        if let Some(f) = fields {
            params.push(("fields", f.to_owned()));
        } else {
            params.push(("fields", DEFAULT_AUTHOR_FIELDS.to_owned()));
        }
        self.get_json(&graph_base(), &path, &params).await
    }

    /// Get papers by a specific author.
    pub async fn get_author_papers(
        &self,
        author_id: &str,
        limit: u32,
        offset: u32,
        fields: Option<&str>,
    ) -> Result<AuthorPapersResponse> {
        let path = format!("/author/{}/papers", urlencode(author_id));
        let mut params: Vec<(&str, String)> = vec![
            ("limit", limit.min(1000).to_string()),
            ("offset", offset.to_string()),
        ];
        if let Some(f) = fields {
            params.push(("fields", f.to_owned()));
        } else {
            params.push(("fields", AUTHOR_PAPER_FIELDS.to_owned()));
        }
        self.get_json(&graph_base(), &path, &params).await
    }

    // ===================================================================
    // Recommendations
    // ===================================================================

    /// Get recommended papers similar to a given paper.
    pub async fn recommendations(
        &self,
        paper_id: &str,
        limit: u32,
        fields: Option<&str>,
    ) -> Result<RecommendationsResponse> {
        let path = format!("/papers/forpaper/{}", urlencode(paper_id));
        let mut params: Vec<(&str, String)> = vec![("limit", limit.min(500).to_string())];
        if let Some(f) = fields {
            params.push(("fields", f.to_owned()));
        } else {
            params.push(("fields", DEFAULT_PAPER_FIELDS.to_owned()));
        }
        self.get_json(&reco_base(), &path, &params).await
    }
}

// ===========================================================================
// Filter type
// ===========================================================================

/// Filters for paper relevance search.
#[derive(Debug, Clone, Default)]
pub struct PaperSearchFilter {
    /// Year range, e.g. `"2020-2023"`, `"2019"`, `"2010-"`.
    pub year: Option<String>,
    /// Venue filter (comma-separated for OR).
    pub venue: Option<String>,
    /// Fields of study (comma-separated for OR).
    pub fields_of_study: Option<String>,
    /// Publication types (comma-separated).
    pub publication_types: Option<String>,
    /// Only papers with open access PDF.
    pub open_access_pdf: bool,
    /// Minimum citation count.
    pub min_citation_count: Option<i32>,
}

// ===========================================================================
// URL helpers
// ===========================================================================

/// Build a URL: `{base}{path}?key=value&...`.
fn build_url(base: &str, path: &str, params: &[(&str, String)]) -> String {
    if params.is_empty() {
        return format!("{base}{path}");
    }
    let qs: Vec<String> = params
        .iter()
        .map(|(k, v)| {
            if v.is_empty() {
                urlencode(k)
            } else {
                format!("{}={}", urlencode(k), urlencode(v))
            }
        })
        .collect();
    format!("{base}{path}?{}", qs.join("&"))
}

/// Minimal percent-encoding for URL query values.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{:02X}", b));
            }
        }
    }
    out
}
