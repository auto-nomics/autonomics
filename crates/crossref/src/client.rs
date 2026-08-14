//! Async HTTP client for the Crossref REST API.

use reqwest::Client;

use crate::error::{CrossrefError, Result};
use crate::types::*;

fn env_endpoint(env: &str, fallback: &str) -> String {
    std::env::var(env).unwrap_or_else(|_| fallback.to_string())
}

/// Base URL for all Crossref REST API requests.
const BASE_URL: &str = "https://api.crossref.org";

/// Resolve the Crossref endpoint override from the environment.
fn base_url() -> String {
    env_endpoint("ENDPOINT_CROSSREF_URL", BASE_URL)
}

// ===========================================================================
// Builder
// ===========================================================================

/// Builder for [`CrossrefClient`].
///
/// Created via [`CrossrefClient::builder()`].
#[derive(Debug, Clone)]
pub struct CrossrefClientBuilder {
    mailto: Option<String>,
    bearer_token: Option<String>,
    user_agent: Option<String>,
    client: Option<Client>,
}

impl Default for CrossrefClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl CrossrefClientBuilder {
    /// Create a new builder with defaults.
    pub fn new() -> Self {
        Self {
            mailto: None,
            bearer_token: None,
            user_agent: None,
            client: None,
        }
    }

    /// Set the contact email for the Crossref "polite pool".
    ///
    /// When set, `mailto=<email>` is appended to every request, directing it
    /// to the high-availability API pool. See the [Etiquette guide](
    /// https://github.com/CrossRef/rest-api-doc#etiquette).
    pub fn mailto(mut self, email: impl Into<String>) -> Self {
        self.mailto = Some(email.into());
        self
    }

    /// Set a bearer token for Crossref Plus service.
    ///
    /// When set, the `Crossref-Plus-API-Token: Bearer <token>` header is sent
    /// on every request.
    pub fn bearer_token(mut self, token: impl Into<String>) -> Self {
        self.bearer_token = Some(token.into());
        self
    }

    /// Override the `User-Agent` header.
    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = Some(ua.into());
        self
    }

    /// Provide a pre-built `reqwest::Client` (e.g. for proxy / timeout).
    pub fn reqwest_client(mut self, client: Client) -> Self {
        self.client = Some(client);
        self
    }

    /// Build the [`CrossrefClient`].
    pub fn build(self) -> CrossrefClient {
        let ua = self.user_agent.unwrap_or_else(|| {
            format!("crossref-rs-sdk/0.1 (+https://github.com/wjixiang/autonomics)")
        });
        let builder = if let Some(c) = self.client {
            return CrossrefClient {
                client: c,
                mailto: self.mailto,
                bearer_token: self.bearer_token,
            };
        } else {
            Client::builder().user_agent(ua)
        };
        let client = builder.build().expect("reqwest client builder");
        CrossrefClient {
            client,
            mailto: self.mailto,
            bearer_token: self.bearer_token,
        }
    }
}

// ===========================================================================
// Client
// ===========================================================================

/// An async client for the [Crossref REST API](
/// https://www.crossref.org/documentation/retrieve-metadata/rest-api/).
///
/// The API is free and open — no API key required. Setting a contact email
/// via [`CrossrefClientBuilder::mailto`] is recommended for the polite pool.
///
/// # Endpoints covered
///
/// | Method                              | API path                    |
/// |-------------------------------------|-----------------------------|
/// | [`works`](Self::works)              | `GET /works`                |
/// | [`works_by_doi`](Self::works_by_doi)| `GET /works/{doi}`          |
/// | [`works_agency`](Self::works_agency)| `GET /works/{doi}/agency`   |
/// | [`journals`](Self::journals)        | `GET /journals`             |
/// | [`journal`](Self::journal)          | `GET /journals/{issn}`      |
/// | [`journal_works`](Self::journal_works) | `GET /journals/{issn}/works` |
/// | [`members`](Self::members)          | `GET /members`              |
/// | [`member`](Self::member)            | `GET /members/{id}`         |
/// | [`member_works`](Self::member_works)| `GET /members/{id}/works`   |
/// | [`funders`](Self::funders)          | `GET /funders`              |
/// | [`funder`](Self::funder)            | `GET /funders/{id}`         |
/// | [`funder_works`](Self::funder_works)| `GET /funders/{id}/works`   |
/// | [`prefix_works`](Self::prefix_works)| `GET /prefixes/{prefix}/works` |
/// | [`types`](Self::types)              | `GET /types`                |
/// | [`type_works`](Self::type_works)    | `GET /types/{id}/works`     |
/// | [`licenses`](Self::licenses)        | `GET /licenses`             |
///
/// # Pagination
///
/// List endpoints support `rows` (page size, max 1000) and `offset` (max 10k).
/// For deep paging through `/works`, use cursor-based pagination via
/// [`WorksQuery::cursor`].
///
/// # Example
///
/// ```no_run
/// # use crossref::CrossrefClient;
/// let client = CrossrefClient::builder()
///     .mailto("researcher@example.org")
///     .build();
/// ```
#[derive(Debug, Clone)]
pub struct CrossrefClient {
    client: Client,
    mailto: Option<String>,
    bearer_token: Option<String>,
}

impl Default for CrossrefClient {
    fn default() -> Self {
        Self::builder().build()
    }
}

impl CrossrefClient {
    /// Create a builder for customising the client.
    pub fn builder() -> CrossrefClientBuilder {
        CrossrefClientBuilder::new()
    }

    /// Create a client with default settings (no polite email).
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a client with the given contact email (polite pool shortcut).
    pub fn with_mailto(email: impl Into<String>) -> Self {
        Self::builder().mailto(email).build()
    }

    // -----------------------------------------------------------------------
    // Core request helpers
    // -----------------------------------------------------------------------

    /// Execute a GET request returning JSON and deserialise it.
    async fn get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<T> {
        let url = build_url(&base_url(), path, params, self.mailto.as_deref());
        let req = {
            let r = self.client.get(&url);
            if let Some(ref token) = self.bearer_token {
                r.header("Crossref-Plus-API-Token", format!("Bearer {token}"))
            } else {
                r
            }
        };
        let resp = req.send().await?;
        let status = resp.status().as_u16();
        let body = resp.text().await?;
        if !(200..300).contains(&status) {
            return Err(CrossrefError::Status { status, body });
        }
        serde_json::from_str(&body).map_err(Into::into)
    }

    // ===================================================================
    // Works
    // ===================================================================

    /// Search the `/works` endpoint for works matching `query`.
    ///
    /// Use [`WorksQuery`] for advanced filters, field queries, sorting, etc.
    pub async fn works(&self, query: &WorksQuery) -> Result<WorksListResponse> {
        self.get("/works", &query.to_params()).await
    }

    /// Retrieve a single work by its DOI.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use crossref::CrossrefClient;
    /// # async fn run() -> crossref::error::Result<()> {
    /// let client = CrossrefClient::new();
    /// let resp = client.works_by_doi("10.1037/0003-066X.59.1.29").await?;
    /// assert!(resp.message.is_some());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn works_by_doi(&self, doi: &str) -> Result<WorkResponse> {
        let path = format!("/works/{}", urlencode(doi));
        self.get(&path, &[]).await
    }

    /// Look up the registration agency for a DOI (Crossref, DataCite, …).
    pub async fn works_agency(&self, doi: &str) -> Result<AgencyResponse> {
        let path = format!("/works/{}/agency", urlencode(doi));
        self.get(&path, &[]).await
    }

    /// Retrieve a random sample of works.
    ///
    /// `sample` must be between 1 and 100.
    pub async fn works_sample(&self, sample: u32) -> Result<WorksListResponse> {
        let n = sample.clamp(1, 100);
        let params = vec![("sample", n.to_string())];
        self.get("/works", &params).await
    }

    // ===================================================================
    // Journals
    // ===================================================================

    /// List journals registered with Crossref.
    pub async fn journals(&self, list: &ListQuery) -> Result<JournalsListResponse> {
        self.get("/journals", &list.to_params()).await
    }

    /// Get details of a specific journal by ISSN.
    pub async fn journal(&self, issn: &str) -> Result<JournalResponse> {
        let path = format!("/journals/{}", urlencode(issn));
        self.get(&path, &[]).await
    }

    /// List works published in a specific journal (by ISSN).
    pub async fn journal_works(&self, issn: &str, query: &WorksQuery) -> Result<WorksListResponse> {
        let path = format!("/journals/{}/works", urlencode(issn));
        self.get(&path, &query.to_params()).await
    }

    // ===================================================================
    // Members
    // ===================================================================

    /// List Crossref members (publishers / organisations).
    pub async fn members(&self, list: &ListQuery) -> Result<MembersListResponse> {
        self.get("/members", &list.to_params()).await
    }

    /// Get details about a specific member by member ID.
    pub async fn member(&self, id: u64) -> Result<MemberResponse> {
        let path = format!("/members/{id}");
        self.get(&path, &[]).await
    }

    /// List works deposited by a specific member.
    pub async fn member_works(&self, id: u64, query: &WorksQuery) -> Result<WorksListResponse> {
        let path = format!("/members/{id}/works");
        self.get(&path, &query.to_params()).await
    }

    // ===================================================================
    // Funders
    // ===================================================================

    /// List funders in the Open Funder Registry.
    pub async fn funders(&self, list: &ListQuery) -> Result<FundersListResponse> {
        self.get("/funders", &list.to_params()).await
    }

    /// Get details of a specific funder by funder DOI or ID.
    pub async fn funder(&self, id: &str) -> Result<FunderResponse> {
        let path = format!("/funders/{}", urlencode(id));
        self.get(&path, &[]).await
    }

    /// List works associated with a specific funder.
    pub async fn funder_works(&self, id: &str, query: &WorksQuery) -> Result<WorksListResponse> {
        let path = format!("/funders/{}/works", urlencode(id));
        self.get(&path, &query.to_params()).await
    }

    // ===================================================================
    // Prefixes
    // ===================================================================

    /// List works associated with a DOI owner prefix (e.g. `10.1016`).
    pub async fn prefix_works(
        &self,
        prefix: &str,
        query: &WorksQuery,
    ) -> Result<WorksListResponse> {
        let path = format!("/prefixes/{}/works", urlencode(prefix));
        self.get(&path, &query.to_params()).await
    }

    // ===================================================================
    // Types & Licenses
    // ===================================================================

    /// List all work types used in Crossref metadata.
    pub async fn types(&self) -> Result<TypesListResponse> {
        self.get("/types", &[]).await
    }

    /// List works of a specific type (e.g. `journal-article`).
    pub async fn type_works(&self, type_id: &str, query: &WorksQuery) -> Result<WorksListResponse> {
        let path = format!("/types/{}/works", urlencode(type_id));
        self.get(&path, &query.to_params()).await
    }

    /// List licenses applied to registered content.
    pub async fn licenses(&self) -> Result<LicensesListResponse> {
        self.get("/licenses", &[]).await
    }
}

// ===========================================================================
// WorksQuery
// ===========================================================================

/// A query against the `/works` endpoint (and sub-resource works endpoints).
///
/// Build via the builder methods or use [`WorksQuery::default`] for a
/// bare request.
#[derive(Debug, Clone, Default)]
pub struct WorksQuery {
    /// Free-form search query.
    pub query: Option<String>,
    /// Field-specific queries.
    pub field_queries: Vec<(String, String)>,
    /// Filters (`name:value` pairs joined with commas).
    pub filters: Vec<(String, String)>,
    /// Sort field.
    pub sort: Option<String>,
    /// Sort order (`asc` / `desc`).
    pub order: Option<String>,
    /// Number of rows per page (max 1000).
    pub rows: Option<u32>,
    /// Result offset (max 10 000 — use cursor beyond that).
    pub offset: Option<u32>,
    /// Deep-paging cursor; use `Some("*")` for the first page.
    pub cursor: Option<String>,
    /// Number of random results (1–100, overrides rows/offset).
    pub sample: Option<u32>,
    /// Select specific fields to return (e.g. `["DOI","title"]`).
    pub select: Option<Vec<String>>,
    /// Facet specification (e.g. `"type-name:*"`).
    pub facet: Option<String>,
}

impl WorksQuery {
    /// Create a new empty query.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the free-form search query.
    pub fn with_query(mut self, q: impl Into<String>) -> Self {
        self.query = Some(q.into());
        self
    }

    /// Add a field query (e.g. `query.author=Smith`).
    pub fn with_field_query(mut self, field: impl Into<String>, value: impl Into<String>) -> Self {
        self.field_queries.push((field.into(), value.into()));
        self
    }

    /// Add a filter (`name:value`).
    pub fn with_filter(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.filters.push((name.into(), value.into()));
        self
    }

    /// Set sort field.
    pub fn with_sort(mut self, sort: impl Into<String>) -> Self {
        self.sort = Some(sort.into());
        self
    }

    /// Set sort order.
    pub fn with_order(mut self, order: impl Into<String>) -> Self {
        self.order = Some(order.into());
        self
    }

    /// Set the number of rows (page size).
    pub fn with_rows(mut self, rows: u32) -> Self {
        self.rows = Some(rows);
        self
    }

    /// Set the offset.
    pub fn with_offset(mut self, offset: u32) -> Self {
        self.offset = Some(offset);
        self
    }

    /// Set the deep-paging cursor.
    pub fn with_cursor(mut self, cursor: impl Into<String>) -> Self {
        self.cursor = Some(cursor.into());
        self
    }

    /// Set the select fields.
    pub fn with_select(mut self, fields: Vec<String>) -> Self {
        self.select = Some(fields);
        self
    }

    /// Render into URL query parameters.
    pub fn to_params(&self) -> Vec<(&str, String)> {
        let mut params: Vec<(&str, String)> = Vec::new();
        if let Some(ref q) = self.query {
            params.push(("query", q.clone()));
        }
        for (field, value) in &self.field_queries {
            // Field queries have dynamic param names (query.author, etc.)
            // so we encode them inline.
            params.push(("__fq", format!("{field}={}", urlencode(value))));
        }
        if !self.filters.is_empty() {
            let joined = self
                .filters
                .iter()
                .map(|(k, v)| format!("{k}:{v}"))
                .collect::<Vec<_>>()
                .join(",");
            params.push(("filter", joined));
        }
        if let Some(ref s) = self.sort {
            params.push(("sort", s.clone()));
        }
        if let Some(ref o) = self.order {
            params.push(("order", o.clone()));
        }
        if let Some(r) = self.rows {
            params.push(("rows", r.to_string()));
        }
        if let Some(o) = self.offset {
            params.push(("offset", o.to_string()));
        }
        if let Some(ref c) = self.cursor {
            params.push(("cursor", c.clone()));
        }
        if let Some(s) = self.sample {
            params.push(("sample", s.to_string()));
        }
        if let Some(ref fields) = self.select {
            params.push(("select", fields.join(",")));
        }
        if let Some(ref f) = self.facet {
            params.push(("facet", f.clone()));
        }
        params
    }
}

// ===========================================================================
// ListQuery — simple list queries for journals/members/funders/types
// ===========================================================================

/// A simple list query for endpoints that support basic query, offset, rows.
#[derive(Debug, Clone, Default)]
pub struct ListQuery {
    pub query: Option<String>,
    pub rows: Option<u32>,
    pub offset: Option<u32>,
}

impl ListQuery {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_query(mut self, q: impl Into<String>) -> Self {
        self.query = Some(q.into());
        self
    }

    pub fn with_rows(mut self, rows: u32) -> Self {
        self.rows = Some(rows);
        self
    }

    pub fn with_offset(mut self, offset: u32) -> Self {
        self.offset = Some(offset);
        self
    }

    pub fn to_params(&self) -> Vec<(&str, String)> {
        let mut params: Vec<(&str, String)> = Vec::new();
        if let Some(ref q) = self.query {
            params.push(("query", q.clone()));
        }
        if let Some(r) = self.rows {
            params.push(("rows", r.to_string()));
        }
        if let Some(o) = self.offset {
            params.push(("offset", o.to_string()));
        }
        params
    }
}

// ===========================================================================
// URL helpers
// ===========================================================================

/// Build a URL: `{base}{path}?key=value&...` with optional `mailto`.
pub(crate) fn build_url(
    base: &str,
    path: &str,
    params: &[(&str, String)],
    mailto: Option<&str>,
) -> String {
    let has_mailto = mailto.is_some() || params.iter().any(|(k, _)| *k == "mailto");
    let mut all_params: Vec<(&str, String)> = params.to_vec();
    if let Some(m) = mailto {
        if !all_params.iter().any(|(k, _)| *k == "mailto") {
            all_params.push(("mailto", m.to_string()));
        }
    }

    if all_params.is_empty() {
        return format!("{base}{path}");
    }

    // Expand inline field-query placeholders created with key "__fq".
    let qs: Vec<String> = all_params
        .iter()
        .map(|(k, v)| {
            if *k == "__fq" {
                // v is already "query.author=Smith"
                v.clone()
            } else {
                format!("{}={}", urlencode(k), urlencode(v))
            }
        })
        .collect();
    let _ = has_mailto; // suppress unused warning
    format!("{base}{path}?{}", qs.join("&"))
}

/// Minimal percent-encoding for URL query values and DOIs.
pub(crate) fn urlencode(s: &str) -> String {
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
