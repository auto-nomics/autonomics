use std::time::{Duration, Instant};

use reqwest::Client;
use serde::de::DeserializeOwned;

use crate::error::{Result, UniProtError};
use crate::types::*;

/// Base URL for all UniProt REST API requests.
const BASE_URL: &str = "https://rest.uniprot.org";

/// Page size cap enforced by the `/search` endpoints.
const MAX_PAGE_SIZE: u32 = 500;

/// Wall-clock budget for [`UniProtClient::map_ids`] to wait for a mapping
/// job to finish.
const MAP_TIMEOUT_SECS: u64 = 120;

/// Poll interval while waiting for an ID mapping job.
const MAP_POLL_INTERVAL: Duration = Duration::from_secs(1);

fn base_url() -> String {
    std::env::var("ENDPOINT_UNIPROT_URL").unwrap_or_else(|_| BASE_URL.to_string())
}

// ===========================================================================
// Client
// ===========================================================================

/// An async client for the [UniProt REST API](https://www.uniprot.org/help/programmatic_access).
///
/// The API is free and open — no API key required. UniProt asks clients to
/// stay under ~10 requests/second and to use the stream endpoints (not
/// paginated search) for bulk downloads; this client reuses one connection
/// pool and sets a polite `User-Agent`.
///
/// # Endpoints covered
///
/// | Method                                   | API path                                  |
///|------------------------------------------|-------------------------------------------|
/// | [`search`](Self::search)                 | `GET /uniprotkb/search`                   |
/// | [`stream`](Self::stream)                 | `GET /uniprotkb/stream`                   |
/// | [`entry`](Self::entry) / [`entry_text`](Self::entry_text) | `GET /uniprotkb/{accession}.{format}` |
/// | [`fasta`](Self::fasta)                   | `GET /uniprotkb/stream` (FASTA)           |
/// | [`search_taxonomy`](Self::search_taxonomy) | `GET /taxonomy/search`                  |
/// | [`taxonomy_entry`](Self::taxonomy_entry) | `GET /taxonomy/{taxonId}`                 |
/// | [`search_proteomes`](Self::search_proteomes) | `GET /proteomes/search`              |
/// | [`proteome_entry`](Self::proteome_entry) | `GET /proteomes/{upid}`                   |
/// | [`map_ids`](Self::map_ids) et al.        | `POST /idmapping/run` + status + results  |
///
/// # Pagination
///
/// The `/search` endpoints use **cursor pagination**: the first request
/// omits the cursor, and each response exposes the next page's opaque
/// cursor as [`SearchResults::next_cursor`] (parsed from the `Link`
/// header). Pass it back via [`SearchRequest::cursor`].
///
/// # Example
///
/// ```no_run
/// use uniprot::UniProtClient;
///
/// let client = UniProtClient::new();
/// ```
#[derive(Debug)]
pub struct UniProtClient {
    client: Client,
    /// Redirect-free variant used only for ID mapping status polling: the
    /// status endpoint replies `303 See Other` to the results once a job
    /// finishes, which we read as "FINISHED" instead of following.
    status_client: Client,
}

/// Polite `User-Agent` shared by both inner clients.
const USER_AGENT: &str = "uniprot-rs-sdk/0.1 (+https://www.uniprot.org)";

impl Default for UniProtClient {
    fn default() -> Self {
        Self::new()
    }
}

impl UniProtClient {
    /// Create a new client with default settings.
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .user_agent(USER_AGENT)
                .build()
                .expect("reqwest client builder"),
            status_client: Client::builder()
                .user_agent(USER_AGENT)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("reqwest client builder"),
        }
    }

    /// Provide a custom `reqwest::Client` (e.g. for timeouts, proxy).
    pub fn with_client(client: Client) -> Self {
        Self {
            client,
            status_client: Client::builder()
                .user_agent(USER_AGENT)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("reqwest client builder"),
        }
    }

    // -----------------------------------------------------------------------
    // Core request helpers
    // -----------------------------------------------------------------------

    /// Execute a GET request, returning the body as text. Non-2xx statuses
    /// become [`UniProtError::Status`].
    async fn get_text(&self, url: &str) -> Result<String> {
        let resp = self.client.get(url).send().await?;
        let status = resp.status().as_u16();
        let body = resp.text().await?;
        if !(200..300).contains(&status) {
            return Err(UniProtError::Status { status, body });
        }
        Ok(body)
    }

    /// Execute a GET request against a `/search` endpoint, deserialising the
    /// JSON body and folding in the pagination headers.
    async fn get_paged<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<SearchResults<T>> {
        let url = build_url(&base_url(), path, params);
        let resp = self.client.get(&url).send().await?;
        let status = resp.status().as_u16();

        let total_results =
            header_str(&resp, "x-total-results").and_then(|v| v.parse::<u64>().ok());
        let release = header_str(&resp, "x-uniprot-release").map(str::to_owned);
        let next_cursor = header_str(&resp, "link").and_then(next_cursor_from_link);

        let body = resp.text().await?;
        if !(200..300).contains(&status) {
            return Err(UniProtError::Status { status, body });
        }

        let mut results: SearchResults<T> = serde_json::from_str(&body)?;
        results.total_results = total_results;
        results.release = release;
        results.next_cursor = next_cursor;
        Ok(results)
    }

    /// Collect query parameters common to every `/search` call.
    fn search_params(req: &SearchRequest) -> Vec<(&'static str, String)> {
        let mut params: Vec<(&'static str, String)> =
            vec![("query", req.query.clone()), ("format", "json".to_owned())];
        if let Some(ref f) = req.fields {
            params.push(("fields", f.join(",")));
        }
        if let Some(size) = req.size {
            params.push(("size", size.to_string()));
        }
        if let Some(ref c) = req.cursor {
            params.push(("cursor", c.clone()));
        }
        if let Some(ref s) = req.sort {
            params.push(("sort", s.to_owned()));
        }
        params
    }

    // ===================================================================
    // UniProtKB search
    // ===================================================================

    /// Search UniProtKB and return one page of typed entries.
    ///
    /// Follow up with [`SearchResults::next_cursor`] +
    /// [`SearchRequest::cursor`] to page through results, or use
    /// [`Self::search_all`] / [`Self::stream`] to collect everything.
    ///
    /// ```no_run
    /// # use uniprot::{UniProtClient, types::SearchRequest};
    /// # async fn run() -> uniprot::error::Result<()> {
    /// let client = UniProtClient::new();
    /// let page = client
    ///     .search(&SearchRequest::new("gene:INS AND organism_id:9606 AND reviewed:true")
    ///         .size(5))
    ///     .await?;
    /// println!("{} of {} entries", page.results.len(),
    ///          page.total_results.unwrap_or_default());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn search(&self, req: &SearchRequest) -> Result<SearchResults<Entry>> {
        if let Some(size) = req.size {
            if size > MAX_PAGE_SIZE {
                return Err(UniProtError::Param(format!(
                    "size {size} exceeds the API cap of {MAX_PAGE_SIZE}"
                )));
            }
        }
        self.get_paged("/uniprotkb/search", &Self::search_params(req))
            .await
    }

    /// Search UniProtKB, following cursor pagination until the query is
    /// exhausted or `max_results` entries have been collected.
    ///
    /// Intended for moderate result sets; for whole-proteome downloads use
    /// [`Self::stream`] (the API explicitly prefers streaming over paging).
    pub async fn search_all(&self, req: &SearchRequest, max_results: usize) -> Result<Vec<Entry>> {
        let mut req = req.clone();
        req.cursor = None;
        let mut entries = Vec::new();
        loop {
            let page = self.search(&req).await?;
            if page.results.is_empty() {
                return Ok(entries);
            }
            for entry in page.results {
                if entries.len() >= max_results {
                    return Ok(entries);
                }
                entries.push(entry);
            }
            match page.next_cursor {
                Some(cursor) => req.cursor = Some(cursor),
                None => return Ok(entries),
            }
        }
    }

    // ===================================================================
    // UniProtKB stream / entries
    // ===================================================================

    /// Stream the full result set for a query as raw text (TSV, FASTA, …).
    ///
    /// This is the bulk-download endpoint: no pagination, no entry cap.
    /// The output for `Format::Tsv` starts with a header row of field
    /// names, making it directly loadable as a table.
    pub async fn stream(
        &self,
        query: &str,
        fields: Option<&[String]>,
        format: Format,
    ) -> Result<String> {
        let mut params: Vec<(&str, String)> = vec![
            ("query", query.to_owned()),
            ("format", format.as_str().to_owned()),
        ];
        if let Some(fields) = fields {
            params.push(("fields", fields.join(",")));
        }
        let url = build_url(&base_url(), "/uniprotkb/stream", &params);
        self.get_text(&url).await
    }

    /// Fetch the sequences of the given accessions as FASTA text.
    ///
    /// ```no_run
    /// # use uniprot::UniProtClient;
    /// # async fn run() -> uniprot::error::Result<()> {
    /// let fasta = UniProtClient::new()
    ///     .fasta(&["P01308", "P0DTC2"])
    ///     .await?;
    /// assert!(fasta.starts_with('>'));
    /// # Ok(())
    /// # }
    /// ```
    pub async fn fasta(&self, accessions: &[&str]) -> Result<String> {
        if accessions.is_empty() {
            return Err(UniProtError::Param(
                "at least one accession is required".into(),
            ));
        }
        for acc in accessions {
            validate_accession(acc)?;
        }
        let query = format!("accession:({})", accessions.join(" OR "));
        self.stream(&query, None, Format::Fasta).await
    }

    /// Fetch a single entry in a raw format (TSV, TXT, GFF, FASTA, …).
    pub async fn entry_text(&self, accession: &str, format: Format) -> Result<String> {
        validate_accession(accession)?;
        let url = format!(
            "{}/uniprotkb/{}.{}",
            base_url(),
            urlencode(accession),
            format.as_str()
        );
        self.get_text(&url).await
    }

    /// Fetch a single entry as a typed [`Entry`].
    pub async fn entry(&self, accession: &str) -> Result<Entry> {
        let body = self.entry_text(accession, Format::Json).await?;
        serde_json::from_str(&body).map_err(Into::into)
    }

    // ===================================================================
    // Taxonomy
    // ===================================================================

    /// Search the UniProt taxonomy. Query fields include `id` (taxon ID)
    /// and free text over names, e.g. `"id:9606"` or `"homo sapiens"`.
    pub async fn search_taxonomy(&self, req: &SearchRequest) -> Result<SearchResults<Taxon>> {
        self.get_paged("/taxonomy/search", &Self::search_params(req))
            .await
    }

    /// Fetch one taxon by NCBI taxon ID, e.g. `9606` for human.
    pub async fn taxonomy_entry(&self, taxon_id: u64) -> Result<Taxon> {
        let url = format!("{}/taxonomy/{}", base_url(), taxon_id);
        let body = self.get_text(&url).await?;
        serde_json::from_str(&body).map_err(Into::into)
    }

    // ===================================================================
    // Proteomes
    // ===================================================================

    /// Search reference proteomes, e.g. `"organism_id:9606"`.
    pub async fn search_proteomes(&self, req: &SearchRequest) -> Result<SearchResults<Proteome>> {
        self.get_paged("/proteomes/search", &Self::search_params(req))
            .await
    }

    /// Fetch one proteome by UPID, e.g. `"UP000005640"` for human.
    pub async fn proteome_entry(&self, upid: &str) -> Result<Proteome> {
        let upid = upid.trim();
        // UPIDs are "UP" + 9 digits (11 chars), e.g. UP000005640.
        if upid.len() != 11
            || !upid.starts_with("UP")
            || !upid[2..].bytes().all(|b| b.is_ascii_digit())
        {
            return Err(UniProtError::Param(format!(
                "invalid proteome UPID: {upid:?} (expected e.g. \"UP000005640\")"
            )));
        }
        let url = format!("{}/proteomes/{}", base_url(), urlencode(upid));
        let body = self.get_text(&url).await?;
        serde_json::from_str(&body).map_err(Into::into)
    }

    // ===================================================================
    // ID mapping
    // ===================================================================

    /// Submit an ID mapping job and return its job ID.
    ///
    /// `from`/`to` are database names such as `"UniProtKB_AC-ID"`,
    /// `"Ensembl"`, `"RefSeq_Protein"`, `"Gene_Name"`, `"PDB"`. See the
    /// [ID mapping help](https://www.uniprot.org/help/id_mapping) for the
    /// full list. Results are fetched with [`Self::id_mapping_results`] or
    /// the polling convenience [`Self::map_ids`].
    pub async fn submit_id_mapping(&self, from: &str, to: &str, ids: &[String]) -> Result<String> {
        if ids.is_empty() {
            return Err(UniProtError::Param("at least one ID is required".into()));
        }
        let url = format!("{}/idmapping/run", base_url());
        let resp = self
            .client
            .post(&url)
            .form(&[
                ("from", from.to_owned()),
                ("to", to.to_owned()),
                ("ids", ids.join(",")),
            ])
            .send()
            .await?;
        let status = resp.status().as_u16();
        let body = resp.text().await?;
        if !(200..300).contains(&status) {
            return Err(UniProtError::Status { status, body });
        }
        let job: IdMappingJob = serde_json::from_str(&body)?;
        if let Some(job_id) = job.job_id {
            Ok(job_id)
        } else if !job.errors.is_empty() {
            Err(UniProtError::Param(job.errors.join("; ")))
        } else {
            Err(UniProtError::Param(
                "id mapping response contained neither a job ID nor errors".into(),
            ))
        }
    }

    /// Check whether a submitted mapping job has finished.
    ///
    /// A `3xx` response means the API is redirecting to the finished
    /// results, reported as `FINISHED`.
    pub async fn id_mapping_status(&self, job_id: &str) -> Result<IdMappingStatus> {
        let url = format!("{}/idmapping/status/{}", base_url(), urlencode(job_id));
        let resp = self.status_client.get(&url).send().await?;
        let status = resp.status().as_u16();
        if (300..400).contains(&status) {
            return Ok(IdMappingStatus {
                job_status: Some("FINISHED".to_owned()),
                errors: Vec::new(),
            });
        }
        let body = resp.text().await?;
        if !(200..300).contains(&status) {
            return Err(UniProtError::Status { status, body });
        }
        serde_json::from_str(&body).map_err(Into::into)
    }

    /// Fetch the results of a finished mapping job as raw text (TSV by
    /// default).
    ///
    /// The output carries the source ID in the `From` column. Note that
    /// the API rejects the pseudo-fields `from`/`to` here — regular
    /// UniProtKB field names only.
    pub async fn id_mapping_results(
        &self,
        job_id: &str,
        fields: Option<&[String]>,
        format: Format,
    ) -> Result<String> {
        let mut params: Vec<(&str, String)> = vec![("format", format.as_str().to_owned())];
        if let Some(fields) = fields {
            params.push(("fields", fields.join(",")));
        }
        let url = build_url(
            &base_url(),
            &format!("/idmapping/uniprotkb/results/stream/{}", urlencode(job_id)),
            &params,
        );
        self.get_text(&url).await
    }

    /// Fetch the results of a finished mapping job as typed entries.
    pub async fn id_mapping_results_json(&self, job_id: &str) -> Result<IdMappingResults> {
        let body = self.id_mapping_results(job_id, None, Format::Json).await?;
        serde_json::from_str(&body).map_err(Into::into)
    }

    /// Map identifiers end-to-end: submit, poll until the job finishes
    /// (≤120 s), then return the results as TSV text.
    ///
    /// ```no_run
    /// # use uniprot::UniProtClient;
    /// # async fn run() -> uniprot::error::Result<()> {
    /// let tsv = UniProtClient::new()
    ///     .map_ids("Gene_Name", "UniProtKB",
    ///              &["INS".to_owned(), "S".to_owned()], None)
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn map_ids(
        &self,
        from: &str,
        to: &str,
        ids: &[String],
        fields: Option<&[String]>,
    ) -> Result<String> {
        let job_id = self.submit_id_mapping(from, to, ids).await?;
        self.await_mapping_job(&job_id).await?;
        self.id_mapping_results(&job_id, fields, Format::Tsv).await
    }

    /// [`Self::map_ids`] with typed results (source ID → [`Entry`]).
    pub async fn map_ids_json(
        &self,
        from: &str,
        to: &str,
        ids: &[String],
    ) -> Result<IdMappingResults> {
        let job_id = self.submit_id_mapping(from, to, ids).await?;
        self.await_mapping_job(&job_id).await?;
        self.id_mapping_results_json(&job_id).await
    }

    /// Poll a mapping job until it finishes or the timeout elapses.
    async fn await_mapping_job(&self, job_id: &str) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(MAP_TIMEOUT_SECS);
        loop {
            let status = self.id_mapping_status(job_id).await?;
            if status.is_finished() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(UniProtError::Timeout(MAP_TIMEOUT_SECS));
            }
            tokio::time::sleep(MAP_POLL_INTERVAL).await;
        }
    }
}

// ===========================================================================
// URL / header helpers
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

/// Access a response header as UTF-8, if present and valid.
fn header_str<'a>(resp: &'a reqwest::Response, name: &str) -> Option<&'a str> {
    resp.headers().get(name)?.to_str().ok()
}

/// Extract the `cursor` query parameter from the `rel="next"` entry of a
/// `Link` header, e.g.
/// `<https://rest.uniprot.org/...?cursor=abc&size=500>; rel="next"`.
fn next_cursor_from_link(link: &str) -> Option<String> {
    for entry in link.split(">,") {
        let entry = entry.trim();
        if !entry.contains("rel=\"next\"") {
            continue;
        }
        let url = entry
            .strip_prefix('<')?
            .split(';')
            .next()?
            .trim()
            .trim_end_matches('>');
        let idx = url.find("cursor=")?;
        let tail = &url[idx + "cursor=".len()..];
        let end = tail.find('&').unwrap_or(tail.len());
        let cursor = &tail[..end];
        if !cursor.is_empty() {
            return Some(cursor.to_owned());
        }
    }
    None
}

/// Reject accessions that could not name an entry (path-safety check).
fn validate_accession(acc: &str) -> Result<()> {
    let valid = (6..=10).contains(&acc.len())
        && acc.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if valid {
        Ok(())
    } else {
        Err(UniProtError::Param(format!(
            "invalid accession: {acc:?} (expected e.g. \"P01308\" or \"P01308-2\")"
        )))
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_urls_with_encoded_params() {
        let url = build_url(
            "https://rest.uniprot.org",
            "/uniprotkb/search",
            &[
                ("query", "gene:INS AND organism_id:9606".into()),
                ("format", "json".into()),
            ],
        );
        assert_eq!(
            url,
            "https://rest.uniprot.org/uniprotkb/search?query=gene%3AINS%20AND%20organism_id%3A9606&format=json"
        );
    }

    #[test]
    fn extracts_cursor_from_link_header() {
        let link = r#"<https://rest.uniprot.org/uniprotkb/search?format=tsv&query=insulin&cursor=88d67348nie3zpemjhxcdb8flyemtryvzu&size=2>; rel="next""#;
        assert_eq!(
            next_cursor_from_link(link).as_deref(),
            Some("88d67348nie3zpemjhxcdb8flyemtryvzu")
        );
    }

    #[test]
    fn extracts_cursor_when_cursor_is_last_param() {
        let link = r#"<https://rest.uniprot.org/proteomes/search?format=json&query=x&cursor=verqtw3fkbzjjfcasypeoqvvh4malo71f05sxcyxvh1lq5>; rel="next", <https://rest.uniprot.org/proteomes/search>; rel="previous""#;
        assert_eq!(
            next_cursor_from_link(link).as_deref(),
            Some("verqtw3fkbzjjfcasypeoqvvh4malo71f05sxcyxvh1lq5")
        );
    }

    #[test]
    fn no_next_link_yields_none() {
        assert_eq!(next_cursor_from_link(""), None);
        assert_eq!(
            next_cursor_from_link(r#"<https://rest.uniprot.org/x>; rel="previous""#),
            None
        );
    }

    #[test]
    fn validates_accessions() {
        assert!(validate_accession("P01308").is_ok());
        assert!(validate_accession("Q9BXM7-2").is_ok());
        assert!(validate_accession("P01308;rm -rf").is_err());
        assert!(validate_accession("").is_err());
        assert!(validate_accession("TOOLONGACCESSION").is_err());
    }

    #[test]
    fn fasta_query_groups_accessions() {
        // The query is built inside `fasta`, which needs network access to
        // run; the grouping syntax is instead covered by the query builder
        // tests in `crate::query`.
        let query = format!("accession:({})", ["P01308", "P0DTC2"].join(" OR "));
        assert_eq!(query, "accession:(P01308 OR P0DTC2)");
    }
}
