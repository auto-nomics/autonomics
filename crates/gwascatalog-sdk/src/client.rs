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
/// HTTPS mirror of the FTP site hosting full summary-statistics files.
pub const FTP_BASE: &str = "https://ftp.ebi.ac.uk/pub/databases/gwas/summary_statistics";

/// Resolve the GWAS Catalog Summary Statistics endpoint via the resource catalog.
pub fn ss_base() -> String {
    resource_catalog::endpoint_or("endpoint.gwascatalog_ss", SS_BASE)
}

/// Resolve the GWAS Catalog REST endpoint via the resource catalog.
pub fn rest_base() -> String {
    resource_catalog::endpoint_or("endpoint.gwascatalog_rest", REST_BASE)
}

/// Resolve the GWAS Catalog Solr Search endpoint via the resource catalog.
pub fn search_base() -> String {
    resource_catalog::endpoint_or("endpoint.gwascatalog_search", SEARCH_BASE)
}

/// Resolve the GWAS Catalog FTP mirror endpoint via the resource catalog.
pub fn ftp_base() -> String {
    resource_catalog::endpoint_or("endpoint.gwascatalog_ftp", FTP_BASE)
}

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
    ftp_base: String,
}

/// Backward-compat alias — the original SDK exposed `GwasCatalogApi`.
pub type GwasCatalogApi = GwasCatalogClient;

impl GwasCatalogClient {
    /// Create a client pointing to the production APIs.
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            ss_base: ss_base(),
            rest_base: rest_base(),
            search_base: search_base(),
            ftp_base: ftp_base(),
        }
    }

    /// Create a client with a custom Summary Statistics base URL (back-compat
    /// with the original `GwasCatalogApi::with_base_url`).
    pub fn with_base_url(ss_base: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            ss_base: ss_base.into(),
            rest_base: rest_base(),
            search_base: search_base(),
            ftp_base: ftp_base(),
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
            ftp_base: ftp_base(),
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

    // ── Full summary-statistics file download (HTTPS FTP mirror) ─────────

    /// Return a reference to the inner [`reqwest::Client`] so callers can
    /// issue streaming requests.
    pub fn http_client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Derive the FTP block directory for a GWAS Catalog accession.
    ///
    /// The FTP site groups accessions into 1000-entry blocks named
    /// `GCST{start}-GCST{end}`. For example `GCST90000061` lives under
    /// `GCST90000001-GCST90001000/`. Returns the full HTTPS URL of the
    /// block directory (no trailing slash).
    pub fn ftp_block_url(&self, accession: &str) -> Result<String> {
        let numeric = accession
            .strip_prefix("GCST")
            .ok_or_else(|| GwasCatalogError::InvalidAccession(accession.to_string()))?;
        if numeric.is_empty() || !numeric.chars().all(|c| c.is_ascii_digit()) {
            return Err(GwasCatalogError::InvalidAccession(accession.to_string()));
        }
        let n: u64 = numeric
            .parse()
            .map_err(|_| GwasCatalogError::InvalidAccession(accession.to_string()))?;
        let width = numeric.len();
        let block_start = (n / 1000) * 1000 + 1;
        let block_end = block_start + 999;
        Ok(format!(
            "{}/GCST{:0>width$}-GCST{:0>width$}",
            self.ftp_base,
            block_start,
            block_end,
            width = width,
        ))
    }

    /// Fetch an HTTPS FTP-mirror directory listing and return the `href`
    /// values of all `<a>` tags that look like real entries (not `../`).
    ///
    /// EBI's Apache listing serves HTML with `<a href="GCST.../">` links.
    pub async fn list_ftp_directory(&self, url: &str) -> Result<Vec<String>> {
        let resp = self.client.get(url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(GwasCatalogError::FtpListing {
                url: url.to_string(),
                reason: format!("HTTP {status}"),
            });
        }
        let html = resp.text().await?;
        Ok(parse_html_links(&html))
    }

    /// Stream-download a file to [`OpendalFileStorage`], calling `on_progress`
    /// with `(bytes_downloaded, total_bytes)` on every chunk.
    ///
    /// Peak memory ≈ chunk size (≈64 KiB), not the full file.
    pub async fn download_stream_to_storage<F>(
        &self,
        url: &str,
        storage: &vfs::OpendalFileStorage,
        path: &str,
        mut on_progress: F,
    ) -> Result<u64>
    where
        F: FnMut(u64, Option<u64>),
    {
        use futures::StreamExt;

        let resp = self.client.get(url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(GwasCatalogError::Api {
                status: status.as_u16(),
                message: format!("download failed for {url}"),
            });
        }

        let total = resp.content_length();
        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::with_capacity(total.unwrap_or(0) as usize);
        let mut downloaded: u64 = 0;

        while let Some(chunk) = stream.next().await {
            let bytes = chunk?;
            buf.extend_from_slice(&bytes);
            downloaded += bytes.len() as u64;
            on_progress(downloaded, total);
        }

        storage
            .op
            .write(path, buf)
            .await
            .map_err(|e| GwasCatalogError::Storage(e.to_string()))?;

        Ok(downloaded)
    }
}

/// Extract `href` values from `<a href="...">` tags in an HTML directory
/// listing, skipping parent-directory (`../`) and anchor (`#`) links.
fn parse_html_links(html: &str) -> Vec<String> {
    let mut links = Vec::new();
    let bytes = html.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Find the next `<a ` (case-insensitive).
        if bytes[i] == b'<' && i + 2 < bytes.len() {
            let tag = &bytes[i..i + 3].to_ascii_lowercase();
            if tag == b"<a " || tag == b"<a\t" || tag == b"<a\n" {
                // Find the closing '>'.
                if let Some(close) = html[i..].find('>') {
                    let attrs = &html[i..i + close];
                    if let Some(href) = extract_href(attrs) {
                        if href != "../" && !href.starts_with('#') && !href.starts_with('?') {
                            links.push(href);
                        }
                    }
                    i += close + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    links
}

/// Extract the value of the first `href="..."` attribute (case-insensitive),
/// tolerating both `"` and `'` quoting.
fn extract_href(attrs: &str) -> Option<String> {
    let lower = attrs.to_ascii_lowercase();
    let pos = lower.find("href")?;
    let after = &attrs[pos + 4..];
    // Skip whitespace and `=`.
    let after = after.trim_start();
    let after = after.strip_prefix('=')?;
    let after = after.trim_start();
    let quote = after.chars().next()?;
    if quote == '"' || quote == '\'' {
        let rest = &after[1..];
        let end = rest.find(quote)?;
        Some(rest[..end].to_string())
    } else {
        // Unquoted — read until whitespace.
        let end = after
            .find(|c: char| c.is_whitespace())
            .unwrap_or(after.len());
        Some(after[..end].to_string())
    }
}

impl Default for GwasCatalogClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ftp_block_url_high_series() {
        let c = GwasCatalogClient::new();
        let url = c.ftp_block_url("GCST90000061").unwrap();
        assert_eq!(
            url,
            "https://ftp.ebi.ac.uk/pub/databases/gwas/summary_statistics/GCST90000001-GCST90001000"
        );
    }

    #[test]
    fn ftp_block_url_low_series() {
        let c = GwasCatalogClient::new();
        let url = c.ftp_block_url("GCST005300").unwrap();
        assert_eq!(
            url,
            "https://ftp.ebi.ac.uk/pub/databases/gwas/summary_statistics/GCST005001-GCST006000"
        );
    }

    #[test]
    fn ftp_block_url_invalid() {
        let c = GwasCatalogClient::new();
        assert!(c.ftp_block_url("XYZ123").is_err());
        assert!(c.ftp_block_url("GCST").is_err());
        assert!(c.ftp_block_url("GCSTabc").is_err());
    }

    #[test]
    fn parse_html_links_extracts_dirs() {
        let html = r#"
        <html><body>
        <h1>Index</h1>
        <a href="../">Parent</a>
        <a href="GCST90000014/">GCST90000014/</a>
        <a href="GCST90000061/">GCST90000061/</a>
        <a href="?C=N;O=D">Name</a>
        </body></html>
        "#;
        let links = parse_html_links(html);
        assert_eq!(links, vec!["GCST90000014/", "GCST90000061/"]);
    }

    #[test]
    fn parse_html_links_extracts_files() {
        let html = r#"<a href="GCST90000061_buildGRCh37.tsv">file</a>
        <a href='GCST90000061_buildGRCh37.tsv-meta.yaml'>meta</a>"#;
        let links = parse_html_links(html);
        assert_eq!(
            links,
            vec![
                "GCST90000061_buildGRCh37.tsv",
                "GCST90000061_buildGRCh37.tsv-meta.yaml",
            ]
        );
    }
}
