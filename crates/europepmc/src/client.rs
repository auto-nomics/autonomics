use reqwest::Client;

use crate::error::{EuropePmcError, Result};
use crate::types::*;

fn env_endpoint(env: &str, fallback: &str) -> String {
    std::env::var(env).unwrap_or_else(|_| fallback.to_string())
}

/// Base URL for all Europe PMC REST API requests.
const BASE_URL: &str = "https://www.ebi.ac.uk/europepmc/webservices/rest";

fn base_url() -> String {
    env_endpoint("ENDPOINT_EUROPEPMC_URL", BASE_URL)
}

// ===========================================================================
// Client
// ===========================================================================

/// An async client for the [Europe PMC Articles RESTful API](
/// https://europepmc.org/RestfulWebService).
///
/// The API is free and open — no API key required. Just be polite with
/// request volume.
///
/// # Endpoints covered
///
/// | Method                     | API path                          |
/// |----------------------------|-----------------------------------|
/// | [`search`](Self::search)   | `GET /search`                     |
/// | [`article`](Self::article) | `GET /article/{source}/{id}`      |
/// | [`references`](Self::references) | `GET /{source}/{id}/references` |
/// | [`citations`](Self::citations) | `GET /{source}/{id}/citations` |
/// | [`profile`](Self::profile) | `GET /profile`                    |
/// | [`database_links`](Self::database_links) | `GET /{source}/{id}/databaseLinks` |
/// | [`full_text_xml`](Self::full_text_xml) | `GET /{id}/fullTextXML` |
///
/// # Pagination
///
/// The search endpoint uses **cursor-based pagination**: the first request
/// uses `cursorMark="*"`, and subsequent requests pass back the
/// `nextCursorMark` from the previous response. Up to 1000 results per page.
///
/// The references / citations endpoints use classic page-number pagination.
///
/// # Example
///
/// ```no_run
/// use europepmc::EuropePmcClient;
///
/// let client = EuropePmcClient::new();
/// ```
#[derive(Debug)]
pub struct EuropePmcClient {
    client: Client,
}

impl Default for EuropePmcClient {
    fn default() -> Self {
        Self::new()
    }
}

impl EuropePmcClient {
    /// Create a new client with default settings.
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .user_agent("europepmc-rs-sdk/0.1 (+https://europepmc.org)")
                .build()
                .expect("reqwest client builder"),
        }
    }

    /// Provide a custom `reqwest::Client` (e.g. for timeouts, proxy).
    pub fn with_client(client: Client) -> Self {
        Self { client }
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
        let url = build_url(&base_url(), path, params);
        let resp = self.client.get(&url).send().await?;
        let status = resp.status().as_u16();
        let body = resp.text().await?;
        if !(200..300).contains(&status) {
            return Err(EuropePmcError::Status { status, body });
        }
        serde_json::from_str(&body).map_err(Into::into)
    }

    /// Execute a GET request that returns raw text (e.g. fullTextXML).
    async fn get_text(&self, path: &str, params: &[(&str, String)]) -> Result<String> {
        let url = build_url(&base_url(), path, params);
        let resp = self.client.get(&url).send().await?;
        let status = resp.status().as_u16();
        let body = resp.text().await?;
        if !(200..300).contains(&status) {
            return Err(EuropePmcError::Status { status, body });
        }
        Ok(body)
    }

    // ===================================================================
    // Search
    // ===================================================================

    /// Search Europe PMC for publications matching a query expression.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use europepmc::{EuropePmcClient, types::{SearchRequest, ResultType}};
    /// # async fn run() -> europepmc::error::Result<()> {
    /// let client = EuropePmcClient::new();
    /// let resp = client.search(&SearchRequest::new("p53")
    ///     .result_type(ResultType::Core)
    ///     .page_size(5))
    ///     .await?;
    /// println!("{} results", resp.hit_count);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn search(&self, req: &SearchRequest) -> Result<SearchResponse> {
        let mut params: Vec<(&str, String)> = vec![("query", req.query.clone())];
        params.push(("resultType", req.result_type.as_str().to_owned()));
        params.push(("format", "json".to_owned()));
        if req.synonym {
            params.push(("synonym", "true".to_owned()));
        }
        if let Some(ref c) = req.cursor_mark {
            params.push(("cursorMark", c.clone()));
        }
        if let Some(n) = req.page_size {
            params.push(("pageSize", n.to_string()));
        }
        if let Some(ref s) = req.sort {
            params.push(("sort", s.clone()));
        }
        self.get_json("/search", &params).await
    }

    // ===================================================================
    // Article
    // ===================================================================

    /// Retrieve a single article by source and ID.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use europepmc::{EuropePmcClient, types::{Source, ResultType}};
    /// # async fn run() -> europepmc::error::Result<()> {
    /// let client = EuropePmcClient::new();
    /// let resp = client.article(Source::Med, "29867326", ResultType::Core).await?;
    /// println!("{}", resp.result.as_ref().unwrap().title);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn article(
        &self,
        source: Source,
        id: &str,
        result_type: ResultType,
    ) -> Result<ArticleResponse> {
        let path = format!("/article/{}/{}", source.as_str(), urlencode(id));
        let params: Vec<(&str, String)> = vec![
            ("resultType", result_type.as_str().to_owned()),
            ("format", "json".to_owned()),
        ];
        self.get_json(&path, &params).await
    }

    // ===================================================================
    // References
    // ===================================================================

    /// Retrieve publications referenced by a given publication.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use europepmc::EuropePmcClient;
    /// # async fn run() -> europepmc::error::Result<()> {
    /// let client = EuropePmcClient::new();
    /// let resp = client.references("MED", "29867326", Default::default()).await?;
    /// println!("{} references", resp.hit_count);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn references(
        &self,
        source: &str,
        id: &str,
        page: PageParams,
    ) -> Result<ReferencesResponse> {
        let path = format!("/{}/{}/references", source, urlencode(id));
        let mut params: Vec<(&str, String)> = vec![("format", "json".to_owned())];
        if let Some(p) = page.page {
            params.push(("page", p.to_string()));
        }
        if let Some(ps) = page.page_size {
            params.push(("pageSize", ps.to_string()));
        }
        self.get_json(&path, &params).await
    }

    // ===================================================================
    // Citations
    // ===================================================================

    /// Retrieve publications that cite a given publication.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use europepmc::EuropePmcClient;
    /// # async fn run() -> europepmc::error::Result<()> {
    /// let client = EuropePmcClient::new();
    /// let resp = client.citations("MED", "29867326", Default::default()).await?;
    /// println!("{} citations", resp.hit_count);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn citations(
        &self,
        source: &str,
        id: &str,
        page: PageParams,
    ) -> Result<CitationsResponse> {
        let path = format!("/{}/{}/citations", source, urlencode(id));
        let mut params: Vec<(&str, String)> = vec![("format", "json".to_owned())];
        if let Some(p) = page.page {
            params.push(("page", p.to_string()));
        }
        if let Some(ps) = page.page_size {
            params.push(("pageSize", ps.to_string()));
        }
        self.get_json(&path, &params).await
    }

    // ===================================================================
    // Profile
    // ===================================================================

    /// Obtain a 'profile' of hit counts for publication types and data sources.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use europepmc::EuropePmcClient;
    /// # async fn run() -> europepmc::error::Result<()> {
    /// let client = EuropePmcClient::new();
    /// let resp = client.profile("p53", "all").await?;
    /// if let Some(ref pl) = resp.profile_list {
    ///     if let Some(ref srcs) = pl.sources {
    ///         for s in srcs { println!("  {}: {}", s.name, s.count); }
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn profile(&self, query: &str, profile_type: &str) -> Result<ProfileResponse> {
        let params: Vec<(&str, String)> = vec![
            ("query", query.to_owned()),
            ("profiletype", profile_type.to_owned()),
            ("format", "json".to_owned()),
        ];
        self.get_json("/profile", &params).await
    }

    // ===================================================================
    // Database cross-references
    // ===================================================================

    /// Retrieve biological database records linked to a given publication.
    ///
    /// `database` examples: `"UNIPROT"`, `"EMBL"`, `"CHEBI"`, `"PDB"`. Pass
    /// `None` to return links for all databases.
    pub async fn database_links(
        &self,
        source: &str,
        id: &str,
        database: Option<&str>,
        page: PageParams,
    ) -> Result<serde_json::Value> {
        let path = format!("/{}/{}/databaseLinks", source, urlencode(id));
        let mut params: Vec<(&str, String)> = vec![("format", "json".to_owned())];
        if let Some(db) = database {
            params.push(("database", db.to_owned()));
        }
        if let Some(p) = page.page {
            params.push(("page", p.to_string()));
        }
        if let Some(ps) = page.page_size {
            params.push(("pageSize", ps.to_string()));
        }
        self.get_json(&path, &params).await
    }

    // ===================================================================
    // Full text XML
    // ===================================================================

    /// Retrieve the full text in XML (JATS) format for an Open Access PMC
    /// article.
    ///
    /// `id` must be a PMC ID (e.g. `"PMC3257301"`).
    pub async fn full_text_xml(&self, id: &str) -> Result<String> {
        let path = format!("/{}/fullTextXML", urlencode(id));
        self.get_text(&path, &[]).await
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
        .map(|(k, v)| format!("{}={}", urlencode(k), urlencode(v)))
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
