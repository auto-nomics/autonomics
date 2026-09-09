//! HTTP client for the KEGG REST API.

use std::sync::Arc;

use reqwest::{Method, Url};
use serde::de::DeserializeOwned;

use crate::error::{KeggError, Result};
use crate::parser;
use crate::rate::RateLimiter;
use crate::types::{Binary, DrugInteraction, EntrySummary, FlatEntry, Info, Pair};

/// Default KEGG REST endpoint.
pub const DEFAULT_KEGG_ENDPOINT: &str = "https://rest.kegg.jp";
/// KEGG's documented maximum request rate for academic users.
pub const KEGG_RATE_LIMIT_PER_SECOND: u32 = 3;
/// Maximum number of flat-file entries accepted by the `get` operation.
pub const MAX_GET_ENTRIES: usize = 10;

const USER_AGENT: &str = "kegg-rs-sdk/0.1 (+https://www.kegg.jp/kegg/rest/)";

/// Async client for the KEGG REST API.
///
/// The client reuses one connection pool and serializes requests through a
/// process-local limiter set to the documented maximum of three requests per
/// second. It is intended for academic use; non-academic deployments need a
/// KEGG license.
#[derive(Debug, Clone)]
pub struct KeggClient {
    http: reqwest::Client,
    base_url: String,
    limiter: Arc<RateLimiter>,
}

impl Default for KeggClient {
    fn default() -> Self {
        Self::new()
    }
}

impl KeggClient {
    /// Create a production client honoring `ENDPOINT_KEGG_URL`.
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .expect("reqwest client builder");
        let base_url = std::env::var("ENDPOINT_KEGG_URL")
            .unwrap_or_else(|_| DEFAULT_KEGG_ENDPOINT.to_string());
        Self::with_client_and_endpoint_and_rate_limit(http, base_url, KEGG_RATE_LIMIT_PER_SECOND)
            .expect("default endpoint and rate limit are valid")
    }

    /// Create a production client with a custom per-second request limit.
    pub fn with_rate_limit(max_per_second: u32) -> Result<Self> {
        let http = reqwest::Client::builder().user_agent(USER_AGENT).build()?;
        let base_url = std::env::var("ENDPOINT_KEGG_URL")
            .unwrap_or_else(|_| DEFAULT_KEGG_ENDPOINT.to_string());
        Self::with_client_and_endpoint_and_rate_limit(http, base_url, max_per_second)
    }

    /// Use an existing HTTP client, endpoint, and rate limit.
    pub fn with_client_and_endpoint_and_rate_limit(
        http: reqwest::Client,
        base_url: impl Into<String>,
        max_per_second: u32,
    ) -> Result<Self> {
        Ok(Self {
            http,
            base_url: base_url.into(),
            limiter: Arc::new(RateLimiter::new(max_per_second)?),
        })
    }

    /// Override only the endpoint after construction.
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.base_url = endpoint.into();
        self
    }

    /// Return the configured REST endpoint.
    pub fn endpoint(&self) -> &str {
        &self.base_url
    }

    async fn get_text(&self, operation: &str) -> Result<String> {
        self.limiter.acquire().await;
        let response = self
            .http
            .request(Method::GET, self.url(operation)?)
            .header("Accept", "text/plain, application/xml, application/json")
            .send()
            .await?;
        finish_text(response).await
    }

    async fn get_binary(&self, operation: &str) -> Result<Binary> {
        self.limiter.acquire().await;
        let response = self
            .http
            .request(Method::GET, self.url(operation)?)
            .header("Accept", "image/png, image/gif, application/octet-stream")
            .send()
            .await?;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();
        let bytes = response.bytes().await?;
        if !(200..300).contains(&status) {
            return Err(KeggError::Status {
                status,
                body: String::from_utf8_lossy(&bytes).to_string(),
            });
        }
        Ok(Binary {
            content_type,
            bytes: bytes.to_vec(),
        })
    }

    fn url(&self, operation: &str) -> Result<Url> {
        let mut url = Url::parse(&format!("{}/", self.base_url.trim_end_matches('/')))?;
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| KeggError::InvalidParameter("endpoint cannot be a base URL".into()))?;
            segments.extend(operation.split('/').filter(|part| !part.is_empty()));
        }
        Ok(url)
    }

    /// Execute an arbitrary GET operation and return its text response.
    ///
    /// This is an escape hatch for new or rarely used KEGG operations.
    pub async fn operation(&self, operation: &str) -> Result<String> {
        require_non_empty(operation, "operation")?;
        self.get_text(operation).await
    }

    /// Fetch database release and linked-database metadata.
    pub async fn info(&self, database: &str) -> Result<Info> {
        require_non_empty(database, "database")?;
        let raw = self.get_text(&format!("info/{database}")).await?;
        Ok(parser::info(&raw))
    }

    /// List all entries in a KEGG database or organism.
    pub async fn list(&self, database: &str) -> Result<Vec<EntrySummary>> {
        require_non_empty(database, "database")?;
        let raw = self.get_text(&format!("list/{database}")).await?;
        Ok(parser::entry_summaries(&raw))
    }

    /// Search a KEGG database with the API's native query syntax.
    pub async fn find(&self, database: &str, query: &str) -> Result<Vec<EntrySummary>> {
        self.find_with_option(database, query, None).await
    }

    /// Search a KEGG database with a native `find` option.
    pub async fn find_with_option(
        &self,
        database: &str,
        query: &str,
        option: Option<&str>,
    ) -> Result<Vec<EntrySummary>> {
        require_non_empty(database, "database")?;
        require_non_empty(query, "query")?;
        let mut operation = format!("find/{database}/{}", percent_path(query));
        if let Some(option) = option {
            require_non_empty(option, "option")?;
            operation.push('/');
            operation.push_str(option);
        }
        let raw = self.get_text(&operation).await?;
        Ok(parser::entry_summaries(&raw))
    }

    /// Fetch one or more flat-file entries, up to KEGG's limit of ten.
    pub async fn get(&self, entries: &str) -> Result<String> {
        self.get_with_option(entries, None).await
    }

    /// Fetch flat-file entries with a KEGG `get` option.
    pub async fn get_with_option(&self, entries: &str, option: Option<&str>) -> Result<String> {
        validate_get_entries(entries)?;
        let mut operation = format!("get/{}", percent_path(entries));
        if let Some(option) = option {
            require_non_empty(option, "option")?;
            operation.push('/');
            operation.push_str(option);
        }
        self.get_text(&operation).await
    }

    /// Fetch and parse the `ENTRY` header of one flat-file entry.
    pub async fn flat_entry(&self, entry: &str) -> Result<FlatEntry> {
        let raw = self.get(entry).await?;
        Ok(parser::flat_entry(&raw))
    }

    /// Convert identifiers between KEGG and an integrated outside database.
    pub async fn conv(&self, target: &str, source: &str) -> Result<Vec<Pair>> {
        require_non_empty(target, "target")?;
        require_non_empty(source, "source")?;
        let raw = self
            .get_text(&format!("conv/{}/{}", target, percent_path(source)))
            .await?;
        Ok(parser::pairs(&raw))
    }

    /// Return relationships between KEGG databases or selected entries.
    pub async fn link(&self, target: &str, source: &str) -> Result<Vec<Pair>> {
        require_non_empty(target, "target")?;
        require_non_empty(source, "source")?;
        let raw = self
            .get_text(&format!("link/{}/{}", target, percent_path(source)))
            .await?;
        Ok(parser::pairs(&raw))
    }

    /// Query drug-drug interactions.
    pub async fn ddi(&self, entries: &str) -> Result<Vec<DrugInteraction>> {
        require_non_empty(entries, "entries")?;
        let raw = self
            .get_text(&format!("ddi/{}", percent_path(entries)))
            .await?;
        Ok(parser::drug_interactions(&raw))
    }

    /// Fetch KGML for one pathway.
    pub async fn kgml(&self, pathway: &str) -> Result<String> {
        require_non_empty(pathway, "pathway")?;
        self.get_with_option(pathway, Some("kgml")).await
    }

    /// Fetch a pathway image at standard or doubled resolution.
    pub async fn pathway_image(&self, pathway: &str, double_size: bool) -> Result<Binary> {
        require_non_empty(pathway, "pathway")?;
        let option = if double_size { "image2x" } else { "image" };
        self.get_binary(&format!("get/{}/{}", pathway, option))
            .await
    }

    /// Fetch a BRITE hierarchy as JSON.
    pub async fn brite_json<T: DeserializeOwned>(&self, hierarchy: &str) -> Result<T> {
        require_non_empty(hierarchy, "hierarchy")?;
        let raw = self
            .get_text(&format!("get/{}/json", percent_path(hierarchy)))
            .await?;
        Ok(serde_json::from_str(&raw)?)
    }
}

async fn finish_text(response: reqwest::Response) -> Result<String> {
    let status = response.status().as_u16();
    let body = response.text().await?;
    if !(200..300).contains(&status) {
        return Err(KeggError::Status { status, body });
    }
    Ok(body)
}

fn require_non_empty(value: &str, name: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(KeggError::InvalidParameter(format!(
            "{name} cannot be empty"
        )));
    }
    Ok(())
}

fn validate_get_entries(entries: &str) -> Result<()> {
    require_non_empty(entries, "entries")?;
    let count = entries
        .split('+')
        .filter(|part| !part.trim().is_empty())
        .count();
    if count == 0 || count > MAX_GET_ENTRIES {
        return Err(KeggError::InvalidParameter(format!(
            "get supports between 1 and {MAX_GET_ENTRIES} entries"
        )));
    }
    Ok(())
}

fn percent_path(value: &str) -> &str {
    value
}
