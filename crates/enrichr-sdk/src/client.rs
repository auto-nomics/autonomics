//! Async client for the Enrichr and Speedrichr REST APIs.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use tokio::time::sleep;

use crate::error::{EnrichrError, Result};
use crate::request::{
    EnrichrHost, joined_lines, normalize_genes, validate_background_id, validate_gene_symbol,
    validate_library_name, validate_user_list_id,
};
use crate::types::*;

/// Default Enrichr endpoint (human).
pub const DEFAULT_ENRICHR_ENDPOINT: &str = "https://maayanlab.cloud/Enrichr";
/// Default Speedrichr endpoint (human).
pub const DEFAULT_SPEEDRICHR_ENDPOINT: &str = "https://maayanlab.cloud/speedrichr/api";

const USER_AGENT: &str = "enrichr-sdk-rs/0.1 (+https://maayanlab.cloud/Enrichr)";
const MIN_REQUEST_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Default)]
struct RateLimit {
    next_ready_at: Option<Instant>,
}

/// Async client for the Enrichr gene-set enrichment API and its Speedrichr
/// background-corrected companion.
///
/// Both services are free and anonymous. The client spaces requests by one
/// second to honor Enrichr's fair-use guidance and reuses one connection
/// pool.
///
/// # Example
///
/// ```no_run
/// # use enrichr_sdk::EnrichrClient;
/// # async fn run() -> enrichr_sdk::Result<()> {
/// let client = EnrichrClient::new();
/// let added = client
///     .add_list(["TP53", "BRCA1", "EGFR"], "oncogene probe")
///     .await?;
/// let result = client
///     .enrich(added.user_list_id, "KEGG_2021_Human")
///     .await?;
/// println!("{} terms in {}", result.terms.len(), result.library);
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct EnrichrClient {
    http: reqwest::Client,
    base_url: String,
    speedrichr_url: String,
    rate_limit: Mutex<RateLimit>,
    rate_limited: bool,
}

impl Default for EnrichrClient {
    fn default() -> Self {
        Self::new()
    }
}

impl EnrichrClient {
    /// Create a client for the human Enrichr host with one-second pacing.
    pub fn new() -> Self {
        Self::builder()
            .build()
            .expect("valid default Enrichr client")
    }

    /// Configure endpoints, HTTP client, and rate limiting.
    pub fn builder() -> EnrichrClientBuilder {
        EnrichrClientBuilder {
            http: None,
            base_url: std::env::var("ENDPOINT_ENRICHR_URL")
                .ok()
                .filter(|url| !url.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_ENRICHR_ENDPOINT.to_owned()),
            speedrichr_url: std::env::var("ENDPOINT_SPEEDRICHR_URL")
                .ok()
                .filter(|url| !url.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_SPEEDRICHR_ENDPOINT.to_owned()),
            rate_limited: true,
        }
    }

    /// Use a stable non-human host, for example [`EnrichrHost::Fly`].
    pub fn with_host(mut self, host: EnrichrHost) -> Self {
        self.base_url = host.endpoint().to_owned();
        self.speedrichr_url = host.speedrichr_endpoint().to_owned();
        self
    }

    /// Override the Enrichr base URL.
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.base_url = normalize_base(endpoint);
        self
    }

    /// Borrow the configured Enrichr base URL.
    pub fn endpoint(&self) -> &str {
        &self.base_url
    }

    /// Shareable Enrichr URL for a `shortId` returned by `addList`.
    pub fn share_url(&self, short_id: &str) -> String {
        format!("{}/enrich?dataset={}", self.base_url, short_id)
    }

    // -----------------------------------------------------------------------
    // Core endpoints
    // -----------------------------------------------------------------------

    /// Metadata for every available gene-set library.
    pub async fn dataset_statistics(&self) -> Result<DatasetStatistics> {
        self.get_json("datasetStatistics", &[]).await
    }

    /// Sorted library names accepted as `backgroundType`.
    pub async fn libraries(&self) -> Result<Vec<String>> {
        Ok(self.dataset_statistics().await?.library_names())
    }

    /// Submit a gene list (`POST /addList`, multipart form).
    pub async fn add_list<S, I>(
        &self,
        genes: I,
        description: impl Into<Option<&str>>,
    ) -> Result<AddedList>
    where
        S: AsRef<str>,
        I: IntoIterator<Item = S>,
    {
        let genes = normalize_genes(&genes.into_iter().collect::<Vec<_>>(), "genes")?;
        let description = description
            .into()
            .map(str::to_owned)
            .unwrap_or_else(|| crate::request::DEFAULT_LIST_DESCRIPTION.to_owned());
        let form = reqwest::multipart::Form::new()
            .text("list", joined_lines(&genes))
            .text("description", description);
        self.post_multipart_json("addList", form).await
    }

    /// Retrieve a previously submitted list (`GET /view`).
    pub async fn view(&self, user_list_id: u64) -> Result<ViewedList> {
        validate_user_list_id(user_list_id)?;
        self.get_json("view", &[("userListId", user_list_id.to_string())])
            .await
    }

    /// Run over-representation enrichment (`GET /enrich`).
    ///
    /// An unknown `background_type` yields HTTP 200 with an empty object,
    /// which this method reports as [`EnrichrError::Api`] pointing at
    /// [`Self::libraries`].
    pub async fn enrich(
        &self,
        user_list_id: u64,
        background_type: &str,
    ) -> Result<EnrichmentResult> {
        validate_user_list_id(user_list_id)?;
        let library = validate_library_name(background_type)?;
        let map: BTreeMap<String, Vec<GeneSetTerm>> = self
            .get_json(
                "enrich",
                &[
                    ("userListId", user_list_id.to_string()),
                    ("backgroundType", library),
                ],
            )
            .await?;
        EnrichmentResult::from_singleton_map(map)
    }

    /// Export enrichment results as the tab-separated table served by
    /// `GET /export` (columns: Term, Overlap, P-value, Adjusted P-value,
    /// Old P-value, Old Adjusted P-value, Odds Ratio, Combined Score, Genes).
    pub async fn export(
        &self,
        user_list_id: u64,
        background_type: &str,
        filename: &str,
    ) -> Result<String> {
        validate_user_list_id(user_list_id)?;
        let library = validate_library_name(background_type)?;
        let filename = if filename.trim().is_empty() {
            "enrichr-results"
        } else {
            filename.trim()
        };
        self.get_text(
            "export",
            &[
                ("userListId", user_list_id.to_string()),
                ("backgroundType", library),
                ("filename", filename.to_owned()),
            ],
        )
        .await
    }

    /// Download a gene-set library in GMT format (`GET /geneSetLibrary`).
    ///
    /// Lines are `term<TAB>description<TAB>gene1<TAB>gene2...`; the
    /// description column is usually empty.
    pub async fn gene_set_library(&self, library_name: &str) -> Result<String> {
        let library = validate_library_name(library_name)?;
        self.get_text(
            "geneSetLibrary",
            &[("mode", "text".to_owned()), ("libraryName", library)],
        )
        .await
    }

    /// Terms containing one gene across every library (`GET /genemap`).
    pub async fn genemap(&self, gene: &str) -> Result<GeneMap> {
        #[derive(serde::Deserialize)]
        struct GeneMapEnvelope {
            #[serde(default)]
            gene: BTreeMap<String, Vec<String>>,
        }
        let gene = validate_gene_symbol(gene)?;
        let envelope: GeneMapEnvelope = self
            .get_json(
                "genemap",
                &[("gene", gene.clone()), ("json", "true".to_owned())],
            )
            .await?;
        Ok(GeneMap::from_envelope(&gene, envelope.gene))
    }

    /// Access any documented Enrichr text endpoint in its native form.
    pub async fn raw(&self, path: &str, params: &[(&str, String)]) -> Result<String> {
        let path = path.trim_start_matches('/');
        self.get_text(path, params).await
    }

    // -----------------------------------------------------------------------
    // Speedrichr (background-corrected enrichment)
    // -----------------------------------------------------------------------

    /// Submit a gene list to Speedrichr (`POST /api/addList`).
    ///
    /// The returned `user_list_id` is only valid against Speedrichr's
    /// background endpoints, not against classic Enrichr.
    pub async fn speedrichr_add_list<S, I>(
        &self,
        genes: I,
        description: impl Into<Option<&str>>,
    ) -> Result<AddedList>
    where
        S: AsRef<str>,
        I: IntoIterator<Item = S>,
    {
        let genes = normalize_genes(&genes.into_iter().collect::<Vec<_>>(), "genes")?;
        let description = description
            .into()
            .map(str::to_owned)
            .unwrap_or_else(|| crate::request::DEFAULT_LIST_DESCRIPTION.to_owned());
        let form = reqwest::multipart::Form::new()
            .text("list", joined_lines(&genes))
            .text("description", description);
        self.speedrichr_post_multipart_json("addList", form).await
    }

    /// Submit the background gene universe (`POST /api/addbackground`).
    ///
    /// The background should cover every gene the experiment could have
    /// observed, not just the significant hits.
    pub async fn speedrichr_add_background<S, I>(&self, genes: I) -> Result<SpeedrichrBackground>
    where
        S: AsRef<str>,
        I: IntoIterator<Item = S>,
    {
        let genes = normalize_genes(&genes.into_iter().collect::<Vec<_>>(), "background genes")?;
        let form = reqwest::multipart::Form::new().text("background", joined_lines(&genes));
        self.speedrichr_post_multipart_json("addbackground", form)
            .await
    }

    /// Run background-corrected enrichment (`POST /api/backgroundenrich`,
    /// form-encoded body).
    pub async fn speedrichr_background_enrich(
        &self,
        user_list_id: u64,
        background_id: &str,
        background_type: &str,
    ) -> Result<EnrichmentResult> {
        validate_user_list_id(user_list_id)?;
        let background_id = validate_background_id(background_id)?;
        let library = validate_library_name(background_type)?;
        let map: BTreeMap<String, Vec<GeneSetTerm>> = self
            .speedrichr_post_form_json(
                "backgroundenrich",
                &[
                    ("userListId", user_list_id.to_string()),
                    ("backgroundid", background_id),
                    ("backgroundType", library),
                ],
            )
            .await?;
        EnrichmentResult::from_singleton_map(map)
    }

    // -----------------------------------------------------------------------
    // HTTP plumbing
    // -----------------------------------------------------------------------

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path)
    }

    fn speedrichr_url(&self, path: &str) -> String {
        format!("{}/{}", self.speedrichr_url, path)
    }

    async fn wait_for_rate_limit(&self) {
        if !self.rate_limited {
            return;
        }
        let delay = {
            let Ok(mut limit) = self.rate_limit.lock() else {
                return;
            };
            let now = Instant::now();
            let ready_at = limit
                .next_ready_at
                .map_or(now, |ready_at| ready_at.max(now));
            limit.next_ready_at = Some(ready_at + MIN_REQUEST_INTERVAL);
            ready_at.saturating_duration_since(now)
        };
        if !delay.is_zero() {
            sleep(delay).await;
        }
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<String> {
        self.wait_for_rate_limit().await;
        let response = request.send().await?;
        let status = response.status().as_u16();
        let body = response.text().await?;
        if !(200..300).contains(&status) {
            // Error bodies are HTML pages; keep a bounded excerpt.
            let excerpt = truncate_chars(&body, 300);
            return Err(EnrichrError::Status {
                status,
                body: excerpt,
            });
        }
        Ok(body)
    }

    async fn get_text(&self, path: &str, params: &[(&str, String)]) -> Result<String> {
        let mut request = self.http.get(self.url(path));
        if !params.is_empty() {
            request = request.query(&params);
        }
        self.send(request).await
    }

    async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<T> {
        let text = self.get_text(path, params).await?;
        let sanitized = sanitize_nonfinite_json(&text);
        let value: serde_json::Value = serde_json::from_str(&sanitized)?;
        if let serde_json::Value::Object(map) = &value {
            if let Some(serde_json::Value::String(message)) = map.get("error") {
                return Err(EnrichrError::Api(format!("{path} failed: {message}")));
            }
        }
        Ok(serde_json::from_value(value)?)
    }

    async fn post_multipart_json<T: DeserializeOwned>(
        &self,
        path: &str,
        form: reqwest::multipart::Form,
    ) -> Result<T> {
        let text = self
            .send(self.http.post(self.url(path)).multipart(form))
            .await?;
        parse_sanitized(&text, path)
    }

    async fn speedrichr_post_multipart_json<T: DeserializeOwned>(
        &self,
        path: &str,
        form: reqwest::multipart::Form,
    ) -> Result<T> {
        let text = self
            .send(self.http.post(self.speedrichr_url(path)).multipart(form))
            .await?;
        parse_sanitized(&text, path)
    }

    async fn speedrichr_post_form_json<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<T> {
        let text = self
            .send(self.http.post(self.speedrichr_url(path)).form(params))
            .await?;
        parse_sanitized(&text, path)
    }
}

fn parse_sanitized<T: DeserializeOwned>(text: &str, path: &str) -> Result<T> {
    let sanitized = sanitize_nonfinite_json(text);
    let value: serde_json::Value = serde_json::from_str(&sanitized)?;
    if let serde_json::Value::Object(map) = &value {
        if let Some(serde_json::Value::String(message)) = map.get("error") {
            return Err(EnrichrError::Api(format!("{path} failed: {message}")));
        }
    }
    Ok(serde_json::from_value(value)?)
}

fn truncate_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        text.to_owned()
    } else {
        let cut = text
            .char_indices()
            .nth(limit)
            .map_or(text.len(), |(index, _)| index);
        text[..cut].to_owned()
    }
}

fn normalize_base(endpoint: impl Into<String>) -> String {
    endpoint.into().trim().trim_end_matches('/').to_owned()
}

/// Replace bare `Infinity` / `-Infinity` literals with finite stand-ins so
/// serde_json can parse the payload.
///
/// Speedrichr emits a bare `Infinity` for the odds ratio and combined score
/// when a term's overlap with the background is complete. The literal is
/// not valid JSON, so it is rewritten to `±1e308` — near `f64::MAX`, and
/// distinguishable from any real score. String contents are skipped, so a
/// term literally named "Infinity pathway" survives untouched.
pub fn sanitize_nonfinite_json(text: &str) -> String {
    const TOKEN: &str = "Infinity";
    const REPLACEMENT: &str = "1e308";
    if !text.contains(TOKEN) {
        return text.to_owned();
    }
    let bytes = text.as_bytes();
    // Byte-level output: pushing raw bytes (instead of `byte as char`)
    // keeps multi-byte UTF-8 term names intact.
    let mut out: Vec<u8> = Vec::with_capacity(text.len());
    let mut index = 0;
    let mut in_string = false;
    let mut escaped = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            out.push(byte);
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        match byte {
            b'"' => {
                in_string = true;
                out.push(b'"');
                index += 1;
            }
            b'-' if text[index..].starts_with("-Infinity") => {
                out.extend_from_slice(format!("-{REPLACEMENT}").as_bytes());
                index += TOKEN.len() + 1;
            }
            b'I' if text[index..].starts_with(TOKEN) => {
                out.extend_from_slice(REPLACEMENT.as_bytes());
                index += TOKEN.len();
            }
            _ => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(out).expect("byte-preserving rewrite of valid UTF-8 stays valid")
}

/// Builder for [`EnrichrClient`].
#[derive(Debug)]
pub struct EnrichrClientBuilder {
    http: Option<reqwest::Client>,
    base_url: String,
    speedrichr_url: String,
    rate_limited: bool,
}

impl EnrichrClientBuilder {
    /// Supply a configured reqwest client (timeout, proxy, TLS, etc.).
    pub fn http(mut self, http: reqwest::Client) -> Self {
        self.http = Some(http);
        self
    }

    /// Set the Enrichr base URL.
    pub fn endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.base_url = normalize_base(endpoint);
        self
    }

    /// Set the Speedrichr base URL.
    pub fn speedrichr_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.speedrichr_url = normalize_base(endpoint);
        self
    }

    /// Select both endpoints from an organism host.
    pub fn host(mut self, host: EnrichrHost) -> Self {
        self.base_url = host.endpoint().to_owned();
        self.speedrichr_url = host.speedrichr_endpoint().to_owned();
        self
    }

    /// Disable the built-in one-second pacing. Only do this for tests.
    pub fn disable_rate_limit(mut self) -> Self {
        self.rate_limited = false;
        self
    }

    /// Build the client.
    pub fn build(self) -> Result<EnrichrClient> {
        if self.base_url.is_empty() {
            return Err(EnrichrError::InvalidRequest(
                "endpoint cannot be empty".into(),
            ));
        }
        if self.speedrichr_url.is_empty() {
            return Err(EnrichrError::InvalidRequest(
                "speedrichr endpoint cannot be empty".into(),
            ));
        }
        let http = match self.http {
            Some(http) => http,
            None => reqwest::Client::builder().user_agent(USER_AGENT).build()?,
        };
        Ok(EnrichrClient {
            http,
            base_url: self.base_url,
            speedrichr_url: self.speedrichr_url,
            rate_limit: Mutex::default(),
            rate_limited: self.rate_limited,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_bare_infinity_outside_strings() {
        let raw = r#"{"KEGG_2021_Human" : [[1,"MicroRNAs in cancer",0.083, Infinity, Infinity, ["MYC"],0.99, 0, 0 ]]}"#;
        let sanitized = sanitize_nonfinite_json(raw);
        assert!(!sanitized.contains("Infinity"));
        let parsed: serde_json::Value = serde_json::from_str(&sanitized).unwrap();
        assert_eq!(
            parsed["KEGG_2021_Human"][0][3].as_f64(),
            Some(1e308),
            "Infinity should clamp to 1e308"
        );
    }

    #[test]
    fn leaves_infinity_inside_strings_untouched() {
        let raw = r#"[1,"Infinity pathway",0.5,2.0,3.0,["TP53"],0.5,0,0]"#;
        let sanitized = sanitize_nonfinite_json(raw);
        let term: GeneSetTerm = serde_json::from_str(&sanitized).unwrap();
        assert_eq!(term.term, "Infinity pathway");
    }

    #[test]
    fn handles_negative_infinity_and_plain_text() {
        assert_eq!(sanitize_nonfinite_json("plain"), "plain");
        let sanitized = sanitize_nonfinite_json("x -Infinity y");
        assert_eq!(sanitized, "x -1e308 y");
    }
}
