//! MinerU cloud PDF extraction (mineru.net API v4).
//!
//! [`MineruExtractor`] is a [`TextExtractor`] that uploads PDFs to the
//! MinerU cloud service and returns the layout-aware markdown it produces
//! (`full.md` from the result zip). Configuration comes from the
//! environment — `MINERU_API_URL` (default `https://mineru.net`) and
//! `MINERU_API_KEY` — the same variable names as jayread, so an existing
//! key works unchanged.
//!
//! ## Wire protocol (v4 "extract by file upload")
//!
//! 1. `POST {base}/api/v4/file-urls/batch` (Bearer) with the file names →
//!    `batch_id` + presigned upload URL.
//! 2. `PUT` the PDF bytes to the presigned URL — **without** an
//!    Authorization header (the signature already scopes the request).
//! 3. Poll `GET {base}/api/v4/extract-results/batch/{batch_id}` every 5 s
//!    until the state is `done` (→ `full_zip_url`) or `failed` (→ `err_msg`).
//! 4. Download the zip and read `{name}/full.md`.
//!
//! The extractor is used as the *primary* stage of a fallback chain: any
//! error (no key, quota exhausted, network) simply falls through to the
//! local `pdf-extract`/tesseract chain.

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use bib_types::FileFormat;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::extract::{ExtractedText, TextExtractor};

/// Default MinerU cloud endpoint when `MINERU_API_URL` is unset.
pub const DEFAULT_API_URL: &str = "https://mineru.net";

// Per-request timeouts (the shared HTTP client's 60 s total timeout would
// cut off every long-stage call, so each request overrides it).
const SUBMIT_TIMEOUT: Duration = Duration::from_secs(300);
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(600);
const POLL_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Poll at most 120 × 5 s ≈ 10 min, matching the overall deadline.
const MAX_POLL_ATTEMPTS: usize = 120;
const OVERALL_DEADLINE: Duration = Duration::from_secs(600);

// --- MinerU v4 API wire types (private) -------------------------------------

#[derive(Serialize)]
struct BatchUploadRequest {
    files: Vec<BatchUploadFile>,
    model_version: String,
}

#[derive(Serialize)]
struct BatchUploadFile {
    name: String,
}

#[derive(Deserialize)]
struct ApiResponse<T> {
    code: i64,
    msg: String,
    #[serde(default)]
    data: Option<T>,
}

#[derive(Default, Deserialize)]
struct BatchUploadData {
    batch_id: String,
    file_urls: Vec<String>,
}

#[derive(Default, Deserialize)]
struct BatchResultData {
    #[serde(default)]
    extract_result: Vec<ExtractResultItem>,
}

#[derive(Deserialize)]
struct ExtractResultItem {
    state: String,
    #[serde(default)]
    full_zip_url: Option<String>,
    #[serde(default)]
    err_msg: Option<String>,
}

// --- Extractor ---------------------------------------------------------------

/// Cloud [`TextExtractor`] backed by the MinerU v4 API.
pub struct MineruExtractor {
    client: reqwest::Client,
    api_url: String,
    api_key: Option<String>,
    /// Ensures the "key missing" warning is logged once, not per upload.
    warned_missing_key: AtomicBool,
}

impl MineruExtractor {
    /// Build from `MINERU_API_URL` / `MINERU_API_KEY` environment variables.
    ///
    /// Missing key is not an error here — the extractor just reports
    /// `Err` on every call so the fallback chain takes over.
    pub fn from_env(client: reqwest::Client) -> Self {
        Self {
            client,
            api_url: std::env::var("MINERU_API_URL")
                .ok()
                .filter(|url| !url.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_API_URL.to_owned()),
            api_key: std::env::var("MINERU_API_KEY")
                .ok()
                .filter(|key| !key.trim().is_empty()),
            warned_missing_key: AtomicBool::new(false),
        }
    }

    /// Run the v4 upload → poll → download flow and return the markdown.
    async fn extract_markdown(&self, pdf_bytes: Vec<u8>, api_key: &str) -> Result<String> {
        let deadline = tokio::time::Instant::now() + OVERALL_DEADLINE;
        let zip_url = self.submit_and_poll(pdf_bytes, api_key, deadline).await?;
        let zip_bytes = self.download_zip(&zip_url).await?;
        markdown_from_zip(&zip_bytes)
    }

    /// Steps 1–3: acquire a presigned URL, upload the bytes, poll to
    /// completion. Returns the result-zip download URL.
    async fn submit_and_poll(
        &self,
        pdf_bytes: Vec<u8>,
        api_key: &str,
        deadline: tokio::time::Instant,
    ) -> Result<String> {
        // Step 1: request a presigned upload URL for one "upload.pdf".
        let batch_url = format!("{}/api/v4/file-urls/batch", self.api_url);
        let request = BatchUploadRequest {
            // Fixed name: the result zip nests entries as upload/full.md,
            // which markdown_from_zip matches by suffix.
            files: vec![BatchUploadFile {
                name: "upload.pdf".to_owned(),
            }],
            model_version: "vlm".to_owned(),
        };
        let response = self
            .client
            .post(&batch_url)
            .timeout(SUBMIT_TIMEOUT)
            .header("Authorization", format!("Bearer {api_key}"))
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Unknown(format!("MinerU URL request failed: {e}")))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Unknown(format!(
                "MinerU URL request failed (HTTP {status}): {body}"
            )));
        }
        let api_response: ApiResponse<BatchUploadData> = response
            .json()
            .await
            .map_err(|e| Error::Unknown(format!("MinerU URL response unparsable: {e}")))?;
        if api_response.code != 0 {
            return Err(Error::Unknown(format!(
                "MinerU URL request rejected ({}): {}",
                api_response.code, api_response.msg
            )));
        }
        let data = api_response
            .data
            .ok_or_else(|| Error::Unknown("MinerU response missing data".into()))?;
        let upload_url = data
            .file_urls
            .into_iter()
            .next()
            .ok_or_else(|| Error::Unknown("MinerU response missing file_urls".into()))?;

        // Step 2: PUT the bytes. No Authorization header — the presigned
        // URL signature is bound to the method and expires on its own.
        let put_response = self
            .client
            .put(&upload_url)
            .timeout(UPLOAD_TIMEOUT)
            .body(pdf_bytes)
            .send()
            .await
            .map_err(|e| Error::Unknown(format!("MinerU PDF upload failed: {e}")))?;
        if !put_response.status().is_success() {
            let status = put_response.status();
            let body = put_response.text().await.unwrap_or_default();
            return Err(Error::Unknown(format!(
                "MinerU PDF upload failed (HTTP {status}): {body}"
            )));
        }

        // Step 3: poll every 5 s until done/failed.
        let result_url = format!(
            "{}/api/v4/extract-results/batch/{}",
            self.api_url, data.batch_id
        );
        let mut attempts = 0;
        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Unknown(
                    "MinerU extraction timed out (10 min overall)".into(),
                ));
            }
            tokio::time::sleep(POLL_INTERVAL).await;

            let response = self
                .client
                .get(&result_url)
                .timeout(POLL_TIMEOUT)
                .header("Authorization", format!("Bearer {api_key}"))
                .send()
                .await
                .map_err(|e| Error::Unknown(format!("MinerU poll failed: {e}")))?;
            if !response.status().is_success() {
                return Err(Error::Unknown(format!(
                    "MinerU poll failed (HTTP {})",
                    response.status()
                )));
            }
            let api_response: ApiResponse<BatchResultData> = response
                .json()
                .await
                .map_err(|e| Error::Unknown(format!("MinerU poll response unparsable: {e}")))?;
            if api_response.code != 0 {
                return Err(Error::Unknown(format!(
                    "MinerU poll rejected ({}): {}",
                    api_response.code, api_response.msg
                )));
            }
            let item = api_response
                .data
                .and_then(|data| data.extract_result.into_iter().next())
                .ok_or_else(|| {
                    Error::Unknown("MinerU poll response missing extract_result".into())
                })?;

            match item.state.as_str() {
                "done" => {
                    return item
                        .full_zip_url
                        .ok_or_else(|| Error::Unknown("MinerU done without full_zip_url".into()));
                }
                "failed" => {
                    return Err(Error::Unknown(format!(
                        "MinerU extraction failed: {}",
                        item.err_msg.unwrap_or_default()
                    )));
                }
                "waiting-file" | "pending" | "running" | "converting" => {
                    attempts += 1;
                    if attempts >= MAX_POLL_ATTEMPTS {
                        return Err(Error::Unknown(
                            "MinerU extraction timed out while polling".into(),
                        ));
                    }
                }
                unknown => {
                    return Err(Error::Unknown(format!(
                        "MinerU reported unknown state: {unknown}"
                    )));
                }
            }
        }
    }

    /// Step 4: download the result zip bytes.
    async fn download_zip(&self, zip_url: &str) -> Result<Vec<u8>> {
        let response = self
            .client
            .get(zip_url)
            .timeout(DOWNLOAD_TIMEOUT)
            .send()
            .await
            .map_err(|e| Error::Unknown(format!("MinerU zip download failed: {e}")))?;
        if !response.status().is_success() {
            return Err(Error::Unknown(format!(
                "MinerU zip download failed (HTTP {})",
                response.status()
            )));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| Error::Unknown(format!("MinerU zip read failed: {e}")))?;
        Ok(bytes.to_vec())
    }
}

#[async_trait]
impl TextExtractor for MineruExtractor {
    fn name(&self) -> &'static str {
        "mineru"
    }

    async fn extract(&self, content: &[u8], format: FileFormat) -> Result<ExtractedText> {
        if format != FileFormat::Pdf {
            return Err(Error::Unknown(
                "MinerU only extracts PDFs; other formats go to the local chain".into(),
            ));
        }
        let Some(api_key) = self.api_key.as_deref() else {
            if !self.warned_missing_key.swap(true, Ordering::Relaxed) {
                tracing::warn!(
                    "MINERU_API_KEY is not set — cloud extraction unavailable, using the local chain"
                );
            }
            return Err(Error::Unknown("MINERU_API_KEY is not set".into()));
        };
        let markdown = self.extract_markdown(content.to_vec(), api_key).await?;
        Ok(ExtractedText::markdown(markdown))
    }
}

/// Pull `full.md` out of a MinerU result zip.
///
/// The zip nests entries under the upload name (`upload/full.md`); older
/// downloads may use a bare `full.md`. Everything else (images, layout and
/// middle JSON) is ignored — the bibliography pipeline only consumes
/// markdown. A missing `full.md` fails with the entry list so operators
/// can see what the service actually returned.
pub fn markdown_from_zip(zip_bytes: &[u8]) -> Result<String> {
    let reader = std::io::Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(reader)
        .map_err(|e| Error::Unknown(format!("MinerU result zip unreadable: {e}")))?;

    let mut all_names = Vec::with_capacity(archive.len());
    let mut markdown: Option<String> = None;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| Error::Unknown(format!("MinerU zip entry unreadable: {e}")))?;
        let name = entry.name().to_owned();
        all_names.push(name.clone());
        if name.ends_with("/full.md") || name == "full.md" {
            let mut text = String::new();
            entry
                .read_to_string(&mut text)
                .map_err(|e| Error::Unknown(format!("MinerU full.md unreadable: {e}")))?;
            markdown = Some(text);
        }
    }

    markdown.ok_or_else(|| {
        Error::Unknown(format!(
            "MinerU result zip has no full.md (entries: {all_names:?})"
        ))
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn build_test_zip(entries: &[(&str, &str)]) -> Vec<u8> {
        let cursor = std::io::Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(cursor);
        let options: zip::write::SimpleFileOptions = Default::default();
        for (name, content) in entries {
            zip.start_file(*name, options).expect("start entry");
            zip.write_all(content.as_bytes()).expect("write entry");
        }
        let cursor = zip.finish().expect("finish zip");
        cursor.into_inner()
    }

    #[test]
    fn markdown_from_zip_with_directory_prefix() {
        let bytes = build_test_zip(&[
            ("upload/layout.json", "{}"),
            ("upload/full.md", "# Title\n\nbody"),
            ("upload/images/fig1.jpg", "\u{ff}\u{d8}fake"),
        ]);
        assert_eq!(markdown_from_zip(&bytes).unwrap(), "# Title\n\nbody");
    }

    #[test]
    fn markdown_from_zip_with_bare_name() {
        let bytes = build_test_zip(&[("full.md", "bare")]);
        assert_eq!(markdown_from_zip(&bytes).unwrap(), "bare");
    }

    #[test]
    fn markdown_from_zip_missing_reports_entries() {
        let bytes = build_test_zip(&[("upload/middle.json", "{}")]);
        let error = markdown_from_zip(&bytes).unwrap_err().to_string();
        assert!(error.contains("no full.md"), "error was: {error}");
        assert!(error.contains("upload/middle.json"), "error was: {error}");
    }

    #[tokio::test]
    async fn non_pdf_rejected_without_key_check() {
        let extractor = MineruExtractor {
            client: reqwest::Client::new(),
            api_url: DEFAULT_API_URL.to_owned(),
            api_key: None,
            warned_missing_key: AtomicBool::new(false),
        };
        let error = extractor.extract(b"hi", FileFormat::Txt).await.unwrap_err();
        assert!(error.to_string().contains("PDF"));
    }

    #[tokio::test]
    async fn missing_key_errors() {
        // Temporarily clear the env var if inherited from the test runner.
        let previous = std::env::var("MINERU_API_KEY").ok();
        unsafe { std::env::remove_var("MINERU_API_KEY") };
        let extractor = MineruExtractor::from_env(reqwest::Client::new());
        let result = extractor.extract(b"%PDF-1.7", FileFormat::Pdf).await;
        if let Some(key) = previous {
            unsafe { std::env::set_var("MINERU_API_KEY", key) };
        }
        assert!(result.is_err());
    }
}
