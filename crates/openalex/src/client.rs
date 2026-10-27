//! Async HTTP client for the [OpenAlex REST API](https://api.openalex.org).
//!
//! The client is organised around entity endpoints (`/works`, `/authors`, …)
//! and supports filter, search, sort, cursor paging, `select`, `group_by`,
//! and `sample` query parameters. An optional `api_key` query parameter is
//! appended to every request for higher rate limits.

use reqwest::Client;

use crate::error::{OpenAlexError, Result};
use crate::types::*;

/// Respect both delta-seconds and HTTP-date Retry-After forms.
fn retry_after(response: &reqwest::Response) -> Option<u64> {
    let value = response.headers().get("retry-after")?.to_str().ok()?;
    if let Ok(seconds) = value.trim().parse::<u64>() {
        return Some(seconds);
    }
    let until = httpdate::parse_http_date(value).ok()?;
    let duration = until
        .duration_since(std::time::SystemTime::now())
        .unwrap_or_default();
    Some(duration.as_secs() + u64::from(duration.subsec_nanos() > 0))
}

fn env_endpoint(env: &str, fallback: &str) -> String {
    std::env::var(env).unwrap_or_else(|_| fallback.to_string())
}

/// Base URL for all OpenAlex API requests.
pub const BASE_URL: &str = "https://api.openalex.org";

/// Resolve the OpenAlex endpoint override from the environment.
pub fn base_url() -> String {
    env_endpoint("ENDPOINT_OPENALEX_URL", BASE_URL)
}

/// All OpenAlex entity endpoints share the same set of query parameters.
/// This builder collects them into one struct so each list method can
/// forward it unchanged.
#[derive(Debug, Clone, Default)]
pub struct ListParams {
    /// Filter expression, e.g. `"publication_year:2024,is_oa:true"`.
    pub filter: Option<String>,
    /// Full-text search query.
    pub search: Option<String>,
    /// Scoped/exact works searches; multiple scopes combine with AND.
    pub scoped_search: std::collections::BTreeMap<String, String>,
    /// Sort expression, e.g. `"cited_by_count:desc"`.
    pub sort: Option<String>,
    /// Results per page (1–100, default 25).
    pub per_page: Option<u32>,
    /// 1-based page number (basic paging, max 10 000 total results).
    pub page: Option<u32>,
    /// Cursor for deep paging. Use `"*"` to start; pass `meta.next_cursor`.
    pub cursor: Option<String>,
    /// Comma-separated list of fields to include in the response.
    pub select: Option<String>,
    /// Random sample size (max 10 000).
    pub sample: Option<u32>,
    /// Sample seed for reproducibility.
    pub seed: Option<u64>,
    /// Aggregate results by a field, e.g. `"publication_year"`.
    pub group_by: Option<String>,
}

impl ListParams {
    /// Create empty params.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the filter expression.
    pub fn with_filter(mut self, filter: impl Into<String>) -> Self {
        self.filter = Some(filter.into());
        self
    }

    /// Set the search query.
    pub fn with_search(mut self, search: impl Into<String>) -> Self {
        self.search = Some(search.into());
        self
    }

    /// Set a supported scoped or exact works search parameter.
    pub fn with_scoped_search(mut self, parameter: &str, query: impl Into<String>) -> Result<Self> {
        if ![
            "search",
            "search.exact",
            "search.title",
            "search.title.exact",
            "search.title_and_abstract",
            "search.title_and_abstract.exact",
        ]
        .contains(&parameter)
        {
            return Err(OpenAlexError::Param(
                "unsupported scoped search parameter".into(),
            ));
        }
        if parameter == "search" {
            self.search = Some(query.into());
        } else {
            self.scoped_search.insert(parameter.into(), query.into());
        }
        Ok(self)
    }

    /// Set the sort expression.
    pub fn with_sort(mut self, sort: impl Into<String>) -> Self {
        self.sort = Some(sort.into());
        self
    }

    /// Set `per_page`.
    pub fn with_per_page(mut self, per_page: u32) -> Self {
        self.per_page = Some(per_page);
        self
    }

    /// Set the page number (basic paging).
    pub fn with_page(mut self, page: u32) -> Self {
        self.page = Some(page);
        self
    }

    /// Set the cursor for deep paging.
    pub fn with_cursor(mut self, cursor: impl Into<String>) -> Self {
        self.cursor = Some(cursor.into());
        self
    }

    /// Limit returned fields via `select`.
    pub fn with_select(mut self, select: impl Into<String>) -> Self {
        self.select = Some(select.into());
        self
    }

    /// Set the `group_by` aggregation field.
    pub fn with_group_by(mut self, field: impl Into<String>) -> Self {
        self.group_by = Some(field.into());
        self
    }

    /// Set sample size and optional seed.
    pub fn with_sample(mut self, n: u32, seed: Option<u64>) -> Self {
        self.sample = Some(n);
        self.seed = seed;
        self
    }

    /// Render into `(key, value)` pairs suitable for URL encoding.
    fn to_query_pairs(&self) -> Vec<(&str, String)> {
        let mut pairs = Vec::new();
        if let Some(ref v) = self.filter {
            pairs.push(("filter", v.clone()));
        }
        if let Some(ref v) = self.search {
            pairs.push(("search", v.clone()));
        }
        pairs.extend(
            self.scoped_search
                .iter()
                .map(|(key, value)| (key.as_str(), value.clone())),
        );
        if let Some(ref v) = self.sort {
            pairs.push(("sort", v.clone()));
        }
        if let Some(v) = self.per_page {
            pairs.push(("per_page", v.to_string()));
        }
        if let Some(v) = self.page {
            pairs.push(("page", v.to_string()));
        }
        if let Some(ref v) = self.cursor {
            pairs.push(("cursor", v.clone()));
        }
        if let Some(ref v) = self.select {
            pairs.push(("select", v.clone()));
        }
        if let Some(v) = self.sample {
            pairs.push(("sample", v.to_string()));
        }
        if let Some(v) = self.seed {
            pairs.push(("seed", v.to_string()));
        }
        if let Some(ref v) = self.group_by {
            pairs.push(("group_by", v.clone()));
        }
        pairs
    }
}

// ===========================================================================
// Client
// ===========================================================================

/// An async client for the [OpenAlex REST API](https://api.openalex.org).
///
/// OpenAlex data is free; an optional `api_key` (free at
/// <https://openalex.org/settings/api>) increases the daily rate-limit budget.
///
/// # Endpoints covered
///
/// | Method | API path |
/// |--------|----------|
/// | [`list_works`](Self::list_works) | `GET /works` |
/// | [`get_work`](Self::get_work) | `GET /works/{id}` |
/// | [`search_works`](Self::search_works) | `GET /works?search=` |
/// | [`list_authors`](Self::list_authors) | `GET /authors` |
/// | [`get_author`](Self::get_author) | `GET /authors/{id}` |
/// | [`list_sources`](Self::list_sources) | `GET /sources` |
/// | [`list_institutions`](Self::list_institutions) | `GET /institutions` |
/// | [`list_topics`](Self::list_topics) | `GET /topics` |
/// | [`list_funders`](Self::list_funders) | `GET /funders` |
/// | [`autocomplete`](Self::autocomplete) | `GET /autocomplete/{entity}` |
///
/// # Example
///
/// ```no_run
/// use openalex::{OpenAlexClient, ListParams};
///
/// # async fn run() -> openalex::error::Result<()> {
/// let client = OpenAlexClient::new(None);
/// let resp = client
///     .list_works(
///         &ListParams::new()
///             .with_filter("publication_year:2024,cited_by_count:>100")
///             .with_sort("cited_by_count:desc")
///             .with_per_page(10),
///     )
///     .await?;
/// println!("{} works", resp.meta.count);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct OpenAlexClient {
    client: Client,
    base_url: String,
    api_key: Option<String>,
    last_request: std::sync::Arc<tokio::sync::Mutex<Option<std::time::Instant>>>,
    quota: std::sync::Arc<std::sync::RwLock<Option<bib_types::discovery::SourceQuota>>>,
}

impl OpenAlexClient {
    /// Last observed server budget, never an inferred remaining balance.
    pub fn quota(&self) -> Option<bib_types::discovery::SourceQuota> {
        self.quota
            .read()
            .expect("quota state lock poisoned")
            .clone()
    }
    /// Cursor continuation beyond the first page requires an API key.
    pub fn has_api_key(&self) -> bool {
        self.api_key
            .as_ref()
            .is_some_and(|key| !key.trim().is_empty())
    }
    /// Create a new client.
    ///
    /// Pass `Some("your-key")` to use a premium API key, or `None` for the
    /// free tier.
    pub fn new(api_key: Option<&str>) -> Self {
        let client = Client::builder()
            .user_agent("openalex-rs-sdk/0.1 (+https://openalex.org)")
            .build()
            .expect("reqwest client builder");
        Self {
            client,
            base_url: base_url(),
            api_key: api_key
                .filter(|key| !key.trim().is_empty())
                .map(|s| s.to_string()),
            last_request: Default::default(),
            quota: Default::default(),
        }
    }

    /// Provide a custom `reqwest::Client` (e.g. for timeouts, proxy).
    pub fn with_client(client: Client, api_key: Option<&str>) -> Self {
        Self {
            client,
            base_url: base_url(),
            api_key: api_key
                .filter(|key| !key.trim().is_empty())
                .map(|s| s.to_string()),
            last_request: Default::default(),
            quota: Default::default(),
        }
    }

    /// Override the base URL (for testing / staging).
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    // -----------------------------------------------------------------------
    // Core request helpers
    // -----------------------------------------------------------------------

    /// Execute a GET request that returns JSON and deserialise it.
    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<T> {
        // Clones reserve starts on one shared limiter. Complex Boolean
        // requests have a lower upstream rate than ordinary queries.
        let operators = params
            .iter()
            .map(|(_, value)| {
                value
                    .split_whitespace()
                    .filter(|word| matches!(*word, "AND" | "OR" | "NOT"))
                    .count()
            })
            .sum::<usize>();
        let millis = match (self.has_api_key(), operators > 5) {
            (false, true) => 1000,
            (true, true) => 200,
            (false, false) => 100,
            (true, false) => 34,
        };
        let start = {
            let mut last = self.last_request.lock().await;
            let now = std::time::Instant::now();
            let start = last
                .and_then(|last| last.checked_add(std::time::Duration::from_millis(millis)))
                .unwrap_or(now)
                .max(now);
            *last = Some(start);
            start
        };
        tokio::time::sleep(start.saturating_duration_since(std::time::Instant::now())).await;
        let url = self.build_url(path, params);
        if url.len() > 4094 {
            return Err(OpenAlexError::Param(
                "OpenAlex request URL exceeds 4094 bytes; narrow or split the query".into(),
            ));
        }
        let resp = self.client.get(&url).send().await?;
        let status = resp.status().as_u16();
        let number = |key: &str| {
            resp.headers()
                .get(key)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v >= 0.0)
        };
        *self.quota.write().expect("quota state lock poisoned") =
            Some(bib_types::discovery::SourceQuota {
                limit: number("x-ratelimit-limit"),
                remaining: number("x-ratelimit-remaining"),
                reset_unix_seconds: resp
                    .headers()
                    .get("x-ratelimit-reset")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse().ok()),
                observed_at_unix_seconds: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
                last_http_status: status,
            });
        let retry_after_seconds = retry_after(&resp);
        let body = resp.text().await?;
        if status == 429 {
            return Err(OpenAlexError::RateLimited {
                retry_after_seconds,
                body,
            });
        }
        if status == 404 {
            return Err(OpenAlexError::NotFound(body));
        }
        if !(200..300).contains(&status) {
            return Err(OpenAlexError::Status { status, body });
        }
        serde_json::from_str(&body).map_err(Into::into)
    }

    /// Build a URL: `{base}{path}?key=value&…&api_key=…`.
    fn build_url(&self, path: &str, params: &[(&str, String)]) -> String {
        let mut all: Vec<(&str, String)> = params.to_vec();
        if let Some(ref key) = self.api_key {
            all.push(("api_key", key.clone()));
        }
        if all.is_empty() {
            return format!("{}{}", self.base_url, path);
        }
        let qs: Vec<String> = all
            .iter()
            .map(|(k, v)| format!("{}={}", urlencode(k), urlencode(v)))
            .collect();
        format!("{}{}?{}", self.base_url, path, qs.join("&"))
    }

    // ===================================================================
    // Works
    // ===================================================================

    /// List or filter works.
    ///
    /// # Example
    /// ```no_run
    /// # use openalex::{OpenAlexClient, ListParams};
    /// # async fn run() -> openalex::error::Result<()> {
    /// let c = OpenAlexClient::new(None);
    /// let r = c.list_works(&ListParams::new().with_filter("is_oa:true").with_per_page(5)).await?;
    /// assert!(r.results.len() <= 5);
    /// # Ok(()) }
    /// ```
    pub async fn list_works(&self, params: &ListParams) -> Result<ListResponse<Work>> {
        self.get_json("/works", &params.to_query_pairs()).await
    }

    /// Fetch all works matching `params`, auto-paginating via cursor.
    ///
    /// **Warning:** this can issue many requests for broad queries. Always
    /// use `filter` to narrow the result set.
    pub async fn list_works_all(&self, params: &ListParams) -> Result<Vec<Work>> {
        let mut all = Vec::new();
        let mut p = params.clone();
        p.cursor = Some("*".to_string());
        p.per_page = Some(params.per_page.unwrap_or(100).min(100));
        // Avoid `page` conflicting with `cursor`.
        p.page = None;
        loop {
            let resp = self.list_works(&p).await?;
            let count = resp.results.len();
            all.extend(resp.results);
            match resp.meta.next_cursor {
                Some(cursor) if !cursor.is_empty() && count > 0 => {
                    p.cursor = Some(cursor);
                }
                _ => break,
            }
        }
        Ok(all)
    }

    /// Retrieve a single work by OpenAlex ID or external ID.
    ///
    /// Accepted `id` formats:
    /// - `"W2741809807"` (OpenAlex ID)
    /// - `"https://doi.org/10.7717/peerj.4375"` (full DOI URL)
    /// - `"doi:10.7717/peerj.4375"` (DOI shortcut)
    /// - `"pmid:29456894"` (PubMed ID shortcut)
    pub async fn get_work(&self, id: &str) -> Result<Work> {
        let path = format!("/works/{}", urlencode(id));
        self.get_json(&path, &[]).await
    }

    /// Convenience: full-text search across works.
    pub async fn search_works(
        &self,
        query: &str,
        per_page: Option<u32>,
    ) -> Result<ListResponse<Work>> {
        let params = ListParams::new()
            .with_search(query)
            .with_per_page(per_page.unwrap_or(25).min(100));
        self.list_works(&params).await
    }

    // ===================================================================
    // Authors
    // ===================================================================

    pub async fn list_authors(&self, params: &ListParams) -> Result<ListResponse<Author>> {
        self.get_json("/authors", &params.to_query_pairs()).await
    }

    /// Retrieve a single author by OpenAlex ID or external ID.
    ///
    /// Accepted formats: `"A5023888391"`, full ORCID URL,
    /// or `"orcid:0000-0001-6187-6610"`.
    pub async fn get_author(&self, id: &str) -> Result<Author> {
        let path = format!("/authors/{}", urlencode(id));
        self.get_json(&path, &[]).await
    }

    // ===================================================================
    // Sources
    // ===================================================================

    pub async fn list_sources(&self, params: &ListParams) -> Result<ListResponse<Source>> {
        self.get_json("/sources", &params.to_query_pairs()).await
    }

    /// Retrieve a single source by OpenAlex ID or ISSN.
    pub async fn get_source(&self, id: &str) -> Result<Source> {
        let path = format!("/sources/{}", urlencode(id));
        self.get_json(&path, &[]).await
    }

    // ===================================================================
    // Institutions
    // ===================================================================

    pub async fn list_institutions(
        &self,
        params: &ListParams,
    ) -> Result<ListResponse<Institution>> {
        self.get_json("/institutions", &params.to_query_pairs())
            .await
    }

    /// Retrieve a single institution by OpenAlex ID or ROR.
    pub async fn get_institution(&self, id: &str) -> Result<Institution> {
        let path = format!("/institutions/{}", urlencode(id));
        self.get_json(&path, &[]).await
    }

    // ===================================================================
    // Topics
    // ===================================================================

    pub async fn list_topics(&self, params: &ListParams) -> Result<ListResponse<Topic>> {
        self.get_json("/topics", &params.to_query_pairs()).await
    }

    pub async fn get_topic(&self, id: &str) -> Result<Topic> {
        let path = format!("/topics/{}", urlencode(id));
        self.get_json(&path, &[]).await
    }

    // ===================================================================
    // Funders
    // ===================================================================

    pub async fn list_funders(&self, params: &ListParams) -> Result<ListResponse<Funder>> {
        self.get_json("/funders", &params.to_query_pairs()).await
    }

    pub async fn get_funder(&self, id: &str) -> Result<Funder> {
        let path = format!("/funders/{}", urlencode(id));
        self.get_json(&path, &[]).await
    }

    // ===================================================================
    // Autocomplete
    // ===================================================================

    /// Fast typeahead search.
    ///
    /// `entity` is one of `"works"`, `"authors"`, `"sources"`,
    /// `"institutions"`, `"topics"`.
    pub async fn autocomplete(&self, entity: &str, query: &str) -> Result<AutocompleteResponse> {
        let path = format!("/autocomplete/{}", urlencode(entity));
        let params: Vec<(&str, String)> = vec![("q", query.to_string())];
        self.get_json(&path, &params).await
    }

    // ===================================================================
    // N-grams (works only)
    // ===================================================================

    /// Retrieve the n-grams for a work (the General Index data).
    ///
    /// Returns the raw JSON value — the response shape is a simple list of
    /// `{ ngram, ngram_count, ngram_tokens, term_frequency }` objects.
    pub async fn work_ngrams(&self, work_id: &str) -> Result<serde_json::Value> {
        let clean = work_id
            .strip_prefix("https://openalex.org/")
            .unwrap_or(work_id);
        let path = format!("/works/{}/ngrams", urlencode(clean));
        self.get_json(&path, &[]).await
    }
}

// ===========================================================================
// URL helpers
// ===========================================================================

/// Minimal percent-encoding for URL query values and path segments.
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

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_params_query_pairs() {
        let p = ListParams::new()
            .with_filter("publication_year:2024")
            .with_per_page(10);
        let pairs = p.to_query_pairs();
        assert!(
            pairs
                .iter()
                .any(|(k, v)| *k == "filter" && v == "publication_year:2024")
        );
        assert!(pairs.iter().any(|(k, v)| *k == "per_page" && v == "10"));
    }

    #[test]
    fn url_encode_special() {
        assert_eq!(
            urlencode("doi:10.7717/peerj.4375"),
            "doi%3A10.7717%2Fpeerj.4375"
        );
        assert_eq!(urlencode("ABC123-_.~"), "ABC123-_.~");
    }

    #[test]
    fn build_url_with_api_key() {
        let client = OpenAlexClient::new(Some("test-key"));
        let url = client.build_url("/works", &[("per_page", "10".to_string())]);
        assert!(url.contains("per_page=10"));
        assert!(url.contains("api_key=test-key"));
    }
}
