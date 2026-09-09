use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::time::sleep;

use crate::error::{Result, StringError};
use crate::request::{
    self, AnnotationQuery, EnrichmentQuery, HomologyQuery, ImageFormat, InteractionPartnerQuery,
    NetworkImageQuery, NetworkQuery, OutputFormat, StringIdQuery,
};
use crate::types::*;

/// Default development endpoint. Use [`StringDbClient::with_endpoint`] with
/// the version endpoint's stable address for reproducible production runs.
pub const DEFAULT_STRING_ENDPOINT: &str = "https://string-db.org";
const USER_AGENT: &str = "string-sdk-rs/0.1 (+https://string-db.org)";
const MIN_REQUEST_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Default)]
struct RateLimit {
    next_ready_at: Option<Instant>,
}

/// Async client for the STRING REST API.
///
/// Conventional STRING endpoints require no API key. Only Values/Ranks
/// enrichment jobs require the free, anonymous key returned by
/// [`get_api_key`](Self::get_api_key).
#[derive(Debug)]
pub struct StringDbClient {
    http: reqwest::Client,
    base_url: String,
    caller_identity: String,
    rate_limit: Mutex<RateLimit>,
    rate_limited: bool,
}

impl Default for StringDbClient {
    fn default() -> Self {
        Self::new()
    }
}

impl StringDbClient {
    /// Create a client for the current STRING release with a one-second
    /// minimum interval between requests.
    pub fn new() -> Self {
        Self::builder()
            .build()
            .expect("valid default STRING client")
    }

    /// Configure endpoint, caller identity, HTTP client, and rate limiting.
    pub fn builder() -> StringDbClientBuilder {
        StringDbClientBuilder {
            http: None,
            base_url: std::env::var("ENDPOINT_STRING_URL")
                .unwrap_or_else(|_| DEFAULT_STRING_ENDPOINT.to_owned()),
            caller_identity: "string-sdk-rs".to_owned(),
            rate_limited: true,
        }
    }

    /// Use a stable version endpoint, for example
    /// `https://version-12-0.string-db.org`.
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.base_url = normalize_endpoint(endpoint);
        self
    }

    /// Identify the application to STRING on every conventional request.
    pub fn with_caller_identity(mut self, caller_identity: impl Into<String>) -> Self {
        self.caller_identity = caller_identity.into();
        self
    }

    /// Borrow the configured endpoint.
    pub fn endpoint(&self) -> &str {
        &self.base_url
    }

    /// Fetch the current STRING version and stable endpoint.
    pub async fn version(&self) -> Result<Vec<Version>> {
        self.get_json_typed(OutputFormat::Json, "version", Vec::new())
            .await
    }

    /// Fetch the single current-version record.
    pub async fn current_version(&self) -> Result<Version> {
        first(self.version().await?, "version")
    }

    // -----------------------------------------------------------------------
    // Core retrieval endpoints
    // -----------------------------------------------------------------------

    /// Resolve names, synonyms, or UniProt accessions to STRING IDs.
    pub async fn get_string_ids(&self, query: &StringIdQuery) -> Result<Vec<StringId>> {
        let identifiers = request::normalize_identifiers(&query.identifiers, "identifiers")?;
        request::validate_species(&query.species)?;
        let mut params = vec![
            (
                "identifiers".to_owned(),
                request::joined(&identifiers, '\r'),
            ),
            (
                "echo_query".to_owned(),
                u8::from(query.echo_query).to_string(),
            ),
        ];
        push_species(&mut params, &query.species);
        self.get_json_typed(OutputFormat::Json, "get_string_ids", params)
            .await
    }

    /// Get interactions between inputs and their optional neighborhood.
    pub async fn network(&self, query: &NetworkQuery) -> Result<Vec<Interaction>> {
        let params = self.network_params(query)?;
        self.get_json_typed(OutputFormat::Json, "network", params)
            .await
    }

    /// Get interactions from each input to all other STRING proteins.
    pub async fn interaction_partners(
        &self,
        query: &InteractionPartnerQuery,
    ) -> Result<Vec<Interaction>> {
        let identifiers = request::normalize_identifiers(&query.identifiers, "identifiers")?;
        request::validate_species(&query.species)?;
        if let Some(score) = query.required_score {
            request::validate_required_score(score)?;
        }
        let mut params = vec![(
            "identifiers".to_owned(),
            request::joined(&identifiers, '\r'),
        )];
        if let Some(limit) = query.limit {
            params.push(("limit".to_owned(), limit.to_string()));
        }
        if let Some(score) = query.required_score {
            params.push(("required_score".to_owned(), score.to_string()));
        }
        params.push((
            "network_type".to_owned(),
            query.network_type.as_str().into(),
        ));
        push_species(&mut params, &query.species);
        self.get_json_typed(OutputFormat::Json, "interaction_partners", params)
            .await
    }

    /// Get pairwise similarity scores for supplied proteins.
    pub async fn homology(&self, query: &HomologyQuery) -> Result<Vec<Homology>> {
        let identifiers = request::normalize_identifiers(&query.identifiers, "identifiers")?;
        let mut params = vec![(
            "identifiers".to_owned(),
            request::joined(&identifiers, '\r'),
        )];
        if let Some(species_b) = query.species_b.as_deref() {
            if species_b.trim().is_empty() {
                return Err(StringError::InvalidRequest(
                    "species_b cannot be empty when supplied".into(),
                ));
            }
            params.push(("species_b".to_owned(), species_b.trim().to_owned()));
        }
        self.get_json_typed(OutputFormat::Json, "homology", params)
            .await
    }

    /// Get a stable link to a STRING network page.
    pub async fn network_link(&self, query: &NetworkQuery) -> Result<NetworkLink> {
        let mut params = self.network_params(query)?;
        params.extend([
            (
                "add_color_nodes".to_owned(),
                query
                    .add_nodes
                    .map_or_else(|| "0".to_owned(), |nodes| nodes.to_string()),
            ),
            ("add_white_nodes".to_owned(), "0".to_owned()),
            (
                "network_flavor".to_owned(),
                query.network_flavor.as_str().into(),
            ),
        ]);
        let links: Vec<NetworkLink> = self
            .get_json_typed(OutputFormat::Json, "get_link", params)
            .await?;
        first(links, "get_link")
    }

    // -----------------------------------------------------------------------
    // Analysis endpoints
    // -----------------------------------------------------------------------

    /// Run over-representation enrichment for a protein set.
    pub async fn enrichment(&self, query: &EnrichmentQuery) -> Result<Vec<Enrichment>> {
        let params = self.enrichment_params(query)?;
        self.get_json_typed(OutputFormat::Json, "enrichment", params)
            .await
    }

    /// Retrieve all assigned annotations, not just enriched terms.
    pub async fn functional_annotation(
        &self,
        query: &AnnotationQuery,
    ) -> Result<Vec<FunctionalAnnotation>> {
        let identifiers = request::normalize_identifiers(&query.identifiers, "identifiers")?;
        request::validate_species(&query.species)?;
        if query.allow_pubmed && query.only_pubmed {
            return Err(StringError::InvalidRequest(
                "allow_pubmed and only_pubmed cannot both be enabled".into(),
            ));
        }
        let mut params = vec![(
            "identifiers".to_owned(),
            request::joined(&identifiers, '\r'),
        )];
        if query.allow_pubmed {
            params.push(("allow_pubmed".to_owned(), "1".to_owned()));
        }
        if query.only_pubmed {
            params.push(("only_pubmed".to_owned(), "1".to_owned()));
        }
        push_species(&mut params, &query.species);
        self.get_json_typed(OutputFormat::Json, "functional_annotation", params)
            .await
    }

    /// Test whether a network has more interactions than expected.
    pub async fn ppi_enrichment(&self, query: &EnrichmentQuery) -> Result<Vec<PpiEnrichment>> {
        let params = self.enrichment_params(query)?;
        self.get_json_typed(OutputFormat::Json, "ppi_enrichment", params)
            .await
    }

    // -----------------------------------------------------------------------
    // Visualization and raw access
    // -----------------------------------------------------------------------

    /// Render a network image for later preview or report integration.
    pub async fn network_image(
        &self,
        query: &NetworkImageQuery,
        image_format: ImageFormat,
    ) -> Result<Image> {
        let network = &query.network;
        let mut params = self.network_params(network)?;
        params.push((
            "network_flavor".to_owned(),
            network.network_flavor.as_str().into(),
        ));
        if let Some(count) = query.add_color_nodes {
            params.push(("add_color_nodes".to_owned(), count.to_string()));
        }
        if let Some(count) = query.add_white_nodes {
            params.push(("add_white_nodes".to_owned(), count.to_string()));
        }
        for (key, enabled) in [
            ("hide_node_labels", query.hide_node_labels),
            ("hide_disconnected_nodes", query.hide_disconnected_nodes),
            ("show_query_node_labels", query.show_query_node_labels),
            (
                "block_structure_pics_in_bubbles",
                query.block_structure_pics_in_bubbles,
            ),
            ("flat_node_design", query.flat_node_design),
            ("center_node_labels", query.center_node_labels),
        ] {
            if enabled {
                params.push((key.to_owned(), "1".to_owned()));
            }
        }
        if let Some(size) = query.custom_label_font_size {
            if !(5..=50).contains(&size) {
                return Err(StringError::InvalidRequest(
                    "custom_label_font_size must be between 5 and 50".into(),
                ));
            }
            params.push(("custom_label_font_size".to_owned(), size.to_string()));
        }
        let media_type = image_format.media_type();
        let bytes = self
            .send_bytes(image_format.as_str(), "network", params, media_type)
            .await?;
        Ok(Image {
            media_type: media_type.to_owned(),
            bytes,
        })
    }

    /// Access any documented text endpoint in STRING's native representation.
    pub async fn raw(
        &self,
        format: OutputFormat,
        method: &str,
        params: Vec<(String, String)>,
    ) -> Result<String> {
        let mut final_params = params;
        if !final_params.iter().any(|(key, _)| key == "caller_identity") {
            final_params.push(("caller_identity".to_owned(), self.caller_identity.clone()));
        }
        self.send_form_text(format, method, final_params).await
    }

    // -----------------------------------------------------------------------
    // Values/Ranks asynchronous enrichment
    // -----------------------------------------------------------------------

    /// Request the free anonymous API key used only by Values/Ranks jobs.
    ///
    /// Calling this method allocates a key on the server. Persist the result;
    /// lost keys cannot be recovered.
    pub async fn get_api_key(&self) -> Result<ApiKey> {
        let keys: Vec<ApiKey> = self
            .get_json_typed(OutputFormat::Json, "get_api_key", Vec::new())
            .await?;
        first(keys, "get_api_key")
    }

    /// Submit a headerless two-column TSV payload (`protein<TAB>value`).
    ///
    /// The full experimental background is required by this method; do not
    /// pre-filter genes before submission.
    pub async fn valuesranks_submit(
        &self,
        api_key: &str,
        identifiers_tsv: &str,
        species: impl Into<String>,
        ge_fdr: f64,
        enrichment_direction: i8,
    ) -> Result<ValuesRanksJob> {
        let api_key = validate_api_key(api_key)?;
        let species = species.into();
        let species = species.trim();
        if species.is_empty() {
            return Err(StringError::InvalidRequest("species is required".into()));
        }
        validate_valuesranks_tsv(identifiers_tsv)?;
        if !(0.0..=1.0).contains(&ge_fdr) {
            return Err(StringError::InvalidRequest(
                "ge_fdr must be between 0 and 1".into(),
            ));
        }
        if !(-1..=1).contains(&enrichment_direction) || enrichment_direction == 0 {
            return Err(StringError::InvalidRequest(
                "enrichment_direction must be -1 (bottom), 0 (both), or 1 (top)".into(),
            ));
        }
        let params = vec![
            ("api_key".to_owned(), api_key),
            ("identifiers".to_owned(), identifiers_tsv.trim().to_owned()),
            ("species".to_owned(), species.to_owned()),
            ("ge_fdr".to_owned(), ge_fdr.to_string()),
            (
                "ge_enrichment_rank_direction".to_owned(),
                enrichment_direction.to_string(),
            ),
        ];
        let jobs: Vec<ValuesRanksJob> = self
            .post_json_typed("valuesranks_enrichment_submit", params)
            .await?;
        first(jobs, "valuesranks_enrichment_submit")
    }

    /// Retrieve one Values/Ranks job, or all jobs when `job_id` is `None`.
    pub async fn valuesranks_status(
        &self,
        api_key: &str,
        job_id: Option<&str>,
    ) -> Result<Vec<ValuesRanksJob>> {
        let api_key = validate_api_key(api_key)?;
        if let Some(job_id) = job_id {
            if job_id.trim().is_empty() {
                return Err(StringError::InvalidRequest(
                    "job_id cannot be empty when supplied".into(),
                ));
            }
        }
        let mut params = vec![("api_key".to_owned(), api_key)];
        if let Some(job_id) = job_id.map(str::trim) {
            params.push(("job_id".to_owned(), job_id.to_owned()));
        }
        self.get_json_typed(OutputFormat::Json, "valuesranks_enrichment_status", params)
            .await
    }

    /// Remove one Values/Ranks job, or all jobs with `Some("all")`.
    pub async fn valuesranks_remove(&self, api_key: &str, job_id: &str) -> Result<ValuesRanksJob> {
        let api_key = validate_api_key(api_key)?;
        let job_id = job_id.trim();
        if job_id.is_empty() {
            return Err(StringError::InvalidRequest("job_id is required".into()));
        }
        let params = vec![
            ("api_key".to_owned(), api_key),
            ("job_id".to_owned(), job_id.to_owned()),
        ];
        let jobs: Vec<ValuesRanksJob> = self
            .post_json_typed("valuesranks_enrichment_remove", params)
            .await?;
        first(jobs, "valuesranks_enrichment_remove")
    }

    // -----------------------------------------------------------------------
    // Parameter helpers
    // -----------------------------------------------------------------------

    fn network_params(&self, query: &NetworkQuery) -> Result<Vec<(String, String)>> {
        let has_ids = !query.identifiers.is_empty();
        let has_term = query.network_term_id.is_some();
        if has_ids == has_term {
            return Err(StringError::InvalidRequest(
                "network query requires exactly one of identifiers or network_term_id".into(),
            ));
        }
        if let Some(term) = query.network_term_id.as_deref() {
            if term.trim().is_empty() {
                return Err(StringError::InvalidRequest(
                    "network_term_id cannot be empty when supplied".into(),
                ));
            }
        } else {
            request::normalize_identifiers(&query.identifiers, "identifiers")?;
            if query.identifiers.len() > 10 && query.species.is_none() {
                return Err(StringError::InvalidRequest(
                    "species is required for networks with more than 10 proteins".into(),
                ));
            }
        }
        request::validate_species(&query.species)?;
        if let Some(score) = query.required_score {
            request::validate_required_score(score)?;
        }

        let mut params = Vec::new();
        if has_term {
            params.push((
                "network_term_id".to_owned(),
                query
                    .network_term_id
                    .as_deref()
                    .unwrap_or_default()
                    .to_owned(),
            ));
        } else {
            params.push((
                "identifiers".to_owned(),
                request::joined(&query.identifiers, '\r'),
            ));
        }
        if let Some(score) = query.required_score {
            params.push(("required_score".to_owned(), score.to_string()));
        }
        params.push((
            "network_type".to_owned(),
            query.network_type.as_str().into(),
        ));
        if let Some(nodes) = query.add_nodes {
            params.push(("add_nodes".to_owned(), nodes.to_string()));
        }
        if query.show_query_node_labels {
            params.push(("show_query_node_labels".to_owned(), "1".to_owned()));
        }
        push_species(&mut params, &query.species);
        Ok(params)
    }

    fn enrichment_params(&self, query: &EnrichmentQuery) -> Result<Vec<(String, String)>> {
        let identifiers = request::normalize_identifiers(&query.identifiers, "identifiers")?;
        let background = if query.background_string_identifiers.is_empty() {
            Vec::new()
        } else {
            request::normalize_identifiers(
                &query.background_string_identifiers,
                "background_string_identifiers",
            )?
        };
        request::validate_species(&query.species)?;
        let mut params = vec![(
            "identifiers".to_owned(),
            request::joined(&identifiers, '\r'),
        )];
        if !background.is_empty() {
            params.push((
                "background_string_identifiers".to_owned(),
                request::joined(&background, '\r'),
            ));
        }
        push_species(&mut params, &query.species);
        Ok(params)
    }

    // -----------------------------------------------------------------------
    // HTTP plumbing
    // -----------------------------------------------------------------------

    fn url(&self, format: OutputFormat, method: &str) -> String {
        format!(
            "{}/api/{}/{}",
            self.base_url,
            format.as_str(),
            percent_encode_path(method),
        )
    }

    fn binary_url(&self, format: &str, method: &str) -> String {
        format!(
            "{}/api/{}/{}",
            self.base_url,
            format,
            percent_encode_path(method),
        )
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

    async fn send_form_text(
        &self,
        format: OutputFormat,
        method: &str,
        params: Vec<(String, String)>,
    ) -> Result<String> {
        self.wait_for_rate_limit().await;
        let response = self
            .http
            .post(self.url(format, method))
            .form(&params)
            .send()
            .await?;
        let status = response.status().as_u16();
        let body = response.text().await?;
        if !(200..300).contains(&status) {
            return Err(StringError::Status { status, body });
        }
        Ok(body)
    }

    async fn send_bytes(
        &self,
        format: &str,
        method: &str,
        params: Vec<(String, String)>,
        expected_media_type: &str,
    ) -> Result<Vec<u8>> {
        self.wait_for_rate_limit().await;
        let response = self
            .http
            .post(self.binary_url(format, method))
            .form(&params)
            .send()
            .await?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            let body = response.text().await?;
            return Err(StringError::Status { status, body });
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let bytes = response.bytes().await?;
        if bytes.is_empty()
            || (!content_type.starts_with(expected_media_type)
                && !content_type.starts_with("application/octet-stream")
                && !content_type.is_empty())
        {
            return Err(StringError::InvalidRequest(format!(
                "expected a {expected_media_type} image response, got content type '{content_type}'"
            )));
        }
        Ok(bytes.to_vec())
    }

    async fn get_json_typed<T: DeserializeOwned>(
        &self,
        format: OutputFormat,
        method: &str,
        mut params: Vec<(String, String)>,
    ) -> Result<T> {
        if !params.iter().any(|(key, _)| key == "caller_identity") {
            params.push(("caller_identity".to_owned(), self.caller_identity.clone()));
        }
        let text = self.send_form_text(format, method, params).await?;
        let value: Value = serde_json::from_str(&text)?;
        if let Some(message) = api_error_message(&value) {
            return Err(StringError::Api(message));
        }
        Ok(serde_json::from_value(value)?)
    }

    async fn post_json_typed<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Vec<(String, String)>,
    ) -> Result<T> {
        self.get_json_typed(OutputFormat::Json, method, params)
            .await
    }
}

/// Builder for [`StringDbClient`].
#[derive(Debug)]
pub struct StringDbClientBuilder {
    http: Option<reqwest::Client>,
    base_url: String,
    caller_identity: String,
    rate_limited: bool,
}

impl StringDbClientBuilder {
    /// Supply a configured reqwest client (timeout, proxy, TLS, etc.).
    pub fn http(mut self, http: reqwest::Client) -> Self {
        self.http = Some(http);
        self
    }

    /// Set a current or version-pinned endpoint.
    pub fn endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.base_url = normalize_endpoint(endpoint);
        self
    }

    /// Set the stable `caller_identity` sent with conventional requests.
    pub fn caller_identity(mut self, caller_identity: impl Into<String>) -> Self {
        self.caller_identity = caller_identity.into();
        self
    }

    /// Disable the built-in one-second pacing. Only do this for tests.
    pub fn disable_rate_limit(mut self) -> Self {
        self.rate_limited = false;
        self
    }

    /// Build the client.
    pub fn build(self) -> Result<StringDbClient> {
        let base_url = normalize_endpoint(self.base_url);
        if base_url.is_empty() {
            return Err(StringError::InvalidRequest(
                "endpoint cannot be empty".into(),
            ));
        }
        let caller_identity = self.caller_identity.trim();
        if caller_identity.is_empty() {
            return Err(StringError::InvalidRequest(
                "caller_identity cannot be empty".into(),
            ));
        }
        let http = match self.http {
            Some(http) => http,
            None => reqwest::Client::builder().user_agent(USER_AGENT).build()?,
        };
        Ok(StringDbClient {
            http,
            base_url,
            caller_identity: caller_identity.to_owned(),
            rate_limit: Mutex::default(),
            rate_limited: self.rate_limited,
        })
    }
}

/// A decoded image payload suitable for MIME previews or report embedding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub media_type: String,
    pub bytes: Vec<u8>,
}

fn normalize_endpoint(endpoint: impl Into<String>) -> String {
    endpoint.into().trim().trim_end_matches('/').to_owned()
}

fn push_species(params: &mut Vec<(String, String)>, species: &Option<String>) {
    if let Some(species) = species.as_deref() {
        params.push(("species".to_owned(), species.trim().to_owned()));
    }
}

fn validate_api_key(api_key: &str) -> Result<String> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return Err(StringError::InvalidRequest("api_key is required".into()));
    }
    Ok(api_key.to_owned())
}

fn first<T>(mut values: Vec<T>, endpoint: &str) -> Result<T> {
    if values.is_empty() {
        return Err(StringError::Decode(serde::de::Error::custom(format!(
            "{endpoint} returned an empty JSON array"
        ))));
    }
    if values.len() != 1 {
        return Ok(values.swap_remove(0));
    }
    Ok(values.remove(0))
}

fn validate_valuesranks_tsv(input: &str) -> Result<()> {
    let mut count = 0;
    for (index, line) in input.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        count += 1;
        let Some((identifier, value)) = line.split_once('\t') else {
            return Err(StringError::InvalidRequest(format!(
                "Values/Ranks TSV line {} must have protein and value columns",
                index + 1
            )));
        };
        if identifier.trim().is_empty() || value.trim().is_empty() {
            return Err(StringError::InvalidRequest(format!(
                "Values/Ranks TSV line {} has an empty protein or value",
                index + 1
            )));
        }
        if !value
            .trim()
            .parse::<f64>()
            .is_ok_and(|parsed| parsed.is_finite())
        {
            return Err(StringError::InvalidRequest(format!(
                "Values/Ranks TSV line {} has a non-numeric value",
                index + 1
            )));
        }
    }
    if count == 0 {
        return Err(StringError::InvalidRequest(
            "Values/Ranks submission requires at least one protein/value row".into(),
        ));
    }
    Ok(())
}

fn api_error_message(value: &Value) -> Option<String> {
    let object = match value {
        Value::Object(object) => Some(object),
        Value::Array(items) => items.iter().find_map(|item| item.as_object()),
        _ => None,
    }?;
    if object.get("status").and_then(Value::as_str) == Some("error") {
        return Some(
            object
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown STRING API error")
                .to_owned(),
        );
    }
    None
}

/// Escape path segments without adding a URL-parsing dependency.
fn percent_encode_path(method: &str) -> String {
    let mut encoded = String::with_capacity(method.len());
    for byte in method.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}
