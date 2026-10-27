//! Process boundary for Zotero's unmodified AGPL document-worker/SDT pipeline.
//! The JavaScript host and models live in `tools/zotero-extractor`, separately
//! licensed and pinned to upstream commits. PDF bytes travel over stdin; no
//! uploaded path or filename is interpreted as a command argument.
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use bib_types::{FileFormat, TextFormat};
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use crate::{Error, ExtractedText, Result, TextExtractor};

const MAX_OUTPUT: u64 = 128 * 1024 * 1024;
const MAX_DIAGNOSTICS: u64 = 256 * 1024;

#[derive(Default)]
pub struct ZoteroExtractor;

#[derive(Deserialize)]
struct Output {
    text: String,
    structure: Value,
}

impl ZoteroExtractor {
    fn entrypoint() -> PathBuf {
        if let Some(path) = std::env::var_os("ZOTERO_EXTRACTOR_PATH") {
            return PathBuf::from(path);
        }
        if let Ok(executable) = std::env::current_exe() {
            if let Some(parent) = executable.parent() {
                let packaged = parent.join("zotero-extractor/extract.mjs");
                if packaged.is_file() {
                    return packaged;
                }
            }
        }
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/zotero-extractor/extract.mjs")
    }
}

#[async_trait]
impl TextExtractor for ZoteroExtractor {
    fn name(&self) -> &'static str {
        "zotero-sdt"
    }

    async fn extract(&self, content: &[u8], format: FileFormat) -> Result<ExtractedText> {
        if format != FileFormat::Pdf {
            return Err(Error::Unknown("Zotero SDT extraction expects a PDF".into()));
        }
        let entrypoint = Self::entrypoint();
        let bundled_node = entrypoint.parent().map(|parent| parent.join("node"));
        let node = std::env::var_os("ZOTERO_NODE").unwrap_or_else(|| {
            bundled_node
                .filter(|path| path.is_file())
                .map(PathBuf::into_os_string)
                .unwrap_or_else(|| "node".into())
        });
        let mut child = Command::new(node)
            .arg(&entrypoint)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| Error::Unknown(format!(
                "cannot start Zotero extractor ({}): {error}; install Node >=22 and run npm ci in tools/zotero-extractor",
                entrypoint.display()
            )))?;
        let mut stdin = child.stdin.take().expect("piped stdin");
        let mut stdout = child
            .stdout
            .take()
            .expect("piped stdout")
            .take(MAX_OUTPUT + 1);
        let mut stderr = child
            .stderr
            .take()
            .expect("piped stderr")
            .take(MAX_DIAGNOSTICS + 1);
        let operation = async {
            let write = async move {
                stdin.write_all(content).await?;
                drop(stdin);
                std::io::Result::Ok(())
            };
            let read_out = async {
                let mut bytes = Vec::new();
                stdout.read_to_end(&mut bytes).await?;
                std::io::Result::Ok(bytes)
            };
            let read_err = async {
                let mut bytes = Vec::new();
                stderr.read_to_end(&mut bytes).await?;
                std::io::Result::Ok(bytes)
            };
            // Drain both output streams while writing so large documents cannot
            // deadlock on full pipes. join! retains diagnostics on early exits.
            tokio::join!(write, read_out, read_err, child.wait())
        };
        let (write, output, diagnostics, status) =
            tokio::time::timeout(Duration::from_secs(180), operation)
                .await
                .map_err(|_| {
                    Error::Unknown("Zotero extraction timed out after 180 seconds".into())
                })?;
        let output =
            output.map_err(|error| Error::Unknown(format!("reading Zotero output: {error}")))?;
        let diagnostics = diagnostics.unwrap_or_default();
        let status = status
            .map_err(|error| Error::Unknown(format!("waiting for Zotero extractor: {error}")))?;
        if !status.success() {
            let tail = diagnostics.len().saturating_sub(8192);
            return Err(Error::Unknown(format!(
                "Zotero extraction failed ({status}): {}",
                String::from_utf8_lossy(&diagnostics[tail..])
            )));
        }
        write.map_err(|error| Error::Unknown(format!("sending PDF to Zotero: {error}")))?;
        if output.len() as u64 > MAX_OUTPUT || diagnostics.len() as u64 > MAX_DIAGNOSTICS {
            return Err(Error::Unknown(
                "Zotero output exceeded the extraction limit".into(),
            ));
        }
        let result: Output = serde_json::from_slice(&output).map_err(|error| {
            Error::Unknown(format!("invalid Zotero extraction response: {error}"))
        })?;
        if result
            .structure
            .pointer("/metadata/processor/type")
            .and_then(Value::as_str)
            != Some("pdf")
            || !result.structure.get("content").is_some_and(Value::is_array)
            || !result
                .structure
                .pointer("/catalog/pages")
                .is_some_and(Value::is_array)
        {
            return Err(Error::Unknown(
                "Zotero returned an invalid SDT document".into(),
            ));
        }
        Ok(ExtractedText {
            text: result.text,
            format: TextFormat::Plain,
            extractor: self.name(),
            images: Vec::new(),
            structure: Some(result.structure),
        })
    }
}

/// Cache envelope binds the semantic document to the exact original file and
/// records the upstream versions needed to invalidate old derived objects.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct StructuredCache {
    pub file_path: String,
    pub file_hash: Option<String>,
    pub structure: Value,
}

impl StructuredCache {
    pub fn matches(&self, fulltext: &bib_types::FullText) -> bool {
        self.file_path == fulltext.file_path
            && self.file_hash == fulltext.file_hash
            && self.structure.get("schemaVersion").and_then(Value::as_str) == Some("1.2.0")
            && self.structure.get("content").is_some_and(Value::is_array)
            && self
                .structure
                .pointer("/catalog/pages")
                .is_some_and(Value::is_array)
            && self
                .structure
                .pointer("/metadata/processor/version")
                .and_then(Value::as_u64)
                == Some(15)
            && self
                .structure
                .pointer("/metadata/processor/type")
                .and_then(Value::as_str)
                == Some("pdf")
    }
}

pub async fn store_structure(
    storage: &vfs::OpendalFileStorage,
    article_id: &str,
    attachment_key: Option<&str>,
    fulltext: &bib_types::FullText,
    structure: Option<&Value>,
) -> std::result::Result<(), String> {
    let object = crate::stored_files::structured_object_path(article_id, attachment_key);
    let path = crate::stored_files::vfs_virtual_path(&object).expect("SDT path is VFS");
    if let Some(structure) = structure {
        let cache = StructuredCache {
            file_path: fulltext.file_path.clone(),
            file_hash: fulltext.file_hash.clone(),
            structure: structure.clone(),
        };
        let bytes = serde_json::to_vec(&cache).map_err(|error| error.to_string())?;
        storage
            .write_bytes(&path, bytes)
            .await
            .map_err(|error| format!("storing Zotero structure: {error}"))
    } else {
        // An OCR/cloud fallback must not leave a previous run's source map live.
        storage
            .delete_object(&path)
            .await
            .map_err(|error| format!("removing stale Zotero structure: {error}"))
    }
}

/// Read a validated cache or generate SDT on demand. Existing plaintext remains
/// available if the structured extractor fails. All callers share the normal
/// extraction semaphore; the gate also coalesces repeated Reading Mode opens.
pub async fn structured_fulltext(
    shared: &crate::BibShared,
    article_id: &str,
    attachment_key: Option<&str>,
    generate: bool,
) -> Result<Option<Value>> {
    static GENERATION: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    let _generation = if generate {
        Some(
            GENERATION
                .get_or_init(|| tokio::sync::Mutex::new(()))
                .lock()
                .await,
        )
    } else {
        None
    };
    let key = if let Some(key) = attachment_key {
        Some(key.to_owned())
    } else {
        shared
            .bib
            .zotero_attachment(article_id)
            .await?
            .map(|attachment| attachment.key)
    };
    let fulltext = match key.as_deref() {
        Some(key) => shared.bib.get_attachment_fulltext(article_id, key).await?,
        None => shared.bib.get_fulltext(article_id).await?,
    }
    .ok_or_else(|| Error::Unknown("PDF attachment was not found".into()))?;
    if fulltext.file_format != FileFormat::Pdf {
        return Err(Error::Unknown(
            "Reading Mode requires a PDF attachment".into(),
        ));
    }
    let storage = shared
        .file_storage
        .as_ref()
        .ok_or_else(|| Error::Unknown("bibliography VFS storage is not configured".into()))?;
    let object = crate::stored_files::structured_object_path(article_id, key.as_deref());
    let path = crate::stored_files::vfs_virtual_path(&object).expect("SDT VFS path");
    match crate::extraction::read_full(storage, &path).await {
        Ok(bytes) => {
            if let Ok(cache) = serde_json::from_slice::<StructuredCache>(&bytes) {
                if cache.matches(&fulltext) {
                    return Ok(Some(cache.structure));
                }
            }
        }
        Err(error) if error.kind() == opendal::ErrorKind::NotFound => {}
        Err(error) => return Err(Error::Unknown(format!("reading Zotero structure: {error}"))),
    }
    if !generate {
        return Ok(None);
    }
    let _permit = shared
        .extraction_permits
        .acquire()
        .await
        .map_err(|error| Error::Unknown(error.to_string()))?;
    let original = crate::stored_files::vfs_virtual_path(&fulltext.file_path)
        .ok_or_else(|| Error::Unknown("PDF original is not stored in the VFS".into()))?;
    let bytes = crate::extraction::read_full(storage, &original)
        .await
        .map_err(|error| Error::Unknown(format!("reading PDF original: {error}")))?;
    let extracted = ZoteroExtractor.extract(&bytes, FileFormat::Pdf).await?;
    // A replacement during extraction must not receive the old PDF's source map.
    let current = match key.as_deref() {
        Some(key) => shared.bib.get_attachment_fulltext(article_id, key).await?,
        None => shared.bib.get_fulltext(article_id).await?,
    }
    .ok_or_else(|| Error::Unknown("PDF attachment was removed during extraction".into()))?;
    if current.file_hash != fulltext.file_hash || current.file_path != fulltext.file_path {
        return Err(Error::Unknown(
            "PDF changed during extraction; reopen Reading Mode".into(),
        ));
    }
    store_structure(
        storage,
        article_id,
        key.as_deref(),
        &fulltext,
        extracted.structure.as_ref(),
    )
    .await
    .map_err(Error::Unknown)?;
    // Upgrade the old flattened fulltext projection without clearing it first.
    // A background extraction already in flight owns its own status transition.
    if current.extract_status != Some(bib_types::ExtractStatus::Running)
        && !extracted.text.trim().is_empty()
    {
        let object = if let Some(key) = key.as_deref() {
            crate::stored_files::extracted_attachment_object_path(article_id, key, extracted.format)
        } else {
            crate::stored_files::extracted_object_path(article_id, extracted.format)
        };
        let path = crate::stored_files::vfs_virtual_path(&object).expect("extracted VFS path");
        storage
            .write_bytes(&path, extracted.text.clone().into_bytes())
            .await
            .map_err(|error| Error::Unknown(format!("storing Zotero fulltext: {error}")))?;
        if let Some(key) = key.as_deref() {
            shared
                .bib
                .record_attachment_extraction_success(
                    key,
                    &extracted.text,
                    extracted.format,
                    extracted.extractor,
                    &object,
                )
                .await?;
        } else {
            shared
                .bib
                .record_extraction_success(
                    article_id,
                    &extracted.text,
                    extracted.format,
                    extracted.extractor,
                )
                .await?;
        }
    }
    Ok(extracted.structure)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires Node >=22 and npm ci in tools/zotero-extractor"]
    async fn zotero_process_preserves_paragraphs_and_pdf_anchors() {
        let bytes = include_bytes!(
            "../../../tools/zotero-extractor/test/fixtures/synthetic-list-links.pdf"
        );
        let result = ZoteroExtractor
            .extract(bytes, FileFormat::Pdf)
            .await
            .unwrap();
        assert_eq!(result.extractor, "zotero-sdt");
        assert!(
            result
                .text
                .contains("Synthetic reference links\n\nThe invented sources")
        );
        let structure = result.structure.unwrap();
        assert_eq!(structure["schemaVersion"], "1.2.0");
        assert_eq!(structure["metadata"]["processor"]["version"], 15);
        assert!(structure["content"][1]["anchor"]["pageRects"].is_array());
        assert!(structure["content"][1]["content"][0]["anchor"]["textMap"].is_string());
    }

    #[tokio::test]
    #[ignore = "requires Node >=22 and npm ci in tools/zotero-extractor"]
    async fn zotero_process_reports_malformed_pdf_without_hanging_on_stdin() {
        let error = ZoteroExtractor
            .extract(b"not a PDF", FileFormat::Pdf)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Zotero extraction failed"));
    }
}
