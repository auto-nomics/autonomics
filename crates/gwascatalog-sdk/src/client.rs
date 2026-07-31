//! Aggregate client owning one [`reqwest::Client`] and three GWAS Catalog
//! base URLs (Summary Statistics, REST, Search).
//!
//! Per-API types and `impl GwasCatalogClient` method blocks live in the
//! [`summary_stats`], [`rest`], and [`search`] modules.

use crate::error::{GwasCatalogError, Result};

/// Summary Statistics API base URL.
pub const SS_BASE: &str = "https://www.ebi.ac.uk/gwas/summary-statistics/api";
/// REST (curated catalog) API base URL.
pub const REST_BASE: &str = "https://www.ebi.ac.uk/gwas/rest/api";
/// Solr Search API base URL.
pub const SEARCH_BASE: &str = "https://www.ebi.ac.uk/gwas/api/search";

/// Aggregate async client for all three GWAS Catalog APIs.
///
/// Method groups per API:
/// - Summary Statistics — see [`summary_stats`](crate::summary_stats) module
/// - REST (curated catalog) — see [`rest`](crate::rest) module
/// - Solr Search — see [`search`](crate::search) module
pub struct GwasCatalogClient {
    client: reqwest::Client,
    ss_base: String,
    rest_base: String,
    search_base: String,
}

/// Backward-compat alias — the original SDK exposed `GwasCatalogApi`.
pub type GwasCatalogApi = GwasCatalogClient;

impl GwasCatalogClient {
    /// Create a client pointing to the production APIs.
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            ss_base: SS_BASE.to_string(),
            rest_base: REST_BASE.to_string(),
            search_base: SEARCH_BASE.to_string(),
        }
    }

    /// Create a client with a custom Summary Statistics base URL (back-compat
    /// with the original `GwasCatalogApi::with_base_url`).
    pub fn with_base_url(ss_base: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            ss_base: ss_base.into(),
            rest_base: REST_BASE.to_string(),
            search_base: SEARCH_BASE.to_string(),
        }
    }

    /// Create a client with custom base URLs for all three APIs.
    pub fn with_base_urls(
        ss_base: impl Into<String>,
        rest_base: impl Into<String>,
        search_base: impl Into<String>,
    ) -> Self {
        Self {
            client: reqwest::Client::new(),
            ss_base: ss_base.into(),
            rest_base: rest_base.into(),
            search_base: search_base.into(),
        }
    }

    pub(crate) fn ss_base(&self) -> &str {
        &self.ss_base
    }

    pub(crate) fn rest_base(&self) -> &str {
        &self.rest_base
    }

    pub(crate) fn search_base(&self) -> &str {
        &self.search_base
    }

    // ── Internal HTTP helper ────────────────────────────────────────────

    /// Perform a GET request and deserialize the JSON body into `T`.
    ///
    /// On non-2xx responses the body is parsed for a `message` field and
    /// returned as [`GwasCatalogError::Api`].
    pub(crate) async fn get<T: serde::de::DeserializeOwned>(
        &self,
        base: &str,
        path: &str,
        query_pairs: &[(&str, String)],
    ) -> Result<T> {
        let mut url = format!("{base}{path}");
        if !query_pairs.is_empty() {
            url.push('?');
            url.push_str(
                &query_pairs
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join("&"),
            );
        }
        let resp = self.client.get(&url).send().await?;
        let status = resp.status();
        if status.is_success() {
            resp.json().await.map_err(GwasCatalogError::Http)
        } else {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let message = body["message"]
                .as_str()
                .unwrap_or("unknown error")
                .to_string();
            Err(GwasCatalogError::Api {
                status: status.as_u16(),
                message,
            })
        }
    }
}

impl Default for GwasCatalogClient {
    fn default() -> Self {
        Self::new()
    }
}
