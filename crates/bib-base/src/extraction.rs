//! Background full-text extraction orchestration.
//!
//! Uploads only store the original file and mark the row `pending`; this
//! module owns everything that happens afterwards:
//!
//! - [`FallbackChainExtractor`] — Zotero SDT first, then MinerU cloud.
//!   Zotero preserves semantic paragraphs and source maps. HTML/TXT use the
//!   built-in non-PDF extractor.
//! - [`run_extraction_parts`] / [`run_extraction`] — claim a pending row
//!   (`pending → running` CAS), read the file back from the VFS, extract,
//!   and record success/failure. Usable synchronously (CLI backfill,
//!   tests).
//! - [`spawn_extraction`] — the background variant: a detached task that
//!   first acquires a slot from the shared [`Semaphore`], keeping cloud
//!   quota and local CPU in check.
//! - [`sweep_pending`] — startup recovery: reset orphaned `running` rows
//!   and re-spawn everything still unfinished.

use std::sync::Arc;

use tokio::sync::Semaphore;

use crate::bib_base::BibBase;
use crate::error::{Error, Result};
use crate::extract::{ExtractedText, SimpleExtractor, TextExtractor};
use crate::mineru::MineruExtractor;
use crate::shared::BibShared;
use crate::stored_files::{
    extracted_attachment_object_path, extracted_object_path, figure_object_path, vfs_virtual_path,
};
use crate::zotero_extract::ZoteroExtractor;
use bib_types::FileFormat;

/// How many extractions may run concurrently.
///
/// Uploads are human-frequency, so this exists to protect the MinerU free
/// quota (~2000 pages/day) and local CPU from burst sweeps, not to manage
/// sustained load.
pub(crate) const MAX_CONCURRENT_EXTRACTIONS: usize = 2;

// ---------------------------------------------------------------------------
// Fallback chain
// ---------------------------------------------------------------------------

/// Try `primary`, and when it errors or yields no text, try `fallback`.
///
/// Non-PDF input uses [`SimpleExtractor`]; neither PDF backend is called.
/// When both PDF stages fail, extraction fails with the fallback error.
pub struct FallbackChainExtractor {
    primary: Arc<dyn TextExtractor>,
    fallback: Arc<dyn TextExtractor>,
}

impl FallbackChainExtractor {
    pub fn new(primary: Arc<dyn TextExtractor>, fallback: Arc<dyn TextExtractor>) -> Self {
        Self { primary, fallback }
    }
}

#[async_trait::async_trait]
impl TextExtractor for FallbackChainExtractor {
    fn name(&self) -> &'static str {
        "chain"
    }

    async fn extract(&self, content: &[u8], format: FileFormat) -> Result<ExtractedText> {
        if format != FileFormat::Pdf {
            return SimpleExtractor::new().extract(content, format).await;
        }

        match self.primary.extract(content, format).await {
            Ok(text) if !text.text.trim().is_empty() => Ok(text),
            primary_outcome => {
                match &primary_outcome {
                    Err(error) => tracing::warn!(
                        extractor = self.primary.name(),
                        error = %error,
                        "primary extractor failed; trying fallback"
                    ),
                    Ok(_) => tracing::warn!(
                        extractor = self.primary.name(),
                        "primary extractor produced no text; trying fallback"
                    ),
                }
                match self.fallback.extract(content, format).await {
                    Ok(text) if !text.text.trim().is_empty() => Ok(text),
                    fallback_outcome => {
                        let detail = match fallback_outcome {
                            Err(error) => error.to_string(),
                            Ok(_) => "extractors produced no text".to_owned(),
                        };
                        Err(Error::Unknown(format!("extraction failed: {detail}")))
                    }
                }
            }
        }
    }
}

/// The process-wide PDF extraction chain: Zotero SDT → MinerU cloud.
///
/// Uses the bundled Zotero document-worker first. Reads `MINERU_API_URL` / `MINERU_API_KEY` from the environment; without
/// a key, PDFs require a successful Zotero SDT extraction.
pub fn default_extractor(http: reqwest::Client) -> Arc<dyn TextExtractor> {
    Arc::new(FallbackChainExtractor::new(
        Arc::new(ZoteroExtractor),
        Arc::new(MineruExtractor::from_env(http)),
    ))
}

// ---------------------------------------------------------------------------
// Orchestration
// ---------------------------------------------------------------------------

/// Read a whole VFS object into memory.
///
/// The storage layer only exposes ranged reads; extraction wants the full
/// file, so stat the length and read `0..len`.
pub async fn read_full(
    storage: &vfs::OpendalFileStorage,
    virtual_path: &str,
) -> std::result::Result<Vec<u8>, opendal::Error> {
    let length = storage.content_length(virtual_path).await?;
    let buffer = storage.read_range(virtual_path, 0..length).await?;
    Ok(buffer.to_vec())
}

/// Claim and run one extraction against explicit parts.
///
/// This is the synchronous core shared by the background spawner, the CLI
/// backfill, and tests. The `pending → running` CAS in step 1 is the
/// dedup boundary: concurrent callers for the same article collapse to
/// one winner, the losers return early with `Err` and change nothing.
///
/// Every terminal path records its outcome in the database, so callers
/// only need the returned [`ExtractedText`] / message for reporting.
pub async fn run_extraction_parts(
    bib: &BibBase,
    storage: &vfs::OpendalFileStorage,
    extractor: &dyn TextExtractor,
    article_id: &str,
) -> std::result::Result<ExtractedText, String> {
    if let Some(attachment) = bib
        .zotero_attachment(article_id)
        .await
        .map_err(|error| format!("failed to resolve preferred attachment: {error}"))?
    {
        return run_attachment_extraction_parts(
            bib,
            storage,
            extractor,
            article_id,
            &attachment.key,
        )
        .await;
    }

    let claimed = bib
        .mark_extraction_running(article_id)
        .await
        .map_err(|error| format!("failed to claim extraction: {error}"))?;
    if !claimed {
        return Err(format!("{article_id} is already being extracted elsewhere"));
    }

    let fulltext = bib
        .get_fulltext(article_id)
        .await
        .map_err(|error| format!("failed to load full-text row: {error}"))?;
    let Some(fulltext) = fulltext else {
        record_failure(
            bib,
            article_id,
            "full-text row disappeared before extraction",
        )
        .await;
        return Err("full-text row disappeared before extraction".to_owned());
    };

    let Some(virtual_path) = vfs_virtual_path(&fulltext.file_path) else {
        let message = format!(
            "original file is not VFS-stored ({}); re-upload it to retry",
            fulltext.file_path
        );
        record_failure(bib, article_id, &message).await;
        return Err(message);
    };

    let content = match read_full(storage, &virtual_path).await {
        Ok(content) => content,
        Err(error) => {
            let message = format!("failed to read original file: {error}");
            record_failure(bib, article_id, &message).await;
            return Err(message);
        }
    };

    match extractor.extract(&content, fulltext.file_format).await {
        Ok(mut extracted) => {
            if let Err(message) = crate::zotero_extract::store_structure(
                storage,
                article_id,
                None,
                &fulltext,
                extracted.structure.as_ref(),
            )
            .await
            {
                record_failure(bib, article_id, &message).await;
                return Err(message);
            }
            // Persist figure images before recording success: the markdown
            // is the product, so an image write failure only downgrades to
            // a warning instead of failing the extraction. Stale figures
            // from a previous run are NOT cleaned here — every claimable
            // row has `text_content = NULL` (`restart_extraction` clears
            // it), so old-vs-new diffing happens at the restart/replace/
            // delete entry points in api-server instead.
            store_figures(storage, article_id, &mut extracted).await;

            // The extracted text is the primary product: write it to its
            // canonical VFS object (`extracted/fulltext.{md|txt}`) before
            // recording success. Unlike figures this is fail-closed — a
            // row marked `done` without a readable file would silently
            // break the agent's `fulltext_path`.
            let extracted_path = extracted_object_path(article_id, extracted.format);
            let extracted_virtual =
                vfs_virtual_path(&extracted_path).expect("canonical extracted path is VFS");
            if let Err(error) = storage
                .write_bytes(&extracted_virtual, extracted.text.clone().into_bytes())
                .await
            {
                let message = format!("failed to store extracted text object: {error}");
                record_failure(bib, article_id, &message).await;
                return Err(message);
            }

            bib.record_extraction_success(
                article_id,
                &extracted.text,
                extracted.format,
                extracted.extractor,
            )
            .await
            .map_err(|error| format!("failed to store extracted text: {error}"))?;
            tracing::info!(
                article_id,
                extractor = extracted.extractor,
                chars = extracted.text.chars().count(),
                figures = extracted.images.len(),
                "full-text extraction finished"
            );
            Ok(extracted)
        }
        Err(error) => {
            let message = error.to_string();
            record_failure(bib, article_id, &message).await;
            Err(message)
        }
    }
}

/// Claim and run extraction for one Zotero attachment item key.
pub async fn run_attachment_extraction_parts(
    bib: &BibBase,
    storage: &vfs::OpendalFileStorage,
    extractor: &dyn TextExtractor,
    article_id: &str,
    attachment_key: &str,
) -> std::result::Result<ExtractedText, String> {
    let claimed = bib
        .mark_attachment_extraction_running(attachment_key)
        .await
        .map_err(|error| format!("failed to claim attachment extraction: {error}"))?;
    if !claimed {
        return Err(format!(
            "attachment {attachment_key} is already being extracted elsewhere"
        ));
    }

    let fulltext = bib
        .get_attachment_fulltext(article_id, attachment_key)
        .await
        .map_err(|error| format!("failed to load attachment full text: {error}"))?;
    let Some(fulltext) = fulltext else {
        record_attachment_failure(
            bib,
            attachment_key,
            "attachment full-text row disappeared before extraction",
        )
        .await;
        return Err("attachment full-text row disappeared before extraction".to_owned());
    };
    let Some(virtual_path) = vfs_virtual_path(&fulltext.file_path) else {
        let message = format!(
            "attachment original is not VFS-stored ({}); re-upload it to retry",
            fulltext.file_path
        );
        record_attachment_failure(bib, attachment_key, &message).await;
        return Err(message);
    };
    let content = match read_full(storage, &virtual_path).await {
        Ok(content) => content,
        Err(error) => {
            let message = format!("failed to read attachment original: {error}");
            record_attachment_failure(bib, attachment_key, &message).await;
            return Err(message);
        }
    };

    match extractor.extract(&content, fulltext.file_format).await {
        Ok(mut extracted) => {
            if let Err(message) = crate::zotero_extract::store_structure(
                storage,
                article_id,
                Some(attachment_key),
                &fulltext,
                extracted.structure.as_ref(),
            )
            .await
            {
                record_attachment_failure(bib, attachment_key, &message).await;
                return Err(message);
            }
            store_attachment_figures(storage, article_id, attachment_key, &mut extracted).await;
            let extracted_path =
                extracted_attachment_object_path(article_id, attachment_key, extracted.format);
            let extracted_virtual =
                vfs_virtual_path(&extracted_path).expect("attachment extracted path is VFS");
            if let Err(error) = storage
                .write_bytes(&extracted_virtual, extracted.text.clone().into_bytes())
                .await
            {
                let message = format!("failed to store attachment extracted text: {error}");
                record_attachment_failure(bib, attachment_key, &message).await;
                return Err(message);
            }

            bib.record_attachment_extraction_success(
                attachment_key,
                &extracted.text,
                extracted.format,
                extracted.extractor,
                &extracted_path,
            )
            .await
            .map_err(|error| format!("failed to store attachment extracted text: {error}"))?;
            tracing::info!(
                article_id,
                attachment_key,
                extractor = extracted.extractor,
                chars = extracted.text.chars().count(),
                figures = extracted.images.len(),
                "attachment full-text extraction finished"
            );
            Ok(extracted)
        }
        Err(error) => {
            let message = error.to_string();
            record_attachment_failure(bib, attachment_key, &message).await;
            Err(message)
        }
    }
}

/// Write extracted figure images into the VFS under their stable names.
///
/// Best-effort by design: the markdown text is the primary product, so a
/// failed image write downgrades to a warning instead of failing the
/// extraction. Image buffers are consumed as they are written to keep the
/// peak memory of a figure-heavy paper bounded.
async fn store_figures(
    storage: &vfs::OpendalFileStorage,
    article_id: &str,
    extracted: &mut ExtractedText,
) {
    for image in &mut extracted.images {
        let object = figure_object_path(article_id, &image.name);
        let Some(virtual_path) = vfs_virtual_path(&object) else {
            tracing::warn!(article_id, name = %image.name, "figure path is not VFS-addressable");
            continue;
        };
        let data = std::mem::take(&mut image.data);
        if let Err(error) = storage.write_bytes(&virtual_path, data).await {
            tracing::warn!(
                article_id,
                name = %image.name,
                error = %error,
                "failed to store figure image"
            );
        }
    }
}

async fn store_attachment_figures(
    storage: &vfs::OpendalFileStorage,
    article_id: &str,
    attachment_key: &str,
    extracted: &mut ExtractedText,
) {
    for image in &mut extracted.images {
        let object = crate::stored_files::attachment_figure_object_path(
            article_id,
            attachment_key,
            &image.name,
        );
        let Some(virtual_path) = vfs_virtual_path(&object) else {
            tracing::warn!(
                article_id,
                attachment_key,
                name = %image.name,
                "attachment figure path is not VFS-addressable"
            );
            continue;
        };
        let data = std::mem::take(&mut image.data);
        if let Err(error) = storage.write_bytes(&virtual_path, data).await {
            tracing::warn!(
                article_id,
                attachment_key,
                name = %image.name,
                error = %error,
                "failed to store attachment figure image"
            );
        }
    }
}

async fn record_failure(bib: &BibBase, article_id: &str, message: &str) {
    if let Err(error) = bib.record_extraction_failure(article_id, message).await {
        tracing::warn!(article_id, error = %error, "failed to record extraction failure");
    }
}

async fn record_attachment_failure(bib: &BibBase, attachment_key: &str, message: &str) {
    if let Err(error) = bib
        .record_attachment_extraction_failure(attachment_key, message)
        .await
    {
        tracing::warn!(
            attachment_key,
            error = %error,
            "failed to record attachment extraction failure"
        );
    }
}

/// Run one extraction using the shared bundle's storage and chain.
pub async fn run_extraction(
    shared: &BibShared,
    article_id: &str,
) -> std::result::Result<ExtractedText, String> {
    let Some(storage) = shared.file_storage.as_ref() else {
        let message = "VFS storage is not configured; cannot extract".to_owned();
        record_failure(&shared.bib, article_id, &message).await;
        return Err(message);
    };
    run_extraction_parts(&shared.bib, storage, shared.extractor.as_ref(), article_id).await
}

/// Spawn a background extraction task for one article.
///
/// The task waits for a semaphore slot first (see
/// [`MAX_CONCURRENT_EXTRACTIONS`]), so a burst of uploads or a large
/// startup sweep queues instead of hammering the cloud API. The returned
/// handle may be dropped.
pub fn spawn_extraction(shared: BibShared, article_id: String) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let permit = match shared.extraction_permits.acquire().await {
            Ok(permit) => permit,
            Err(_) => {
                tracing::warn!(article_id, "extraction semaphore closed; task aborted");
                return;
            }
        };
        let outcome = run_extraction(&shared, &article_id).await;
        drop(permit);
        if let Err(message) = outcome {
            tracing::warn!(
                article_id,
                message,
                "background extraction did not complete"
            );
        }
    })
}

/// Spawn extraction for a specific attachment item key.
pub fn spawn_attachment_extraction(
    shared: BibShared,
    article_id: String,
    attachment_key: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let permit = match shared.extraction_permits.acquire().await {
            Ok(permit) => permit,
            Err(_) => {
                tracing::warn!(attachment_key, "extraction semaphore closed; task aborted");
                return;
            }
        };
        let outcome = run_attachment_extraction(&shared, &article_id, &attachment_key).await;
        drop(permit);
        if let Err(message) = outcome {
            tracing::warn!(
                attachment_key,
                message,
                "background attachment extraction did not complete"
            );
        }
    })
}

/// Run one attachment extraction with the shared storage and extractor.
pub async fn run_attachment_extraction(
    shared: &BibShared,
    article_id: &str,
    attachment_key: &str,
) -> std::result::Result<ExtractedText, String> {
    let Some(storage) = shared.file_storage.as_ref() else {
        let message = "VFS storage is not configured; cannot extract attachment".to_owned();
        record_attachment_failure(&shared.bib, attachment_key, &message).await;
        return Err(message);
    };
    run_attachment_extraction_parts(
        &shared.bib,
        storage,
        shared.extractor.as_ref(),
        article_id,
        attachment_key,
    )
    .await
}

/// Startup recovery: return orphaned `running` rows to `pending`, spawn
/// tasks for everything still unfinished (`pending` + `failed` with a
/// VFS-stored original), and materialize extracted-text objects for any
/// `done` rows written before the canonical `extracted/` path existed.
pub async fn sweep_pending(shared: &BibShared) {
    if let Err(error) = shared.bib.reset_stale_running().await {
        tracing::warn!(error = %error, "failed to reset stale running extractions");
    }
    if let Err(error) = shared.bib.reset_stale_attachment_running().await {
        tracing::warn!(
            error = %error,
            "failed to reset stale attachment running extractions"
        );
    }
    backfill_extracted_objects(shared).await;
    let pending = match shared.bib.list_attachments_needing_extraction().await {
        Ok(pending) => pending,
        Err(error) => {
            tracing::warn!(error = %error, "failed to list attachments needing extraction");
            return;
        }
    };
    if !pending.is_empty() {
        tracing::info!(
            count = pending.len(),
            "resuming unfinished full-text extractions"
        );
    }
    for item in pending {
        spawn_attachment_extraction(shared.clone(), item.article_id, item.attachment_key);
    }
}

/// One-time-per-startup migration: write the canonical extracted-text
/// object for `done` rows that predate file-backed extraction.
///
/// The row's inline `text_content` is materialized to
/// `extracted/fulltext.{md|txt}` when that object does not exist yet.
/// Idempotent — existing objects are left untouched (re-extraction
/// overwrites them in place) — and best-effort: a write failure warns
/// and skips, the row stays valid for the DB-backed read paths.
pub async fn backfill_extracted_objects(shared: &BibShared) {
    let Some(storage) = shared.file_storage.as_ref() else {
        return;
    };
    let rows = match shared.bib.list_extracted_for_backfill().await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(error = %error, "failed to list extracted texts for backfill");
            return;
        }
    };
    for item in rows {
        let path =
            extracted_attachment_object_path(&item.article_id, &item.attachment_key, item.format);
        let Some(virtual_path) = vfs_virtual_path(&path) else {
            continue;
        };
        if storage.content_length(&virtual_path).await.is_ok() {
            continue; // already file-backed
        }
        if let Err(error) = storage
            .write_bytes(&virtual_path, item.text.into_bytes())
            .await
        {
            tracing::warn!(
                article_id = %item.article_id,
                attachment_key = %item.attachment_key,
                path = %path,
                error = %error,
                "failed to backfill extracted text object"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bib_base::BibBase;
    use crate::extract::ExtractedImage;
    use crate::fulltext::PendingExtraction;
    use crate::shared::BibShared;
    use crate::stored_files::stored_fulltext;
    use bib_types::{
        Article, ExtractStatus, FullText, FullTextSource, TextFormat as BibTextFormat,
    };

    async fn shared_with_temp_storage() -> BibShared {
        let mut shared = BibShared::open_in_memory().await.unwrap();
        shared = shared.with_file_storage(Arc::new(vfs::OpendalFileStorage::new_temp()));
        shared
    }

    async fn upload_pending(shared: &BibShared, article_id: &str, content: &[u8], filename: &str) {
        shared
            .bib
            .upsert_article(&Article::new(article_id, "Extraction e2e"))
            .await
            .unwrap();
        let stored = stored_fulltext(article_id, filename, content);
        let virtual_path = vfs_virtual_path(&stored.path).unwrap();
        shared
            .file_storage
            .as_ref()
            .unwrap()
            .write_bytes(&virtual_path, content.to_vec())
            .await
            .unwrap();
        let format = FileFormat::from_extension(
            filename
                .rsplit_once('.')
                .map(|(_, ext)| ext)
                .unwrap_or("txt"),
        );
        shared
            .bib
            .upsert_fulltext_pending(&FullText {
                article_id: article_id.to_owned(),
                attachment_key: None,
                file_path: stored.path,
                file_format: format,
                text_content: None,
                source: FullTextSource::UserUpload,
                file_hash: Some(stored.file_hash),
                file_size: Some(content.len() as i64),
                uploaded_at: None,
                extract_status: None,
                text_format: None,
                extracted_by: None,
                extract_error: None,
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn end_to_end_txt_extraction_without_mineru_key() {
        // No MINERU_API_KEY in the test environment (or it is ignored —
        // the chain must finish locally either way for non-PDF input).
        let shared = shared_with_temp_storage().await;
        let article_id = "doi:10.1/txt-e2e";
        upload_pending(&shared, article_id, b"hello extraction", "notes.txt").await;

        let extracted = run_extraction(&shared, article_id).await.unwrap();
        assert_eq!(extracted.text, "hello extraction");

        let stored = shared.bib.get_fulltext(article_id).await.unwrap().unwrap();
        assert_eq!(stored.extract_status, Some(ExtractStatus::Done));
        assert_eq!(stored.text_format, Some(BibTextFormat::Plain));
        assert!(
            stored
                .extracted_by
                .as_deref()
                .is_some_and(|n| n.starts_with("simple"))
        );
        assert_eq!(stored.extract_error, None);

        // Finished rows leave the pending list.
        assert!(
            shared
                .bib
                .list_articles_needing_extraction()
                .await
                .unwrap()
                .is_empty()
        );

        // The canonical extracted-text object exists and carries the text.
        let attachment = shared
            .bib
            .zotero_attachment(article_id)
            .await
            .unwrap()
            .unwrap();
        let path = vfs_virtual_path(&extracted_attachment_object_path(
            article_id,
            &attachment.key,
            BibTextFormat::Plain,
        ))
        .unwrap();
        let bytes = read_full(shared.file_storage.as_ref().unwrap(), &path)
            .await
            .unwrap();
        assert_eq!(bytes, b"hello extraction");
    }

    #[tokio::test]
    async fn backfill_materializes_canonical_object_for_legacy_done_row() {
        let shared = shared_with_temp_storage().await;
        let article_id = "doi:10.1/legacy-md";
        // A pre-file-backed row: done + markdown, inline only, no object.
        shared
            .bib
            .upsert_article(&bib_types::Article::new(article_id, "Legacy"))
            .await
            .unwrap();
        shared
            .bib
            .upsert_fulltext(&FullText {
                article_id: article_id.to_owned(),
                attachment_key: None,
                file_path: format!("vfs:///literature/{article_id}/deadbeef-paper.pdf"),
                file_format: bib_types::FileFormat::Pdf,
                text_content: Some("# Title\n\nlegacy body ![](images/aaa.jpg)".into()),
                source: FullTextSource::UserUpload,
                file_hash: None,
                file_size: None,
                uploaded_at: None,
                extract_status: Some(ExtractStatus::Done),
                text_format: Some(BibTextFormat::Markdown),
                extracted_by: Some("mineru".into()),
                extract_error: None,
            })
            .await
            .unwrap();
        let storage = shared.file_storage.as_ref().unwrap();
        let attachment = shared
            .bib
            .zotero_attachment(article_id)
            .await
            .unwrap()
            .unwrap();
        let path = vfs_virtual_path(&extracted_attachment_object_path(
            article_id,
            &attachment.key,
            BibTextFormat::Markdown,
        ))
        .unwrap();
        assert!(storage.content_length(&path).await.is_err());
        let rows = shared.bib.list_extracted_for_backfill().await.unwrap();
        assert_eq!(rows.len(), 1, "rows = {rows:?}");
        assert_eq!(rows[0].attachment_key, attachment.key);

        backfill_extracted_objects(&shared).await;
        let bytes = read_full(storage, &path).await.unwrap();
        assert_eq!(bytes, b"# Title\n\nlegacy body ![](images/aaa.jpg)");

        // Idempotent: a second sweep leaves the object untouched.
        backfill_extracted_objects(&shared).await;
        let bytes = read_full(storage, &path).await.unwrap();
        assert_eq!(bytes, b"# Title\n\nlegacy body ![](images/aaa.jpg)");
    }

    #[tokio::test]
    async fn pdf_chain_preserves_primary_structure_and_only_uses_fallback_when_needed() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Backend {
            name: &'static str,
            text: Option<&'static str>,
            calls: AtomicUsize,
        }
        #[async_trait::async_trait]
        impl TextExtractor for Backend {
            fn name(&self) -> &'static str {
                self.name
            }
            async fn extract(&self, _: &[u8], format: FileFormat) -> Result<ExtractedText> {
                assert_eq!(format, FileFormat::Pdf);
                self.calls.fetch_add(1, Ordering::SeqCst);
                let text = self
                    .text
                    .ok_or_else(|| Error::Unknown("unavailable".into()))?;
                Ok(ExtractedText {
                    text: text.into(),
                    extractor: self.name,
                    format: bib_types::TextFormat::Plain,
                    images: vec![],
                    structure: Some(serde_json::json!({"backend": self.name})),
                })
            }
        }
        for primary_text in [
            Some("First paragraph\n\nSecond paragraph"),
            Some(" \n"),
            None,
        ] {
            let primary = Arc::new(Backend {
                name: "zotero-sdt",
                text: primary_text,
                calls: AtomicUsize::new(0),
            });
            let fallback = Arc::new(Backend {
                name: "mineru",
                text: Some("MinerU body"),
                calls: AtomicUsize::new(0),
            });
            let chain = FallbackChainExtractor::new(primary.clone(), fallback.clone());
            let result = chain.extract(b"%PDF", FileFormat::Pdf).await.unwrap();
            let usable = primary_text.is_some_and(|text| !text.trim().is_empty());
            let expected = if usable { "zotero-sdt" } else { "mineru" };
            assert_eq!(result.extractor, expected);
            assert_eq!(
                result.structure,
                Some(serde_json::json!({"backend": expected}))
            );
            assert_eq!(
                result.text,
                if usable {
                    primary_text.unwrap()
                } else {
                    "MinerU body"
                }
            );
            assert_eq!(primary.calls.load(Ordering::SeqCst), 1);
            assert_eq!(fallback.calls.load(Ordering::SeqCst), usize::from(!usable));
        }
        let empty = || {
            Arc::new(Backend {
                name: "empty",
                text: Some(" "),
                calls: AtomicUsize::new(0),
            })
        };
        assert!(
            FallbackChainExtractor::new(empty(), empty())
                .extract(b"%PDF", FileFormat::Pdf)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn pdf_failure_does_not_fall_back_to_builtin_extraction() {
        let shared = shared_with_temp_storage().await;
        let article_id = "doi:10.1/pdf-e2e";
        upload_pending(&shared, article_id, b"%PDF-1.7", "paper.pdf").await;

        struct Unavailable;
        #[async_trait::async_trait]
        impl TextExtractor for Unavailable {
            fn name(&self) -> &'static str {
                "unavailable"
            }
            async fn extract(&self, _: &[u8], _: FileFormat) -> Result<ExtractedText> {
                Err(Error::Unknown("PDF backend unavailable".into()))
            }
        }
        let extractor = FallbackChainExtractor::new(Arc::new(Unavailable), Arc::new(Unavailable));
        let error = run_extraction_parts(
            &shared.bib,
            shared.file_storage.as_ref().unwrap(),
            &extractor,
            article_id,
        )
        .await
        .unwrap_err();
        assert!(error.contains("PDF backend unavailable"));
        let stored = shared.bib.get_fulltext(article_id).await.unwrap().unwrap();
        assert_eq!(stored.extract_status, Some(ExtractStatus::Failed));
        assert!(stored.text_content.is_none());
        // Upload remains recoverable: the original PDF is still available.
        let path = vfs_virtual_path(&stored.file_path).unwrap();
        assert_eq!(
            read_full(shared.file_storage.as_ref().unwrap(), &path)
                .await
                .unwrap(),
            b"%PDF-1.7"
        );
        // Non-PDF formats bypass both failing PDF backends.
        assert_eq!(
            extractor
                .extract(b"paragraph one\n\nparagraph two", FileFormat::Txt)
                .await
                .unwrap()
                .text,
            "paragraph one\n\nparagraph two"
        );
        assert_eq!(
            extractor
                .extract(b"<p>HTML body</p>", FileFormat::Html)
                .await
                .unwrap()
                .text,
            "HTML body"
        );
    }

    #[tokio::test]
    async fn second_claim_loses_and_failure_is_recorded() {
        let shared = shared_with_temp_storage().await;
        let article_id = "doi:10.1/claim";
        upload_pending(&shared, article_id, b"payload", "doc.txt").await;

        // Winner claims; loser gets Err without touching the row state.
        assert!(
            shared
                .bib
                .mark_extraction_running(article_id)
                .await
                .unwrap()
        );
        let loser = run_extraction(&shared, article_id).await;
        assert!(loser.is_err());
        let stored = shared.bib.get_fulltext(article_id).await.unwrap().unwrap();
        assert_eq!(stored.extract_status, Some(ExtractStatus::Running));

        // Return the row to pending so the exploding extractor can claim it.
        assert!(shared.bib.restart_extraction(article_id).await.unwrap());

        // A failing extractor records its message on the row.
        struct ExplodingExtractor;
        #[async_trait::async_trait]
        impl TextExtractor for ExplodingExtractor {
            fn name(&self) -> &'static str {
                "exploding"
            }
            async fn extract(&self, _content: &[u8], _format: FileFormat) -> Result<ExtractedText> {
                Err(Error::Unknown("kaboom".into()))
            }
        }
        let exploded = run_extraction_parts(
            &shared.bib,
            shared.file_storage.as_ref().unwrap(),
            &ExplodingExtractor,
            article_id,
        )
        .await
        .unwrap_err();
        assert!(exploded.contains("kaboom"));
        let stored = shared.bib.get_fulltext(article_id).await.unwrap().unwrap();
        assert_eq!(stored.extract_status, Some(ExtractStatus::Failed));
        assert!(
            stored
                .extract_error
                .as_deref()
                .is_some_and(|e| e.contains("kaboom"))
        );

        // Failed VFS rows are retryable.
        let pending: Vec<PendingExtraction> =
            shared.bib.list_articles_needing_extraction().await.unwrap();
        assert_eq!(pending.len(), 1);
    }

    #[tokio::test]
    async fn sweep_resets_running_and_finishes_pending() {
        let shared = shared_with_temp_storage().await;
        let article_id = "doi:10.1/sweep";
        upload_pending(&shared, article_id, b"sweep me", "s.txt").await;
        assert!(
            shared
                .bib
                .mark_extraction_running(article_id)
                .await
                .unwrap()
        );

        sweep_pending(&shared).await;
        // The spawned task finishes asynchronously; poll briefly for done.
        for _ in 0..50 {
            let stored = shared.bib.get_fulltext(article_id).await.unwrap().unwrap();
            if stored.extract_status == Some(ExtractStatus::Done) {
                assert_eq!(stored.text_content.as_deref(), Some("sweep me"));
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("sweep did not finish the pending extraction in time");
    }

    #[tokio::test]
    async fn figures_stored_and_orphans_cleaned_on_re_extraction() {
        let shared = shared_with_temp_storage().await;
        let article_id = "doi:10.1/figs";
        upload_pending(&shared, article_id, b"figure source", "p.txt").await;
        let storage = shared.file_storage.as_ref().unwrap();

        // A scripted extractor: run 1 yields two figures, run 2 rewrites
        // one — re-extraction overwrites the survivor in place (orphan
        // deletion lives at the restart/replace/delete entry points, not
        // here, because restart clears the old text before claiming).
        struct ScriptedExtractor(std::sync::Mutex<std::collections::VecDeque<ExtractedText>>);
        #[async_trait::async_trait]
        impl TextExtractor for ScriptedExtractor {
            fn name(&self) -> &'static str {
                "scripted"
            }
            async fn extract(&self, _content: &[u8], _format: FileFormat) -> Result<ExtractedText> {
                Ok(self
                    .0
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("scripted extractor exhausted"))
            }
        }
        let extractor = ScriptedExtractor(std::sync::Mutex::new(std::collections::VecDeque::from(
            vec![
                ExtractedText {
                    text: "![](images/aaa.jpg) ![](images/bbb.png)".to_owned(),
                    format: BibTextFormat::Markdown,
                    extractor: "scripted",
                    structure: None,
                    images: vec![
                        ExtractedImage {
                            name: "aaa.jpg".to_owned(),
                            data: vec![1],
                        },
                        ExtractedImage {
                            name: "bbb.png".to_owned(),
                            data: vec![2, 2],
                        },
                    ],
                },
                ExtractedText {
                    text: "![](images/aaa.jpg)".to_owned(),
                    format: BibTextFormat::Markdown,
                    extractor: "scripted",
                    structure: None,
                    images: vec![ExtractedImage {
                        name: "aaa.jpg".to_owned(),
                        data: vec![7, 7, 7],
                    }],
                },
            ],
        )));

        run_extraction_parts(&shared.bib, storage, &extractor, article_id)
            .await
            .unwrap();
        let attachment = shared
            .bib
            .zotero_attachment(article_id)
            .await
            .unwrap()
            .unwrap();
        let figure = |name: &str| {
            vfs_virtual_path(&crate::stored_files::attachment_figure_object_path(
                article_id,
                &attachment.key,
                name,
            ))
            .unwrap()
        };
        assert_eq!(storage.content_length(&figure("aaa.jpg")).await.unwrap(), 1);
        assert_eq!(storage.content_length(&figure("bbb.png")).await.unwrap(), 2);
        let stored = shared.bib.get_fulltext(article_id).await.unwrap().unwrap();
        assert_eq!(stored.text_format, Some(BibTextFormat::Markdown));
        assert_eq!(stored.extract_status, Some(ExtractStatus::Done));

        assert!(shared.bib.restart_extraction(article_id).await.unwrap());
        let extracted = run_extraction_parts(&shared.bib, storage, &extractor, article_id)
            .await
            .unwrap();
        assert_eq!(extracted.images.len(), 1);
        // The figure MinerU re-derived is overwritten in place.
        assert_eq!(storage.content_length(&figure("aaa.jpg")).await.unwrap(), 3);
    }
}

#[cfg(test)]
mod structured_tests {
    use super::*;
    use crate::zotero_extract::{StructuredCache, structured_fulltext};

    struct SemanticExtractor;
    #[async_trait::async_trait]
    impl TextExtractor for SemanticExtractor {
        fn name(&self) -> &'static str {
            "zotero-sdt"
        }
        async fn extract(&self, _: &[u8], _: FileFormat) -> Result<ExtractedText> {
            Ok(ExtractedText {
                text: "First paragraph.\n\nSecond paragraph.".into(),
                format: bib_types::TextFormat::Plain,
                extractor: self.name(),
                images: Vec::new(),
                structure: Some(serde_json::json!({
                    "schemaVersion": "1.2.0",
                    "metadata": { "processor": { "type": "pdf", "version": 15 } },
                    "catalog": { "pages": [], "outline": [] },
                    "content": [{ "type": "paragraph", "content": [{ "text": "First paragraph." }] }]
                })),
            })
        }
    }

    #[tokio::test]
    async fn zotero_structure_cache_preserves_paragraphs_and_rejects_stale_source_or_processor() {
        use crate::stored_files::{stored_fulltext, structured_object_path};
        use bib_types::{Article, FullText, FullTextSource};
        let shared = BibShared::open_in_memory()
            .await
            .unwrap()
            .with_file_storage(Arc::new(vfs::OpendalFileStorage::new_temp()));
        let id = "doi:10.1/structured";
        shared
            .bib
            .upsert_article(&Article::new(id, "Structured document"))
            .await
            .unwrap();
        let stored = stored_fulltext(id, "document.pdf", b"PDF source");
        let storage = shared.file_storage.as_ref().unwrap();
        storage
            .write_bytes(
                &vfs_virtual_path(&stored.path).unwrap(),
                b"PDF source".to_vec(),
            )
            .await
            .unwrap();
        shared
            .bib
            .upsert_fulltext_pending(&FullText {
                article_id: id.into(),
                attachment_key: None,
                file_path: stored.path,
                file_format: FileFormat::Pdf,
                source: FullTextSource::UserUpload,
                file_hash: Some(stored.file_hash),
                file_size: Some(10),
                uploaded_at: None,
                text_content: None,
                extract_status: None,
                text_format: None,
                extracted_by: None,
                extract_error: None,
            })
            .await
            .unwrap();
        assert!(
            structured_fulltext(&shared, id, None, false)
                .await
                .unwrap()
                .is_none()
        );
        run_extraction_parts(&shared.bib, storage, &SemanticExtractor, id)
            .await
            .unwrap();
        let fulltext = shared.bib.get_fulltext(id).await.unwrap().unwrap();
        assert_eq!(
            fulltext.text_content.as_deref(),
            Some("First paragraph.\n\nSecond paragraph.")
        );
        let attachment = shared.bib.zotero_attachment(id).await.unwrap().unwrap();
        let path = vfs_virtual_path(&structured_object_path(id, Some(&attachment.key))).unwrap();
        let bytes = read_full(storage, &path).await.unwrap();
        let mut cache: StructuredCache = serde_json::from_slice(&bytes).unwrap();
        assert!(cache.matches(&fulltext));
        assert!(
            structured_fulltext(&shared, id, Some(&attachment.key), false)
                .await
                .unwrap()
                .is_some()
        );
        cache.file_hash = Some("a different PDF".into());
        storage
            .write_bytes(&path, serde_json::to_vec(&cache).unwrap())
            .await
            .unwrap();
        assert!(
            structured_fulltext(&shared, id, None, false)
                .await
                .unwrap()
                .is_none()
        );
        cache.file_hash = fulltext.file_hash;
        cache.structure["metadata"]["processor"]["version"] = 14.into();
        storage
            .write_bytes(&path, serde_json::to_vec(&cache).unwrap())
            .await
            .unwrap();
        assert!(
            structured_fulltext(&shared, id, None, false)
                .await
                .unwrap()
                .is_none()
        );
        // Re-running a plain/OCR stage must remove any old semantic source map.
        shared
            .bib
            .restart_attachment_extraction(&attachment.key)
            .await
            .unwrap();
        struct PlainExtractor;
        #[async_trait::async_trait]
        impl TextExtractor for PlainExtractor {
            fn name(&self) -> &'static str {
                "plain"
            }
            async fn extract(&self, _: &[u8], _: FileFormat) -> Result<ExtractedText> {
                Ok(ExtractedText::new("Plain fallback text"))
            }
        }
        run_extraction_parts(&shared.bib, storage, &PlainExtractor, id)
            .await
            .unwrap();
        assert!(storage.content_length(&path).await.is_err());
    }
}
