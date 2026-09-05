//! Async client for the MinerU v4 "Precision Extract" cloud API.
//!
//! MinerU (<https://mineru.net>) converts PDFs into markdown plus structured
//! content blocks in the cloud. This crate wraps the file-upload flavour of
//! the v4 API:
//!
//! 1. `POST {base}/api/v4/file-urls/batch` claims a presigned upload URL,
//! 2. the PDF bytes are `PUT` to that URL (**without** a `Content-Type`
//!    header — the OSS signature matches the bare PUT, any content type
//!    breaks it),
//! 3. MinerU picks the file up on its own and the batch is polled at
//!    `GET {base}/api/v4/extract-results/batch/{batch_id}` until it reaches
//!    `done` or `failed`,
//! 4. the result zip (`full.md`, `content_list.json`, `middle.json`) is
//!    downloaded and decoded by [`parse_mineru_zip`].
//!
//! The client is key-optional by design: [`MineruClient::from_env`] seeds the
//! token from `MINERU_API_TOKEN` (and the base URL from `ENDPOINT_MINERU_URL`
//! following the `ENDPOINT_*_URL` convention used by the other API crates),
//! while [`MineruClient::set_key`] / [`MineruClient::set_base_url`] allow the
//! running server to hot-swap both from persisted settings.
//!
//! Ported from jayread's `jayread-mineru` crate; the image extraction and
//! `middle.json` layout-block parsing are intentionally left behind until a
//! consumer needs them — v1 of the parse pipeline only stores markdown.

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Default MinerU cloud endpoint.
pub const DEFAULT_BASE_URL: &str = "https://mineru.net";

/// Model backend requested from MinerU. jayread used `vlm`; it has the best
/// layout / table / formula quality, which suits academic papers.
const MODEL_VERSION: &str = "vlm";

/// Fixed upload name — MinerU only needs an extension to route the file.
const UPLOAD_NAME: &str = "upload.pdf";

// Per-request timeouts (seconds): URL claim / PUT / poll / zip download.
const CLAIM_TIMEOUT: Duration = Duration::from_secs(300);
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(600);
const POLL_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// 120 polls × 5 s aligns with the 10-minute overall deadline.
const MAX_POLL_ATTEMPTS: u32 = 120;
const OVERALL_TIMEOUT: Duration = Duration::from_secs(600);

/// Process-wide connection pool. The MinerU flow mixes very short polls with
/// multi-minute uploads, so no client-level timeout is set — every request
/// carries its own.
static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn http_client() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .build()
            .expect("failed to build MinerU HTTP client")
    })
}

/// Failures of the MinerU round trip.
#[derive(Debug, Error)]
pub enum Error {
    #[error("MinerU API token is not configured")]
    NotConfigured,
    #[error("MinerU request failed: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("MinerU request failed (HTTP {status}): {body}")]
    Http {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("MinerU API error ({code}): {msg}")]
    Api { code: i32, msg: String },
    #[error("invalid MinerU response: {0}")]
    InvalidResponse(String),
}

/// Parsed MinerU result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseResult {
    /// Full markdown document (`full.md`).
    pub markdown: String,
    /// Content blocks from `content_list.json`.
    pub blocks: Vec<RawBlock>,
    /// Physical page sizes in PDF points, keyed by 0-based page index
    /// (`middle.json.pdf_info[i].page_size`).
    #[serde(default)]
    pub page_sizes: HashMap<i32, [f64; 2]>,
}

/// One content block from the MinerU result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawBlock {
    /// 0-based page index.
    pub page_idx: i32,
    /// text / title / table / image / equation / list.
    pub block_type: String,
    /// Text content (markdown table for table blocks, `[image:N]` for images).
    pub text: String,
    /// Bounding box `[x0, y0, x1, y1]`.
    pub bbox: [f64; 4],
    /// Heading level for title blocks.
    pub level: Option<i32>,
    /// Reading order within the result.
    pub reading_order: Option<i32>,
    /// Image path inside the result zip, when present.
    pub img_path: Option<String>,
    /// Raw table payload (compact JSON: 2-D cell array, or HTML string).
    pub table_body: Option<String>,
    pub table_caption: Option<String>,
}

/// MinerU v4 client with a hot-swappable token and base URL.
#[derive(Debug)]
pub struct MineruClient {
    base_url: RwLock<String>,
    api_key: RwLock<Option<String>>,
    /// Result-poll cadence; tests shrink it from the 5 s default.
    poll_interval: RwLock<Duration>,
}

impl Default for MineruClient {
    fn default() -> Self {
        Self::new()
    }
}

impl MineruClient {
    pub fn new() -> Self {
        Self {
            base_url: RwLock::new(DEFAULT_BASE_URL.to_owned()),
            api_key: RwLock::new(None),
            poll_interval: RwLock::new(POLL_INTERVAL),
        }
    }

    /// Build a client from `ENDPOINT_MINERU_URL` / `MINERU_API_TOKEN`.
    pub fn from_env() -> Self {
        let client = Self::new();
        if let Ok(url) = std::env::var("ENDPOINT_MINERU_URL") {
            let url = url.trim().to_owned();
            if !url.is_empty() {
                client.set_base_url(url);
            }
        }
        if let Ok(key) = std::env::var("MINERU_API_TOKEN") {
            let key = key.trim().to_owned();
            if !key.is_empty() {
                client.set_key(Some(key));
            }
        }
        client
    }

    /// Replace the API token (settings hot-swap); `None` clears it.
    pub fn set_key(&self, key: Option<String>) {
        *self.api_key.write().expect("mineru key lock") = key;
    }

    /// Whether a token is available — callers gate PDF uploads on this.
    pub fn has_key(&self) -> bool {
        self.api_key
            .read()
            .expect("mineru key lock")
            .as_deref()
            .is_some_and(|key| !key.trim().is_empty())
    }

    /// Replace the base URL (settings hot-swap); trailing slashes are trimmed.
    pub fn set_base_url(&self, url: String) {
        let url = url.trim().trim_end_matches('/').to_owned();
        *self.base_url.write().expect("mineru url lock") = url;
    }

    /// Override the result-poll cadence (tests use tens of milliseconds).
    pub fn set_poll_interval(&self, interval: Duration) {
        *self.poll_interval.write().expect("mineru poll lock") = interval;
    }

    fn base_url(&self) -> String {
        self.base_url.read().expect("mineru url lock").clone()
    }

    fn bearer(&self) -> Result<String, Error> {
        let key = self
            .api_key
            .read()
            .expect("mineru key lock")
            .clone()
            .filter(|key| !key.trim().is_empty())
            .ok_or(Error::NotConfigured)?;
        Ok(format!("Bearer {key}"))
    }

    /// Parse a PDF through MinerU, returning markdown plus content blocks.
    ///
    /// `on_progress` fires on every poll that reports page-level progress
    /// (`extract_progress.extracted_pages`, `total_pages`); `total_pages` is 0
    /// until MinerU starts on the file.
    pub async fn parse_pdf(
        &self,
        pdf_bytes: Vec<u8>,
        on_progress: impl Fn(u64, u64),
    ) -> Result<ParseResult, Error> {
        let deadline = tokio::time::Instant::now() + OVERALL_TIMEOUT;
        let client = http_client();

        let size_mb = pdf_bytes.len() as f64 / 1_048_576.0;
        if size_mb > 200.0 {
            tracing::warn!(
                "PDF is {size_mb:.1} MB, above MinerU's usual 200 MB per-file limit; parse may fail"
            );
        }

        let zip_url = self.submit_and_poll(client, pdf_bytes, deadline, on_progress).await?;

        tracing::info!("downloading MinerU result zip");
        let bytes = client
            .get(&zip_url)
            .timeout(DOWNLOAD_TIMEOUT)
            .send()
            .await?
            .error_for_status()
            .map_err(|error| Error::Http {
                status: error.status().unwrap_or_default(),
                body: "failed to download result zip".to_owned(),
            })?
            .bytes()
            .await?;
        tracing::info!("MinerU result zip downloaded ({} bytes)", bytes.len());

        parse_mineru_zip(&bytes)
    }

    /// Steps 1–3: claim an upload URL, PUT the file, poll until finished.
    async fn submit_and_poll(
        &self,
        client: &reqwest::Client,
        pdf_bytes: Vec<u8>,
        deadline: tokio::time::Instant,
        on_progress: impl Fn(u64, u64),
    ) -> Result<String, Error> {
        let base = self.base_url();
        let bearer = self.bearer()?;

        // Step 1: claim a presigned upload URL.
        let claim = client
            .post(format!("{base}/api/v4/file-urls/batch"))
            .timeout(CLAIM_TIMEOUT)
            .header("Authorization", bearer.clone())
            .json(&BatchUploadRequest {
                files: vec![BatchUploadFile {
                    name: UPLOAD_NAME.to_owned(),
                }],
                model_version: MODEL_VERSION.to_owned(),
            })
            .send()
            .await?;
        let claim = ensure_success(claim).await?;
        let claim: ApiResponse<BatchUploadData> = claim.json().await?;
        let claim = claim.into_result()?;
        let batch_id = claim.batch_id;
        let upload_url = claim
            .file_urls
            .into_iter()
            .next()
            .ok_or_else(|| Error::InvalidResponse("claim response has no file_urls".into()))?;
        tracing::info!("MinerU batch {batch_id} claimed, uploading PDF");

        // Step 2: PUT the bytes. No Content-Type header — the presigned URL
        // signature covers exactly this request shape.
        let put = client
            .put(&upload_url)
            .timeout(UPLOAD_TIMEOUT)
            .body(pdf_bytes)
            .send()
            .await?;
        ensure_success(put).await?;
        tracing::info!("PDF uploaded to MinerU, polling for results");

        // Step 3: poll the batch until it settles.
        let result_url = format!("{base}/api/v4/extract-results/batch/{batch_id}");
        let mut attempts = 0u32;
        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::InvalidResponse(
                    "parse did not finish within 10 minutes".into(),
                ));
            }
            // Copy out the interval first: holding the RwLock guard across
            // the sleep would make the future !Send.
            let interval = *self.poll_interval.read().expect("mineru poll lock");
            tokio::time::sleep(interval).await;

            let response = client
                .get(&result_url)
                .timeout(POLL_TIMEOUT)
                .header("Authorization", bearer.clone())
                .send()
                .await?;
            let response = ensure_success(response).await?;
            let response: ApiResponse<BatchResultData> = response.json().await?;
            let data = response.into_result()?;
            let item = data
                .extract_result
                .into_iter()
                .next()
                .ok_or_else(|| Error::InvalidResponse("poll response has no extract_result".into()))?;

            match item.state.as_str() {
                "done" => {
                    return item.full_zip_url.ok_or_else(|| {
                        Error::InvalidResponse("task finished without a download URL".into())
                    })
                }
                "failed" => {
                    return Err(Error::Api {
                        code: 0,
                        msg: item.err_msg.unwrap_or_else(|| "unknown MinerU failure".into()),
                    })
                }
                "waiting-file" | "pending" | "running" | "converting" => {
                    if let Some(progress) = &item.extract_progress {
                        on_progress(progress.extracted_pages, progress.total_pages);
                    }
                    attempts += 1;
                    if attempts >= MAX_POLL_ATTEMPTS {
                        return Err(Error::InvalidResponse(
                            "parse did not finish within the poll budget".into(),
                        ));
                    }
                }
                other => {
                    return Err(Error::InvalidResponse(format!("unknown task state {other}")))
                }
            }
        }
    }
}

/// Fail a non-2xx response, including the body for diagnostics.
async fn ensure_success(response: reqwest::Response) -> Result<reqwest::Response, Error> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    Err(Error::Http { status, body })
}

// ---------------------------------------------------------------------------
// Wire types (private)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct BatchUploadRequest {
    files: Vec<BatchUploadFile>,
    model_version: String,
}

#[derive(Debug, Serialize)]
struct BatchUploadFile {
    name: String,
}

#[derive(Debug, Deserialize)]
struct ApiResponse<T> {
    code: i32,
    msg: String,
    // Option fields default to None when the key is absent; no `#[serde(default)]`
    // — on a generic field it would demand `T: Default`.
    data: Option<T>,
}

impl<T> ApiResponse<T> {
    fn into_result(self) -> Result<T, Error> {
        if self.code != 0 {
            return Err(Error::Api {
                code: self.code,
                msg: self.msg,
            });
        }
        self.data.ok_or_else(|| Error::InvalidResponse("response has no data".into()))
    }
}

#[derive(Debug, Deserialize)]
struct BatchUploadData {
    batch_id: String,
    file_urls: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct BatchResultData {
    #[serde(default)]
    extract_result: Vec<ExtractResultItem>,
}

#[derive(Debug, Deserialize)]
struct ExtractResultItem {
    state: String,
    #[serde(default)]
    full_zip_url: Option<String>,
    #[serde(default)]
    err_msg: Option<String>,
    #[serde(default)]
    extract_progress: Option<ExtractProgress>,
}

#[derive(Debug, Deserialize)]
struct ExtractProgress {
    #[serde(default)]
    extracted_pages: u64,
    #[serde(default)]
    total_pages: u64,
}

// ---------------------------------------------------------------------------
// Result zip decoding
// ---------------------------------------------------------------------------

/// Decode a MinerU result zip into a [`ParseResult`].
///
/// Zip layout: `{filename}/full.md`, `…/content_list.json`, `…/middle.json`
/// (entries may also appear without the directory prefix). `full.md` is
/// required; the JSON parts are optional and simply leave `blocks` /
/// `page_sizes` empty when absent.
pub fn parse_mineru_zip(zip_bytes: &[u8]) -> Result<ParseResult, Error> {
    use std::io::Read;

    let reader = std::io::Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(reader)
        .map_err(|error| Error::InvalidResponse(format!("failed to open result zip: {error}")))?;

    let mut markdown: Option<String> = None;
    let mut content_list: Option<String> = None;
    let mut middle: Option<String> = None;

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| Error::InvalidResponse(format!("failed to read zip entry: {error}")))?;
        let name = entry.name().to_owned();

        if name.ends_with("/full.md") || name == "full.md" {
            let mut text = String::new();
            entry
                .read_to_string(&mut text)
                .map_err(|error| Error::InvalidResponse(format!("failed to read full.md: {error}")))?;
            markdown = Some(text);
        } else if name.ends_with("/content_list.json") || name.ends_with("_content_list.json") {
            let mut text = String::new();
            entry.read_to_string(&mut text).map_err(|error| {
                Error::InvalidResponse(format!("failed to read content_list.json: {error}"))
            })?;
            content_list = Some(text);
        } else if name.ends_with("/middle.json") || name == "middle.json" {
            let mut text = String::new();
            entry.read_to_string(&mut text).map_err(|error| {
                Error::InvalidResponse(format!("failed to read middle.json: {error}"))
            })?;
            middle = Some(text);
        }
    }

    let markdown = markdown
        .ok_or_else(|| Error::InvalidResponse("result zip contains no full.md".into()))?;
    let blocks = content_list
        .as_deref()
        .map(parse_content_list)
        .transpose()?
        .unwrap_or_default();
    let page_sizes = middle
        .as_deref()
        .map(parse_page_sizes)
        .transpose()?
        .unwrap_or_default();

    Ok(ParseResult {
        markdown,
        blocks,
        page_sizes,
    })
}

#[derive(Debug, Deserialize)]
struct ContentItem {
    #[serde(rename = "type")]
    item_type: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    page_idx: Option<i32>,
    #[serde(default)]
    bbox: Option<Vec<f64>>,
    #[serde(default)]
    text_level: Option<i32>,
    #[serde(default)]
    img_idx: Option<i64>,
    #[serde(default)]
    img_path: Option<String>,
    #[serde(default)]
    table_body: Option<serde_json::Value>,
    #[serde(default)]
    table_caption: Option<String>,
    #[serde(default)]
    image_caption: Option<String>,
}

/// Decode `content_list.json` into blocks.
///
/// Text blocks here can be merged across pages by MinerU — fine for the
/// current markdown-only consumer; a future paragraph-level consumer should
/// switch to `middle.json.pdf_info[].para_blocks` the way jayread did.
fn parse_content_list(json: &str) -> Result<Vec<RawBlock>, Error> {
    let items: Vec<ContentItem> = serde_json::from_str(json)
        .map_err(|error| Error::InvalidResponse(format!("bad content_list.json: {error}")))?;

    let mut blocks = Vec::new();
    for (index, item) in items.into_iter().enumerate() {
        let bbox = match item.bbox.as_deref() {
            Some(values) if values.len() >= 4 => [values[0], values[1], values[2], values[3]],
            _ => [0.0, 0.0, 0.0, 0.0],
        };
        let page_idx = item.page_idx.unwrap_or(0);

        let block_type = match item.item_type.as_str() {
            "title" | "section_header" => "title",
            "table" => "table",
            "image" => "image",
            "equation" => "equation",
            "list" => "list",
            _ => "text",
        };

        let text = match block_type {
            "table" => match &item.table_body {
                Some(serde_json::Value::Array(rows)) => format_table_markdown(rows),
                Some(serde_json::Value::String(html)) => html.clone(),
                Some(other) => other.to_string(),
                None => item.text.clone().unwrap_or_default(),
            },
            "image" => match (item.img_idx, &item.img_path) {
                (Some(idx), _) => format!("[image_{idx}]"),
                (None, Some(path)) => format!("[image:{path}]"),
                (None, None) => item.text.clone().unwrap_or_default(),
            },
            _ => item.text.clone().unwrap_or_default(),
        };

        if !text.trim().is_empty() {
            let level = item.text_level.or(if item.item_type == "section_header" {
                Some(1)
            } else {
                None
            });
            blocks.push(RawBlock {
                page_idx,
                block_type: block_type.to_owned(),
                text,
                bbox,
                level,
                reading_order: Some(index as i32),
                img_path: item.img_path.clone(),
                table_body: item.table_body.as_ref().map(|value| value.to_string()),
                table_caption: item.table_caption.clone(),
            });
        }

        // Image captions ride on the image item; emit them as their own text
        // block so they survive in reading order.
        let caption = item
            .image_caption
            .as_deref()
            .map(str::trim)
            .filter(|caption| !caption.is_empty());
        if let Some(caption) = caption {
            blocks.push(RawBlock {
                page_idx,
                block_type: "text".to_owned(),
                text: caption.to_owned(),
                bbox,
                level: None,
                reading_order: Some(index as i32),
                img_path: None,
                table_body: None,
                table_caption: None,
            });
        }
    }
    Ok(blocks)
}

/// Render MinerU's 2-D cell array as a markdown table.
fn format_table_markdown(rows: &[serde_json::Value]) -> String {
    let render_row = |row: &serde_json::Value| -> String {
        let cells = row
            .as_array()
            .map(|cells| {
                cells
                    .iter()
                    .map(|cell| match cell {
                        serde_json::Value::String(text) => text.trim().to_owned(),
                        other => other.to_string(),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        format!("| {} |", cells.join(" | "))
    };
    let mut lines = Vec::with_capacity(rows.len() + 1);
    for (index, row) in rows.iter().enumerate() {
        lines.push(render_row(row));
        if index == 0 {
            let width = cells_in(rows.first());
            lines.push(format!("| {} |", vec!["---"; width].join(" | ")));
        }
    }
    lines.join("\n")
}

/// Column count of the first row, at least 1, for the separator line.
fn cells_in(row: Option<&serde_json::Value>) -> usize {
    row.and_then(serde_json::Value::as_array)
        .map_or(1, |cells| cells.len().max(1))
}

#[derive(Debug, Deserialize)]
struct MiddleJson {
    #[serde(default)]
    pdf_info: Vec<MiddlePage>,
}

#[derive(Debug, Deserialize)]
struct MiddlePage {
    #[serde(default)]
    page_idx: Option<i32>,
    #[serde(default)]
    page_size: Option<Vec<f64>>,
}

fn parse_page_sizes(json: &str) -> Result<HashMap<i32, [f64; 2]>, Error> {
    let middle: MiddleJson = serde_json::from_str(json)
        .map_err(|error| Error::InvalidResponse(format!("bad middle.json: {error}")))?;
    let mut sizes = HashMap::new();
    for (index, page) in middle.pdf_info.into_iter().enumerate() {
        if let Some(size) = page.page_size.as_deref() {
            if size.len() >= 2 {
                sizes.insert(page.page_idx.unwrap_or(index as i32), [size[0], size[1]]);
            }
        }
    }
    Ok(sizes)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Build an in-memory MinerU result zip from (name, bytes) entries.
    fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let buffer = std::io::Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(buffer);
        for (name, bytes) in entries {
            zip.start_file::<_, ()>(name.to_owned(), zip::write::SimpleFileOptions::default())
                .expect("start zip entry");
            zip.write_all(bytes).expect("write zip entry");
        }
        let buffer = zip.finish().expect("finish zip");
        buffer.into_inner()
    }

    const MARKDOWN: &str = "# Title\n\nparagraph text\n";

    fn content_list() -> String {
        serde_json::json!([
            { "type": "title", "text": "Title", "page_idx": 0, "text_level": 1,
              "bbox": [10.0, 20.0, 200.0, 40.0] },
            { "type": "text", "text": "paragraph text", "page_idx": 0,
              "bbox": [10.0, 50.0, 200.0, 80.0] },
            { "type": "table", "page_idx": 1, "bbox": [1.0, 2.0, 3.0, 4.0],
              "table_body": [["h1", "h2"], ["a", "b"]], "table_caption": "Table 1" },
            { "type": "image", "page_idx": 2, "img_path": "images/abc.jpg",
              "image_caption": "Figure 1" },
            { "type": "text", "text": "  ", "page_idx": 2 }
        ])
        .to_string()
    }

    fn middle_json() -> String {
        serde_json::json!({ "pdf_info": [
            { "page_idx": 0, "page_size": [595.0, 842.0] },
            { "page_idx": 1 }
        ]})
        .to_string()
    }

    #[test]
    fn parses_full_result_zip() {
        let bytes = build_zip(&[
            ("paper/full.md", MARKDOWN.as_bytes()),
            ("paper/content_list.json", content_list().as_bytes()),
            ("paper/middle.json", middle_json().as_bytes()),
        ]);
        let result = parse_mineru_zip(&bytes).expect("zip parses");

        assert_eq!(result.markdown, MARKDOWN);
        // title + text + table + image placeholder + caption; the blank text
        // item is dropped.
        assert_eq!(result.blocks.len(), 5);
        assert_eq!(result.blocks[0].block_type, "title");
        assert_eq!(result.blocks[0].level, Some(1));
        assert_eq!(result.blocks[1].block_type, "text");

        let table = &result.blocks[2];
        assert_eq!(table.block_type, "table");
        assert!(table.text.contains("| h1 | h2 |"));
        assert_eq!(table.table_caption.as_deref(), Some("Table 1"));

        let image = &result.blocks[3];
        assert_eq!(image.block_type, "image");
        assert_eq!(image.text, "[image:images/abc.jpg]");
        assert_eq!(result.blocks[4].text, "Figure 1");

        assert_eq!(result.page_sizes.get(&0), Some(&[595.0, 842.0]));
        assert!(!result.page_sizes.contains_key(&1));
    }

    #[test]
    fn accepts_entries_without_directory_prefix() {
        let bytes = build_zip(&[("full.md", MARKDOWN.as_bytes())]);
        let result = parse_mineru_zip(&bytes).expect("zip parses");
        assert_eq!(result.markdown, MARKDOWN);
        assert!(result.blocks.is_empty());
        assert!(result.page_sizes.is_empty());
    }

    #[test]
    fn missing_markdown_is_an_error() {
        let bytes = build_zip(&[("paper/content_list.json", content_list().as_bytes())]);
        let error = parse_mineru_zip(&bytes).expect_err("full.md is required");
        assert!(error.to_string().contains("no full.md"));
    }

    #[test]
    fn bad_content_list_json_is_an_error() {
        let bytes = build_zip(&[
            ("paper/full.md", MARKDOWN.as_bytes()),
            ("paper/content_list.json", b"{not json"),
        ]);
        assert!(parse_mineru_zip(&bytes).is_err());
    }

    #[test]
    fn poll_payloads_deserialize() {
        let running: ApiResponse<BatchResultData> = serde_json::from_str(
            r#"{"code":0,"msg":"ok","data":{"batch_id":"b","extract_result":[
                {"file_name":"a.pdf","state":"running","err_msg":"",
                 "extract_progress":{"extracted_pages":1,"total_pages":2}}]}}"#,
        )
        .expect("running payload parses");
        let data = running.into_result().expect("code is 0");
        let item = &data.extract_result[0];
        assert_eq!(item.state, "running");
        assert_eq!(
            (item.extract_progress.as_ref().unwrap().extracted_pages, item.extract_progress.as_ref().unwrap().total_pages),
            (1, 2)
        );

        let done: ApiResponse<BatchResultData> = serde_json::from_str(
            r#"{"code":0,"msg":"ok","data":{"batch_id":"b","extract_result":[
                {"file_name":"a.pdf","state":"done","err_msg":"",
                 "full_zip_url":"https://cdn.example/x.zip"}]}}"#,
        )
        .expect("done payload parses");
        let item = &done.into_result().expect("code is 0").extract_result[0];
        assert_eq!(item.state, "done");
        assert_eq!(
            item.full_zip_url.as_deref(),
            Some("https://cdn.example/x.zip")
        );
    }

    #[test]
    fn business_error_surfaces_code() {
        let payload: ApiResponse<BatchUploadData> =
            serde_json::from_str(r#"{"code":401,"msg":"token invalid"}"#)
                .expect("error payload parses");
        let error = payload.into_result().expect_err("non-zero code");
        assert!(error.to_string().contains("token invalid"));
    }

    #[test]
    fn table_separator_matches_first_row_width() {
        let rows = serde_json::json!([["a", "b", "c"], ["d", "e", "f"]]);
        let table = match &rows {
            serde_json::Value::Array(rows) => format_table_markdown(rows),
            _ => unreachable!(),
        };
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines[0], "| a | b | c |");
        assert_eq!(lines[1], "| --- | --- | --- |");
        assert_eq!(lines[2], "| d | e | f |");
    }
}
