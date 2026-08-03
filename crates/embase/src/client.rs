use reqwest::Client;
use serde_json::Value;

use crate::error::{EmbaseError, Result};
use crate::types::*;

/// Base URL for all Embase API requests.
const BASE_URL: &str = "https://api.elsevier.com/content/embase/article";

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// An async client for the [Elsevier Embase API](
/// https://nonprod-devportal.elsevier.com/embase_apis.html).
///
/// All endpoints are covered:
/// - **Search** — text search with Embase CommandLanguage syntax.
/// - **Retrieval** — fetch a single record by DOI, PII, PMID, Embase
///   accession number, MEDLINE ID, or LUI.
///
/// # Authentication
///
/// An API key is **required** for all requests. Obtain one from the
/// [Elsevier Developer Portal](https://dev.elsevier.com/apikey/manage).
///
/// Optionally provide an institution token (`insttoken`) or an OAuth
/// authtoken for user-level entitlements.
///
/// # Example
///
/// ```no_run
/// use embase::EmbaseClient;
///
/// let client = EmbaseClient::new("your-api-key", None, None);
/// ```
#[derive(Debug, Clone)]
pub struct EmbaseClient {
    client: Client,
    api_key: String,
    inst_token: Option<String>,
    auth_token: Option<String>,
}

impl EmbaseClient {
    /// Create a new client.
    ///
    /// * `api_key`     – Elsevier API key (required).
    /// * `inst_token`  – Optional institution token.
    /// * `auth_token`  – Optional OAuth bearer token for user-based entitlements.
    pub fn new(api_key: &str, inst_token: Option<&str>, auth_token: Option<&str>) -> Self {
        Self {
            client: Client::new(),
            api_key: api_key.to_owned(),
            inst_token: inst_token.map(str::to_owned),
            auth_token: auth_token.map(str::to_owned),
        }
    }

    /// Create a client reading credentials from environment variables.
    ///
    /// | Variable             | Purpose                              |
    /// |----------------------|--------------------------------------|
    /// | `EMBASE_API_KEY`     | API key (**required**)               |
    /// | `EMBASE_INSTTOKEN`   | Institution token (optional)         |
    /// | `EMBASE_AUTHTOKEN`   | OAuth bearer token (optional)        |
    ///
    /// Returns [`EmbaseError::MissingApiKey`] if `EMBASE_API_KEY` is unset.
    pub fn from_env() -> Self {
        let api_key = std::env::var("EMBASE_API_KEY").unwrap_or_default();
        let inst_token = std::env::var("EMBASE_INSTTOKEN").ok();
        let auth_token = std::env::var("EMBASE_AUTHTOKEN").ok();
        Self {
            api_key,
            inst_token,
            auth_token,
            client: Client::new(),
        }
    }

    /// Returns `Ok(())` if the client has a non-empty API key.
    fn require_api_key(&self) -> Result<()> {
        if self.api_key.is_empty() {
            Err(EmbaseError::MissingApiKey)
        } else {
            Ok(())
        }
    }

    // ----- HTTP helpers -----

    /// Attach the common authentication headers to a request builder.
    fn add_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let req = req
            .header("Accept", "application/json")
            .header("X-ELS-APIKey", &self.api_key);
        let req = if let Some(ref t) = self.inst_token {
            req.header("X-ELS-Insttoken", t)
        } else {
            req
        };
        if let Some(ref t) = self.auth_token {
            req.header("X-ELS-Authtoken", t)
        } else {
            req
        }
    }

    /// Execute a GET request and return the raw JSON value.
    async fn get_json(&self, url: &str) -> Result<Value> {
        self.require_api_key()?;
        let resp = self.add_auth(self.client.get(url)).send().await?;
        let status = resp.status().as_u16();
        let body = resp.text().await?;
        if !(200..300).contains(&status) {
            return Err(EmbaseError::Status { status, body });
        }
        serde_json::from_str(&body).map_err(Into::into)
    }

    // ===================================================================
    // Search API
    // ===================================================================

    /// Search Embase for records matching a CommandLanguage query.
    ///
    /// Returns matching records with metadata (title, authors, DOI, journal,
    /// abstract, etc.).
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use embase::{EmbaseClient, types::SearchRequest};
    /// # async fn run() -> embase::error::Result<()> {
    /// let client = EmbaseClient::from_env();
    /// let resp = client.search(&SearchRequest::new("'CRISPR':ti,ab")).await?;
    /// println!("{} results", resp.total_results);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn search(&self, req: &SearchRequest) -> Result<SearchResponse> {
        let mut params: Vec<(&str, String)> = Vec::new();

        // Either `query` or `alertid` must be present.
        if let Some(ref aid) = req.alert_id {
            params.push(("alertid", aid.clone()));
        } else {
            params.push(("query", req.query.clone()));
        }
        if let Some(n) = req.count {
            params.push(("count", n.to_string()));
        }
        if let Some(n) = req.start {
            params.push(("start", n.to_string()));
        }
        if let Some(ref s) = req.sort {
            params.push(("sort", s.clone()));
        }

        let url = build_url(BASE_URL, &params);
        let v = self.get_json(&url).await?;
        serde_json::from_value(v).map_err(Into::into)
    }

    // ===================================================================
    // Retrieval API
    // ===================================================================

    /// Retrieve a single Embase record by identifier type and value.
    ///
    /// This is the generic backend used by all the `retrieve_by_*` methods.
    /// You normally won't call this directly — use the convenience wrappers.
    pub async fn retrieve(&self, id_type: RetrievalId, id: &str) -> Result<RetrievalResponse> {
        let url = format!(
            "{}/{}/{}",
            BASE_URL,
            id_type.path_segment(),
            urlencoding(id)
        );
        let v = self.get_json(&url).await?;
        serde_json::from_value(v).map_err(Into::into)
    }

    /// Retrieve a record by DOI.
    pub async fn retrieve_by_doi(&self, doi: &str) -> Result<RetrievalResponse> {
        self.retrieve(RetrievalId::Doi, doi).await
    }

    /// Retrieve a record by PII (Publication Item Identifier).
    pub async fn retrieve_by_pii(&self, pii: &str) -> Result<RetrievalResponse> {
        self.retrieve(RetrievalId::Pii, pii).await
    }

    /// Retrieve a record by PubMed ID.
    pub async fn retrieve_by_pmid(&self, pmid: &str) -> Result<RetrievalResponse> {
        self.retrieve(RetrievalId::PubmedId, pmid).await
    }

    /// Retrieve a record by MEDLINE ID.
    pub async fn retrieve_by_medline(&self, id: &str) -> Result<RetrievalResponse> {
        self.retrieve(RetrievalId::Medline, id).await
    }

    /// Retrieve a record by Embase accession number.
    pub async fn retrieve_by_embase_id(&self, id: &str) -> Result<RetrievalResponse> {
        self.retrieve(RetrievalId::Embase, id).await
    }

    /// Retrieve a record by LUI (Local Unique Identifier).
    pub async fn retrieve_by_lui(&self, lui: &str) -> Result<RetrievalResponse> {
        self.retrieve(RetrievalId::Lui, lui).await
    }
}

// ---------------------------------------------------------------------------
// URL helpers
// ---------------------------------------------------------------------------

/// Build a GET URL with encoded query parameters.
fn build_url(base: &str, params: &[(&str, String)]) -> String {
    if params.is_empty() {
        return base.to_owned();
    }
    let qs: Vec<String> = params
        .iter()
        .map(|(k, v)| format!("{}={}", urlencode(k), urlencode(v)))
        .collect();
    format!("{}?{}", base, qs.join("&"))
}

/// Minimal percent-encoding for URL query values.
/// Encodes characters that are not allowed unencoded in a URL query string.
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

/// Encode only the path segment of a URL (encodes `/` as `%2F`).
fn urlencoding(s: &str) -> String {
    urlencode(s)
}
