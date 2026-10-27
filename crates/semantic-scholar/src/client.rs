use std::sync::Arc;
use std::time::{Duration, Instant};

use reqwest::Client;
use tokio::sync::Mutex;

use crate::error::{Result, S2Error};
use crate::types::*;

fn env_endpoint(env: &str, fallback: &str) -> String {
    std::env::var(env).unwrap_or_else(|_| fallback.to_string())
}

/// Base URL for the Semantic Scholar Academic Graph API.
const GRAPH_BASE: &str = "https://api.semanticscholar.org/graph/v1";

/// Base URL for the Semantic Scholar Recommendations API.
const RECO_BASE: &str = "https://api.semanticscholar.org/recommendations/v1";

#[derive(Debug, Default)]
struct Pace {
    last: Option<Instant>,
    cooldown: Option<Instant>,
    blocked: bool,
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
#[derive(Clone)]
pub struct S2Client {
    client: Client,
    api_key: Option<String>,
    pace: Arc<Mutex<Pace>>,
    graph_base: String,
    reco_base: String,
    max_retries: u32,
}

impl std::fmt::Debug for S2Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S2Client")
            .field("api_key_configured", &self.has_api_key())
            .field("max_retries", &self.max_retries)
            .finish_non_exhaustive()
    }
}

impl Default for S2Client {
    fn default() -> Self {
        Self::new()
    }
}

impl S2Client {
    /// Create a new client with default settings (no API key).
    pub fn new() -> Self {
        Self::with_client(
            Client::builder()
                .user_agent("semantic-scholar-rs-sdk/0.1 (+https://www.semanticscholar.org)")
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(90))
                .build()
                .expect("reqwest client builder"),
        )
    }

    /// Provide an API key for higher rate limits.
    ///
    /// The key is sent as the `x-api-key` header on every request.
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into()).filter(|key| !key.trim().is_empty());
        self
    }

    /// Provide a custom `reqwest::Client` (e.g. for timeouts, proxy).
    pub fn with_client(client: Client) -> Self {
        Self {
            client,
            api_key: None,
            pace: Arc::new(Mutex::new(Pace::default())),
            graph_base: env_endpoint("ENDPOINT_S2_GRAPH_URL", GRAPH_BASE),
            reco_base: env_endpoint("ENDPOINT_S2_RECO_URL", RECO_BASE),
            max_retries: 5,
        }
    }

    /// Override this instance's Graph endpoint without process environment mutation.
    pub fn with_graph_base_url(mut self, url: impl Into<String>) -> Self {
        self.graph_base = url.into().trim_end_matches('/').to_owned();
        self
    }

    /// Override this instance's Recommendations endpoint.
    pub fn with_reco_base_url(mut self, url: impl Into<String>) -> Self {
        self.reco_base = url.into().trim_end_matches('/').to_owned();
        self
    }

    /// Set automatic 429 retries. Use zero when an outer session owns its budget.
    /// Clones retain the shared pacing and cooldown even with different policies.
    pub fn with_max_retries(mut self, retries: u32) -> Self {
        self.max_retries = retries.min(5);
        self
    }

    pub fn has_api_key(&self) -> bool {
        self.api_key.is_some()
    }

    // -----------------------------------------------------------------------
    // Core request helpers
    // -----------------------------------------------------------------------

    /// Conservative request starts shared across clones. Waiting/cancelled calls
    /// do not reserve future timestamps or accumulate idle burst credits.
    async fn enforce_rate_limit(&self) -> Result<()> {
        loop {
            let mut pace = self.pace.lock().await;
            if pace.blocked {
                return Err(S2Error::RateLimited {
                    retry_after_seconds: Some(u64::MAX),
                });
            }
            let rate = pace
                .last
                .map(|at| Duration::from_millis(1050).saturating_sub(at.elapsed()))
                .unwrap_or_default();
            let cooldown = pace
                .cooldown
                .map(|at| at.saturating_duration_since(Instant::now()))
                .unwrap_or_default();
            let delay = rate.max(cooldown);
            if delay.is_zero() {
                pace.last = Some(Instant::now());
                return Ok(());
            }
            drop(pace);
            tokio::time::sleep(delay).await;
        }
    }

    fn retry_after(resp: &reqwest::Response) -> Option<u64> {
        let value = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)?
            .to_str()
            .ok()?;
        value.trim().parse().ok().or_else(|| {
            httpdate::parse_http_date(value).ok().map(|at| {
                let delay = at
                    .duration_since(std::time::SystemTime::now())
                    .unwrap_or_default();
                delay
                    .as_secs()
                    .saturating_add(u64::from(delay.subsec_nanos() > 0))
            })
        })
    }

    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        base: &str,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<T> {
        self.request_json(reqwest::Method::GET, base, path, params, None)
            .await
    }

    async fn post_json<T: serde::de::DeserializeOwned>(
        &self,
        base: &str,
        path: &str,
        params: &[(&str, String)],
        body: &impl serde::Serialize,
    ) -> Result<T> {
        self.request_json(
            reqwest::Method::POST,
            base,
            path,
            params,
            Some(&serde_json::to_value(body)?),
        )
        .await
    }

    async fn request_json<T: serde::de::DeserializeOwned>(
        &self,
        method: reqwest::Method,
        base: &str,
        path: &str,
        params: &[(&str, String)],
        body: Option<&serde_json::Value>,
    ) -> Result<T> {
        let url = build_url(base, path, params);
        if url.len() > 8192 {
            return Err(S2Error::Param("request URL exceeds 8192 bytes".into()));
        }
        for attempt in 0..=self.max_retries {
            self.enforce_rate_limit().await?;
            let mut req = self.client.request(method.clone(), &url);
            if let Some(key) = &self.api_key {
                req = req.header("x-api-key", key);
            }
            if let Some(body) = body {
                req = req.json(body);
            }
            let mut resp = req.send().await?;
            let status = resp.status().as_u16();
            if status == 429 {
                let retry_after_seconds = Self::retry_after(&resp);
                let seconds = retry_after_seconds.unwrap_or((2u64 << attempt).min(30));
                let mut pace = self.pace.lock().await;
                if let Some(until) = Instant::now().checked_add(Duration::from_secs(seconds)) {
                    pace.cooldown = Some(pace.cooldown.map_or(until, |old| old.max(until)));
                } else {
                    pace.blocked = true;
                }
                drop(pace);
                // Drop the response before retrying; never wait for an untrusted 429 body.
                drop(resp);
                if attempt < self.max_retries {
                    continue;
                }
                return Err(S2Error::RateLimited {
                    retry_after_seconds,
                });
            }
            let success = (200..300).contains(&status);
            let limit = if success { 16 * 1024 * 1024 } else { 64 * 1024 };
            let oversized = || {
                if success {
                    S2Error::ResponseTooLarge
                } else {
                    S2Error::Status {
                        status,
                        body: "response body omitted (size limit)".into(),
                    }
                }
            };
            if resp
                .content_length()
                .is_some_and(|length| length > limit as u64)
            {
                return Err(oversized());
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = resp.chunk().await? {
                if bytes.len().saturating_add(chunk.len()) > limit {
                    return Err(oversized());
                }
                bytes.extend_from_slice(&chunk);
            }
            if !success {
                return Err(S2Error::Status {
                    status,
                    body: String::from_utf8_lossy(&bytes).into_owned(),
                });
            }
            let value: serde_json::Value = serde_json::from_slice(&bytes)?;
            let invalid =
                || S2Error::InvalidResponse("missing metadata records or response envelope".into());
            if value.get("error").is_some_and(|error| !error.is_null()) {
                return Err(invalid());
            }
            if path == "/paper/search" || path == "/paper/search/bulk" {
                if value["total"].as_u64().is_none() || !value["data"].is_array() {
                    return Err(invalid());
                }
                if path == "/paper/search"
                    && (value["offset"].as_u64().is_none()
                        || value
                            .get("next")
                            .is_some_and(|v| !v.is_null() && v.as_u64().is_none()))
                {
                    return Err(invalid());
                }
                if path.ends_with("/bulk")
                    && value
                        .get("token")
                        .is_some_and(|v| !v.is_null() && v.as_str().is_none_or(str::is_empty))
                {
                    return Err(invalid());
                }
            } else if path.starts_with("/paper/")
                && path[7..].split('/').count() == 1
                && !matches!(path, "/paper/autocomplete" | "/paper/batch")
                && value["paperId"].as_str().is_none_or(str::is_empty)
            {
                return Err(invalid());
            }
            return serde_json::from_value(value).map_err(Into::into);
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
        self.get_json(&self.graph_base, "/paper/search", &params)
            .await
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
        params.extend(filter.to_params());
        self.get_json(&self.graph_base, "/paper/search", &params)
            .await
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
        self.search_paper_bulk_filtered(query, token, fields, sort, &PaperSearchFilter::default())
            .await
    }

    /// Bulk retrieval with the same documented metadata filters as relevance
    /// search. The provider returns up to 1000 records; it has no page-size
    /// parameter. Applications must buffer a batch when exposing smaller pages.
    pub async fn search_paper_bulk_filtered(
        &self,
        query: &str,
        token: Option<&str>,
        fields: Option<&str>,
        sort: Option<&str>,
        filter: &PaperSearchFilter,
    ) -> Result<PaperBulkResponse> {
        let mut params: Vec<(&str, String)> = filter.to_params();
        if !query.is_empty() {
            params.push(("query", query.to_owned()));
        }
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
        self.get_json(&self.graph_base, "/paper/search/bulk", &params)
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
        self.get_json(&self.graph_base, &path, &params).await
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
        self.post_json(&self.graph_base, "/paper/batch", &params, &body)
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
        self.get_json(&self.graph_base, &path, &params).await
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
        self.get_json(&self.graph_base, &path, &params).await
    }

    // ===================================================================
    // Autocomplete
    // ===================================================================

    /// Suggest paper query completions for interactive search.
    pub async fn autocomplete(&self, query: &str) -> Result<serde_json::Value> {
        let params: Vec<(&str, String)> = vec![("query", query.to_owned())];
        self.get_json(&self.graph_base, "/paper/autocomplete", &params)
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
        self.get_json(&self.graph_base, "/author/search", &params)
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
        self.get_json(&self.graph_base, &path, &params).await
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
        self.get_json(&self.graph_base, &path, &params).await
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
        self.get_json(&self.reco_base, &path, &params).await
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
    /// Inclusive date or year interval, e.g. `"2020-01:2023-12"`.
    pub publication_date_or_year: Option<String>,
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

impl PaperSearchFilter {
    fn to_params(&self) -> Vec<(&'static str, String)> {
        let mut params = Vec::new();
        for (key, value) in [
            ("year", &self.year),
            ("publicationDateOrYear", &self.publication_date_or_year),
            ("venue", &self.venue),
            ("fieldsOfStudy", &self.fields_of_study),
            ("publicationTypes", &self.publication_types),
        ] {
            if let Some(value) = value {
                params.push((key, value.clone()));
            }
        }
        if self.open_access_pdf {
            params.push(("openAccessPdf", String::new()));
        }
        if let Some(count) = self.min_citation_count {
            params.push(("minCitationCount", count.to_string()));
        }
        params
    }
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
