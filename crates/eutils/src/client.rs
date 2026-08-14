use std::collections::HashMap;

use reqwest::Client;
use serde_json::Value;

use crate::error::{EutilsError, Result};
use crate::types::*;

fn env_endpoint(env: &str, fallback: &str) -> String {
    std::env::var(env).unwrap_or_else(|_| fallback.to_string())
}

/// Base URL for all NCBI E-utility requests.
const BASE_URL: &str = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils";

fn base_url() -> String {
    env_endpoint("ENDPOINT_EUTILS_URL", BASE_URL)
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// An async client for the [NCBI Entrez Programming Utilities (E-utilities)](
/// https://www.ncbi.nlm.nih.gov/books/NBK25497/).
///
/// # Rate limits
///
/// Without an API key: ≤ 3 requests/second per IP.
/// With an API key:    ≤ 10 requests/second per IP.
///
/// Register your `tool` and `email` with NCBI to restore access if your IP
/// gets blocked (see Chapter 2 of the E-utilities documentation).
///
/// # Example
///
/// ```no_run
/// use eutils::EutilsClient;
///
/// let client = EutilsClient::new("my-tool", "dev@example.com", None);
/// ```
#[derive(Debug, Clone)]
pub struct EutilsClient {
    client: Client,
    tool: String,
    email: String,
    api_key: Option<String>,
}

impl EutilsClient {
    /// Create a new client.
    ///
    /// * `tool`   – a short identifier for your software (no spaces).
    /// * `email`  – a valid contact e-mail address.
    /// * `api_key` – optional NCBI API key for elevated rate limits.
    pub fn new(tool: &str, email: &str, api_key: Option<&str>) -> Self {
        Self {
            client: Client::new(),
            tool: tool.to_owned(),
            email: email.to_owned(),
            api_key: api_key.map(str::to_owned),
        }
    }

    /// Create a client using default tool/email values read from environment
    /// variables `EUTILS_TOOL` and `EUTILS_EMAIL`, with an optional API key
    /// from `EUTILS_API_KEY`.
    pub fn from_env() -> Self {
        Self::new(
            &std::env::var("EUTILS_TOOL").unwrap_or_else(|_| "agentik".into()),
            &std::env::var("EUTILS_EMAIL").unwrap_or_else(|_| "agentik@localhost".into()),
            std::env::var("EUTILS_API_KEY").ok().as_deref(),
        )
    }

    // ----- helpers -----

    /// Inject the common `tool`, `email`, and optional `api_key` parameters.
    fn inject_common(&self, params: &mut Vec<(&str, String)>) {
        params.push(("tool", self.tool.clone()));
        params.push(("email", self.email.clone()));
        if let Some(ref key) = self.api_key {
            params.push(("api_key", key.clone()));
        }
    }

    /// Build a GET URL and fetch JSON.
    async fn get_json(&self, endpoint: &str, params: Vec<(&str, String)>) -> Result<Value> {
        let mut p = params;
        self.inject_common(&mut p);

        let url = format!("{}/{endpoint}", base_url());
        let resp = self.client.get(&url).query(&p).send().await?;
        let status = resp.status().as_u16();
        let body = resp.text().await?;

        if !(200..300).contains(&status) {
            return Err(EutilsError::Status { status, body });
        }
        serde_json::from_str(&body).map_err(Into::into)
    }

    /// Build a GET URL and fetch raw text.
    async fn get_text(&self, endpoint: &str, params: Vec<(&str, String)>) -> Result<String> {
        let mut p = params;
        self.inject_common(&mut p);

        let url = format!("{}/{endpoint}", base_url());
        let resp = self.client.get(&url).query(&p).send().await?;
        let status = resp.status().as_u16();
        let body = resp.text().await?;

        if !(200..300).contains(&status) {
            return Err(EutilsError::Status { status, body });
        }
        Ok(body)
    }

    /// POST form-encoded params and fetch JSON.
    async fn post_json(&self, endpoint: &str, params: HashMap<&str, String>) -> Result<Value> {
        let mut p = params;
        p.insert("tool", self.tool.clone());
        p.insert("email", self.email.clone());
        if let Some(ref key) = self.api_key {
            p.insert("api_key", key.clone());
        }

        let url = format!("{}/{endpoint}", base_url());
        let resp = self.client.post(&url).form(&p).send().await?;
        let status = resp.status().as_u16();
        let body = resp.text().await?;

        if !(200..300).contains(&status) {
            return Err(EutilsError::Status { status, body });
        }
        serde_json::from_str(&body).map_err(Into::into)
    }

    /// POST form-encoded params and fetch raw text (used for large ID lists).
    async fn post_text(&self, endpoint: &str, params: HashMap<&str, String>) -> Result<String> {
        let mut p = params;
        p.insert("tool", self.tool.clone());
        p.insert("email", self.email.clone());
        if let Some(ref key) = self.api_key {
            p.insert("api_key", key.clone());
        }

        let url = format!("{}/{endpoint}", base_url());
        let resp = self.client.post(&url).form(&p).send().await?;
        let status = resp.status().as_u16();
        let body = resp.text().await?;

        if !(200..300).contains(&status) {
            return Err(EutilsError::Status { status, body });
        }
        Ok(body)
    }

    // ===================================================================
    // EInfo – database statistics
    // ===================================================================

    /// Retrieve database statistics.
    ///
    /// If `db` is `None`, returns a list of all valid Entrez database names.
    /// If `db` is `Some(name)`, returns field counts, links, and last-update
    /// info for that database.
    pub async fn einfo(&self, db: Option<&str>) -> Result<EInfoResponse> {
        let mut params = vec![("retmode", "json".into())];
        if let Some(db) = db {
            params.push(("db", db.to_owned()));
        }
        let v = self.get_json("einfo.fcgi", params).await?;
        serde_json::from_value(v).map_err(Into::into)
    }

    // ===================================================================
    // ESearch – text searches
    // ===================================================================

    /// Search an Entrez database for UIDs matching a text query.
    ///
    /// Returns the list of matching UIDs, total count, and (optionally) a
    /// web environment for chaining into ESummary / EFetch / ELink.
    pub async fn esearch(&self, req: &ESearchRequest) -> Result<ESearchResponse> {
        let mut params = vec![
            ("db", req.db.clone()),
            ("term", req.term.clone()),
            ("retmode", "json".into()),
        ];
        if let Some(n) = req.retmax {
            params.push(("retmax", n.to_string()));
        }
        if let Some(n) = req.retstart {
            params.push(("retstart", n.to_string()));
        }
        if let Some(ref s) = req.sort {
            params.push(("sort", s.clone()));
        }
        if req.usehistory == Some(true) {
            params.push(("usehistory", "y".into()));
        }
        if let Some(ref w) = req.web_env {
            params.push(("WebEnv", w.clone()));
        }
        if let Some(ref k) = req.query_key {
            params.push(("query_key", k.clone()));
        }
        if let Some(ref dt) = req.datetype {
            params.push(("datetype", dt.clone()));
        }
        if let Some(n) = req.reldate {
            params.push(("reldate", n.to_string()));
        }
        if let Some(ref d) = req.mindate {
            params.push(("mindate", d.clone()));
        }
        if let Some(ref d) = req.maxdate {
            params.push(("maxdate", d.clone()));
        }

        let v = self.get_json("esearch.fcgi", params).await?;
        serde_json::from_value(v).map_err(Into::into)
    }

    // ===================================================================
    // ESummary – document summary downloads
    // ===================================================================

    /// Retrieve document summaries for a list of UIDs.
    ///
    /// Returns structured JSON. For PubMed, consider adding
    /// `version: Some("2.0".into())` to `req` for richer DocSums.
    pub async fn esummary(&self, req: &ESummaryRequest) -> Result<Value> {
        let mut params = vec![
            ("db", req.db.clone()),
            ("id", req.id.clone()),
            ("retmode", "json".into()),
        ];
        if let Some(n) = req.retmax {
            params.push(("retmax", n.to_string()));
        }
        if let Some(n) = req.retstart {
            params.push(("retstart", n.to_string()));
        }
        if let Some(ref v) = req.version {
            params.push(("version", v.clone()));
        }
        self.get_json("esummary.fcgi", params).await
    }

    // ===================================================================
    // EFetch – data record downloads
    // ===================================================================

    /// Retrieve formatted data records for UIDs.
    ///
    /// Returns raw text (e.g. MEDLINE or abstract format for PubMed).
    /// Use `rettype: Some("abstract")` and `retmode: Some("text")` for
    /// human-readable PubMed abstracts.
    pub async fn efetch(&self, req: &EFetchRequest) -> Result<String> {
        // Use POST for large ID lists (> 200 IDs) as recommended by NCBI.
        if req.id.matches(',').count() > 200 {
            let mut form = HashMap::new();
            form.insert("db", req.db.clone());
            // Only send `id` when non-empty; an empty `id=` alongside a History
            // server (WebEnv/query_key) makes NCBI return zero records.
            if !req.id.is_empty() {
                form.insert("id", req.id.clone());
            }
            if let Some(ref t) = req.rettype {
                form.insert("rettype", t.clone());
            }
            if let Some(ref m) = req.retmode {
                form.insert("retmode", m.clone());
            }
            if let Some(n) = req.retmax {
                form.insert("retmax", n.to_string());
            }
            if let Some(n) = req.retstart {
                form.insert("retstart", n.to_string());
            }
            if let Some(ref w) = req.web_env {
                form.insert("WebEnv", w.clone());
            }
            if let Some(ref k) = req.query_key {
                form.insert("query_key", k.clone());
            }
            return self.post_text("efetch.fcgi", form).await;
        }

        let mut params = vec![("db", req.db.clone())];
        // Only send `id` when non-empty; an empty `id=` alongside a History
        // server (WebEnv/query_key) makes NCBI return zero records.
        if !req.id.is_empty() {
            params.push(("id", req.id.clone()));
        }
        if let Some(ref t) = req.rettype {
            params.push(("rettype", t.clone()));
        }
        if let Some(ref m) = req.retmode {
            params.push(("retmode", m.clone()));
        }
        if let Some(n) = req.retmax {
            params.push(("retmax", n.to_string()));
        }
        if let Some(n) = req.retstart {
            params.push(("retstart", n.to_string()));
        }
        if let Some(ref w) = req.web_env {
            params.push(("WebEnv", w.clone()));
        }
        if let Some(ref k) = req.query_key {
            params.push(("query_key", k.clone()));
        }
        self.get_text("efetch.fcgi", params).await
    }

    // ===================================================================
    // ELink – Entrez links
    // ===================================================================

    /// Retrieve UIDs linked to a set of input UIDs.
    ///
    /// Common uses:
    /// - Find related PubMed articles (`dbfrom: "pubmed", db: Some("pubmed")`)
    /// - Link genes to PubMed articles (`dbfrom: "gene", db: Some("pubmed")`)
    pub async fn elink(&self, req: &ELinkRequest) -> Result<Value> {
        let mut params = vec![("dbfrom", req.dbfrom.clone()), ("retmode", "json".into())];
        // ELink uses one repeated `id` parameter per UID. Comma-separated values
        // would be merged into a single linkset server-side, so we split here to
        // preserve per-UID grouping (one linkset per input UID).
        for id in req.id.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            params.push(("id", id.to_owned()));
        }
        if let Some(ref db) = req.db {
            params.push(("db", db.clone()));
        }
        if let Some(ref cmd) = req.cmd {
            params.push(("cmd", cmd.clone()));
        }
        if let Some(ref ln) = req.linkname {
            params.push(("linkname", ln.clone()));
        }
        self.get_json("elink.fcgi", params).await
    }

    // ===================================================================
    // EGQuery – global query
    // ===================================================================

    /// Run a text query across all Entrez databases simultaneously.
    ///
    /// Returns the count of matching records for each database.
    ///
    /// **Note:** EGQuery requires HTTP POST (GET returns 301 redirect).
    pub async fn egquery(&self, term: &str) -> Result<EGQueryResponse> {
        let mut p = HashMap::new();
        p.insert("term", term.to_owned());
        p.insert("retmode", "json".into());
        let v = self.post_json("egquery.fcgi", p).await?;
        serde_json::from_value(v).map_err(Into::into)
    }

    // ===================================================================
    // ESpell – spelling suggestions
    // ===================================================================

    /// Retrieve spelling suggestions for a text query.
    pub async fn espell(&self, db: &str, term: &str) -> Result<Value> {
        let params = vec![
            ("db", db.to_owned()),
            ("term", term.to_owned()),
            ("retmode", "json".into()),
        ];
        self.get_json("espell.fcgi", params).await
    }

    // ===================================================================
    // EPost – UID uploads
    // ===================================================================

    /// Upload a list of UIDs to the Entrez History server.
    ///
    /// Returns a `(query_key, web_env)` pair for chaining into ESummary,
    /// EFetch, or ELink.
    pub async fn epost(&self, db: &str, id: &[String]) -> Result<(String, String)> {
        let mut form = HashMap::new();
        form.insert("db", db.to_owned());
        form.insert("id", id.join(","));
        let text = self.post_text("epost.fcgi", form).await?;

        // EPost returns XML — extract <QueryKey> and <WebEnv>.
        let query_key = extract_xml_field(&text, "QueryKey")
            .ok_or_else(|| EutilsError::Parse(serde_json::from_str::<Value>("{}").unwrap_err()))?;
        let web_env = extract_xml_field(&text, "WebEnv")
            .ok_or_else(|| EutilsError::Parse(serde_json::from_str::<Value>("{}").unwrap_err()))?;
        Ok((query_key, web_env))
    }

    // ===================================================================
    // ECitMatch – batch citation matching
    // ===================================================================

    /// Retrieve PMIDs for a batch of citation strings.
    ///
    /// Each citation string must follow the format:
    /// `journal_name|volume|page|year|authors|title`
    pub async fn ecitmatch(&self, req: &ECitMatchRequest) -> Result<String> {
        let bdata = req.bdata.join("\r\n");
        let mut form = HashMap::new();
        form.insert("db", "pubmed".to_owned());
        form.insert("rettype", "xml".to_owned());
        form.insert("bdata", bdata);
        self.post_text("ecitmatch.cgi", form).await
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract a text value from a simple XML field (lightweight, no dep needed).
fn extract_xml_field(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml.find(&close)?;
    Some(xml[start..end].to_owned())
}
