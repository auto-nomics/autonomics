//! Adapter for the public NIH BioArt website, not an official NIH API SDK.
//! Search uses the site's discovered Next.js server action. Treat catalogue
//! descriptions as external data; retain per-asset licence and attribution.
//!
//! ```no_run
//! # async fn example() -> bioart::Result<()> {
//! let client = bioart::BioArtClient::new()?;
//! let page = client.search("T cell", 0, 20).await?;
//! for asset in page.assets {
//!     println!("{}: {} ({:?})", asset.accession, asset.title, asset.licenses);
//!     // File URLs can also be passed to the host's policy-gated downloader.
//! }
//! # Ok(())
//! # }
//! ```
//!
//! The upstream website is not a versioned public API: protocol drift returns
//! an explicit error. Search descriptions retain upstream HTML, and downloaded
//! SVG is untrusted. The adapter never infers Public Domain when a licence is
//! missing. Callers retain the asset metadata alongside downloaded bytes.
use regex::Regex;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::sync::OnceCell;
use url::Url;

pub const DEFAULT_ENDPOINT: &str = "https://bioart.niaid.nih.gov/";
const METADATA_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("HTTP transport: {0}")]
    Http(#[from] reqwest::Error),
    #[error("invalid endpoint: {0}")]
    Url(#[from] url::ParseError),
    #[error("BioArt protocol changed or invalid response: {0}")]
    Protocol(String),
    #[error("response exceeds {0} bytes")]
    TooLarge(usize),
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("asset {0} not found")]
    NotFound(u64),
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetFile {
    pub asset_id: u64,
    pub file_id: u64,
    /// Upstream format label, e.g. SVG, PNG, AI. Unknown formats are retained.
    pub format: String,
    pub url: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    pub id: u64,
    pub accession: String,
    pub title: String,
    /// May contain upstream HTML. Consumers must not render it unsanitized.
    pub description: String,
    pub source_url: String,
    pub licenses: Vec<String>,
    pub creators: Vec<String>,
    pub collections: Vec<String>,
    pub keywords: Vec<String>,
    pub ontology_terms: Vec<String>,
    pub created: Option<String>,
    pub updated: Option<String>,
    pub files: Vec<AssetFile>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchPage {
    pub total: u64,
    pub start: u64,
    pub assets: Vec<Asset>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Representation {
    pub group_id: u64,
    pub files: Vec<AssetFile>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetDetail {
    pub asset: Asset,
    pub representations: Vec<Representation>,
}
#[derive(Debug)]
pub struct Download {
    pub file: AssetFile,
    pub content_type: Option<String>,
    pub sha256: String,
    pub bytes: Vec<u8>,
}
/// Bounded PNG and provenance, ready to embed in an editable scene.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanvasImage {
    pub asset: Asset,
    pub file: AssetFile,
    pub sha256: String,
    #[serde(rename = "dataURL")]
    pub data_url: String,
    pub width: u32,
    pub height: u32,
}
#[derive(Clone)]
pub struct BioArtClient {
    http: reqwest::Client,
    endpoint: Url,
    search_action: Arc<OnceCell<String>>,
}
impl BioArtClient {
    pub fn new() -> Result<Self> {
        Self::with_endpoint(DEFAULT_ENDPOINT)
    }
    /// Custom origin for tests or a trusted mirror. No credentials are required.
    pub fn with_endpoint(endpoint: &str) -> Result<Self> {
        let endpoint = Url::parse(endpoint)?;
        if !["http", "https"].contains(&endpoint.scheme())
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.path() != "/"
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(Error::Invalid("endpoint must be an HTTP(S) origin".into()));
        }
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(45))
                .user_agent("autonomics-bioart/0.1")
                .redirect(reqwest::redirect::Policy::limited(5))
                .build()?,
            endpoint,
            search_action: Arc::new(OnceCell::new()),
        })
    }
    pub fn asset_url(&self, id: u64) -> String {
        self.endpoint
            .join(&format!("bioart/{id}"))
            .expect("static URL path")
            .to_string()
    }
    pub fn file(&self, asset_id: u64, file_id: u64, format: impl Into<String>) -> AssetFile {
        AssetFile {
            asset_id,
            file_id,
            format: format.into(),
            url: self
                .endpoint
                .join(&format!("api/bioarts/{asset_id}/files/{file_id}"))
                .expect("static URL path")
                .to_string(),
        }
    }
    async fn text(&self, url: Url) -> Result<String> {
        let bytes = bounded(self.http.get(url).send().await?, METADATA_LIMIT).await?;
        String::from_utf8(bytes).map_err(|e| Error::Protocol(e.to_string()))
    }
    async fn action(&self) -> Result<&str> {
        let action = self
            .search_action
            .get_or_try_init(|| async {
                let html = self.text(self.endpoint.join("discover")?).await?;
                let scripts: Vec<String> = {
                    let doc = Html::parse_document(&html);
                    doc.select(&Selector::parse("script[src]").expect("static selector"))
                        .filter_map(|n| n.value().attr("src"))
                        .filter(|src| src.contains("/discover/page-"))
                        .map(str::to_owned)
                        .collect()
                };
                let pattern =
                    Regex::new(r#"createServerReference\)\("([a-f0-9]+)"[^;]*?"discoverSearch"\)"#)
                        .expect("static regex");
                for script in scripts {
                    let url = self.endpoint.join(&script)?;
                    if url.origin() != self.endpoint.origin() {
                        continue;
                    }
                    let source = self.text(url).await?;
                    if let Some(capture) = pattern.captures(&source) {
                        return Ok(capture[1].to_owned());
                    }
                }
                Err(Error::Protocol("discoverSearch action not found".into()))
            })
            .await?;
        Ok(action.as_str())
    }
    async fn query(&self, query: String, start: u64, size: u32) -> Result<SearchPage> {
        if size == 0 || size > 100 {
            return Err(Error::Invalid("size must be 1..=100".into()));
        }
        let action = self.action().await?;
        let body = serde_json::to_string(&[format!("{query}?start={start}&size={size}")])
            .map_err(|e| Error::Protocol(e.to_string()))?;
        let response = self
            .http
            .post(self.endpoint.join("discover")?)
            .header("Next-Action", action)
            .header("Content-Type", "text/plain;charset=UTF-8")
            .header("Origin", self.endpoint.origin().ascii_serialization())
            .body(body)
            .send()
            .await?;
        let bytes = bounded(response, METADATA_LIMIT).await?;
        let wire = std::str::from_utf8(&bytes).map_err(|e| Error::Protocol(e.to_string()))?;
        let value = wire
            .lines()
            .filter_map(|line| line.split_once(':'))
            .filter_map(|(_, json)| serde_json::from_str::<Value>(json).ok())
            .find(|v| v.get("hits").is_some())
            .ok_or_else(|| Error::Protocol("search hits missing".into()))?;
        parse_page(&value, self)
    }
    /// Literal keyword search, with an explicit bounded page (no implicit crawl).
    pub async fn search(&self, text: &str, start: u64, size: u32) -> Result<SearchPage> {
        if text.len() > 1024 || text.chars().any(char::is_control) {
            return Err(Error::Invalid(
                "query too long or contains control characters".into(),
            ));
        }
        let query = if text.trim().is_empty() {
            "type:bioart".into()
        } else {
            format!(
                "type:bioart AND \"{}\"",
                text.replace('\\', "\\\\").replace('"', "\\\"")
            )
        };
        self.query(query, start, size).await
    }
    pub async fn asset(&self, id: u64) -> Result<Asset> {
        self.query(format!("type:bioart AND id:{id}"), 0, 1)
            .await?
            .assets
            .into_iter()
            .find(|a| a.id == id)
            .ok_or(Error::NotFound(id))
    }
    pub async fn detail(&self, id: u64) -> Result<AssetDetail> {
        let asset = self.asset(id).await?;
        let html = self.text(Url::parse(&asset.source_url)?).await?;
        let representations = parse_representations(&html, id, self)?;
        Ok(AssetDetail {
            asset,
            representations,
        })
    }
    pub async fn canvas_image(&self, id: u64) -> Result<CanvasImage> {
        use base64::Engine as _;
        let detail = self.detail(id).await?;
        let file = detail
            .asset
            .files
            .iter()
            .chain(
                detail
                    .representations
                    .iter()
                    .flat_map(|group| group.files.iter()),
            )
            .find(|file| file.format.eq_ignore_ascii_case("PNG"))
            .ok_or_else(|| Error::Invalid("asset has no PNG representation".into()))?;
        let download = self.download(file, 8 * 1024 * 1024).await?;
        let (width, height) = png_dimensions(&download.bytes)?;
        Ok(CanvasImage {
            asset: detail.asset,
            file: download.file,
            sha256: download.sha256,
            data_url: format!(
                "data:image/png;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(download.bytes)
            ),
            width,
            height,
        })
    }
    /// Downloads only the numeric upstream asset/file path, never a caller URL.
    /// Returned SVG/XML is untrusted content and must be sanitized before display.
    pub async fn download(&self, file: &AssetFile, max_bytes: usize) -> Result<Download> {
        if max_bytes == 0 {
            return Err(Error::Invalid("max_bytes must be positive".into()));
        }
        let canonical = self.file(file.asset_id, file.file_id, &file.format);
        let response = self.http.get(&canonical.url).send().await?;
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let bytes = bounded(response, max_bytes).await?;
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        Ok(Download {
            file: canonical,
            content_type,
            sha256,
            bytes,
        })
    }
}
async fn bounded(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    response.error_for_status_ref()?;
    if response.content_length().is_some_and(|n| n > limit as u64) {
        return Err(Error::TooLarge(limit));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(Error::TooLarge(limit));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
fn parse_page(value: &Value, client: &BioArtClient) -> Result<SearchPage> {
    let hits = &value["hits"];
    let total = hits["found"]
        .as_u64()
        .ok_or_else(|| Error::Protocol("total missing".into()))?;
    let start = hits["start"]
        .as_u64()
        .ok_or_else(|| Error::Protocol("start missing".into()))?;
    let rows = hits["hit"]
        .as_array()
        .ok_or_else(|| Error::Protocol("hit list missing".into()))?;
    let mut assets = Vec::new();
    for row in rows {
        let f = &row["fields"];
        let list = |key: &str| -> Vec<String> {
            f[key]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        };
        let first = |key: &str| list(key).into_iter().next();
        let id = first("id")
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| Error::Protocol("asset id missing".into()))?;
        let mut files = Vec::new();
        for info in list("filesinfo") {
            for group in info.split('|') {
                if let Some((format, ids)) = group.split_once(':') {
                    if format == "All" {
                        continue;
                    }
                    for file_id in ids.split(',') {
                        let file_id = file_id
                            .parse::<u64>()
                            .map_err(|_| Error::Protocol("invalid file id".into()))?;
                        files.push(client.file(id, file_id, format));
                    }
                }
            }
        }
        assets.push(Asset {
            id,
            accession: format!("BIOART-{id:06}"),
            title: first("title").ok_or_else(|| Error::Protocol("title missing".into()))?,
            description: first("description").unwrap_or_default(),
            source_url: client.asset_url(id),
            licenses: list("license"),
            creators: list("creator"),
            collections: list("collection"),
            keywords: list("keywords"),
            ontology_terms: list("ontologykey"),
            created: first("created"),
            updated: first("updated"),
            files,
        });
    }
    Ok(SearchPage {
        total,
        start,
        assets,
    })
}
fn parse_representations(
    html: &str,
    asset_id: u64,
    client: &BioArtClient,
) -> Result<Vec<Representation>> {
    let doc = Html::parse_document(html);
    let mut flight = String::new();
    for script in doc.select(&Selector::parse("script").expect("static selector")) {
        let text = script.inner_html();
        if let Some(raw) = text
            .strip_prefix("self.__next_f.push(")
            .and_then(|s| s.strip_suffix(')'))
        {
            if let Ok(value) = serde_json::from_str::<Value>(raw) {
                if let Some(s) = value.get(1).and_then(Value::as_str) {
                    flight.push_str(s);
                }
            }
        }
    }
    let marker = "\"filemapping\":";
    let index = flight
        .find(marker)
        .ok_or_else(|| Error::Protocol("representation mapping missing".into()))?
        + marker.len();
    let mapping = serde_json::Deserializer::from_str(&flight[index..])
        .into_iter::<BTreeMap<String, BTreeMap<String, u64>>>()
        .next()
        .ok_or_else(|| Error::Protocol("representation mapping empty".into()))?
        .map_err(|e| Error::Protocol(e.to_string()))?;
    mapping
        .into_iter()
        .map(|(group, files)| {
            Ok(Representation {
                group_id: group
                    .parse()
                    .map_err(|_| Error::Protocol("invalid group id".into()))?,
                files: files
                    .into_iter()
                    .map(|(format, id)| client.file(asset_id, id, format))
                    .collect(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_license_is_not_inferred_and_unknown_formats_survive() {
        let client = BioArtClient::new().unwrap();
        let value = serde_json::json!({"hits":{"found":1,"start":0,"hit":[{"fields":{"id":["7"],"title":["Unlicensed asset"],"filesinfo":["All:22|NEW_FORMAT:22"]}}]}});
        let page = parse_page(&value, &client).unwrap();
        assert!(page.assets[0].licenses.is_empty());
        assert_eq!(page.assets[0].files[0].format, "NEW_FORMAT");
        assert!(parse_page(&serde_json::json!({"hits":{"found":1}}), &client).is_err());
    }
}

fn png_dimensions(bytes: &[u8]) -> Result<(u32, u32)> {
    if bytes.len() < 24 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") || &bytes[12..16] != b"IHDR" {
        return Err(Error::Protocol("invalid PNG header".into()));
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 16_000_000 {
        return Err(Error::Invalid("PNG dimensions exceed import limit".into()));
    }
    Ok((width, height))
}
#[cfg(test)]
mod png_tests {
    use super::*;
    #[test]
    fn rejects_invalid_and_oversized_png_headers() {
        assert!(png_dimensions(&[]).is_err());
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        bytes.extend(20u32.to_be_bytes());
        bytes.extend(10u32.to_be_bytes());
        assert_eq!(png_dimensions(&bytes).unwrap(), (20, 10));
        bytes[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(png_dimensions(&bytes).is_err());
    }
}
