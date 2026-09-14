//! Background full-text extraction orchestration.
//!
//! Uploads only store the original file and mark the row `pending`; this
//! module owns everything that happens afterwards:
//!
//! - [`FallbackChainExtractor`] — MinerU cloud first, local
//!   pdf-extract/tesseract as fallback (any MinerU failure — missing key,
//!   quota, network — degrades to the local chain instead of erroring).
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
use crate::extract::{ExtractedText, OcrFallbackExtractor, TextExtractor};
use crate::mineru::MineruExtractor;
use crate::shared::BibShared;
use crate::stored_files::vfs_virtual_path;
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
/// Non-PDF input goes straight to `fallback` — MinerU only accepts PDFs.
/// When both stages fail, the fallback's error is surfaced (it is the one
/// the local machine can act on).
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
            return self.fallback.extract(content, format).await;
        }

        match self.primary.extract(content, format).await {
            Ok(text) if !text.text.trim().is_empty() => Ok(text),
            primary_outcome => {
                match &primary_outcome {
                    Err(error) => tracing::warn!(
                        extractor = self.primary.name(),
                        error = %error,
                        "primary extractor failed; trying local fallback"
                    ),
                    Ok(_) => tracing::warn!(
                        extractor = self.primary.name(),
                        "primary extractor produced no text; trying local fallback"
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

/// The process-wide extraction chain: MinerU cloud → local simple + OCR.
///
/// Reads `MINERU_API_URL` / `MINERU_API_KEY` from the environment; without
/// a key every PDF simply takes the local path.
pub fn default_extractor(http: reqwest::Client) -> Arc<dyn TextExtractor> {
    Arc::new(FallbackChainExtractor::new(
        Arc::new(MineruExtractor::from_env(http)),
        Arc::new(OcrFallbackExtractor::new()),
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
        Ok(extracted) => {
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

async fn record_failure(bib: &BibBase, article_id: &str, message: &str) {
    if let Err(error) = bib.record_extraction_failure(article_id, message).await {
        tracing::warn!(article_id, error = %error, "failed to record extraction failure");
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

/// Startup recovery: return orphaned `running` rows to `pending` and
/// spawn tasks for everything still unfinished (`pending` + `failed` with
/// a VFS-stored original).
pub async fn sweep_pending(shared: &BibShared) {
    if let Err(error) = shared.bib.reset_stale_running().await {
        tracing::warn!(error = %error, "failed to reset stale running extractions");
    }
    let pending = match shared.bib.list_articles_needing_extraction().await {
        Ok(pending) => pending,
        Err(error) => {
            tracing::warn!(error = %error, "failed to list articles needing extraction");
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
        spawn_extraction(shared.clone(), item.article_id);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bib_base::BibBase;
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
    }

    #[tokio::test]
    async fn end_to_end_pdf_falls_back_when_mineru_unavailable() {
        let shared = shared_with_temp_storage().await;
        let article_id = "doi:10.1/pdf-e2e";
        let pdf = build_minimal_pdf();
        upload_pending(&shared, article_id, &pdf, "paper.pdf").await;

        // The default chain without a MinerU key: primary errors out and
        // the local extractor produces the text.
        let extractor = default_extractor(reqwest::Client::new());
        let extracted = run_extraction_parts(
            &shared.bib,
            shared.file_storage.as_ref().unwrap(),
            extractor.as_ref(),
            article_id,
        )
        .await
        .unwrap();
        assert!(extracted.text.contains("FallbackChain"));

        let stored = shared.bib.get_fulltext(article_id).await.unwrap().unwrap();
        assert_eq!(stored.extract_status, Some(ExtractStatus::Done));
        assert!(
            stored
                .extracted_by
                .as_deref()
                .is_some_and(|n| n.starts_with("simple"))
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

    /// Minimal single-page PDF whose text pdf-extract can decode.
    fn build_minimal_pdf() -> Vec<u8> {
        use lopdf::content::{Content, Operation};
        use lopdf::{Dictionary, Object, Stream, dictionary};

        let mut doc = lopdf::Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
        });
        let resources_id = doc.add_object(dictionary! {
            "Font" => dictionary! { "F1" => font_id },
        });
        let content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 12.into()]),
                Operation::new("Tj", vec![Object::string_literal("FallbackChain PDF body")]),
                Operation::new("ET", vec![]),
            ],
        };
        let content_id = doc.add_object(Stream::new(
            Dictionary::new(),
            content.encode().expect("PDF content should encode"),
        ));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
            "Contents" => content_id,
        });
        let pages = dictionary! {
            "Type" => "Pages",
            "Kids" => vec![Object::Reference(page_id)],
            "Count" => 1,
        };
        doc.objects.insert(pages_id, Object::Dictionary(pages));
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);

        let mut pdf = Vec::new();
        doc.save_to(&mut pdf)
            .expect("generated PDF should serialize");
        pdf
    }
}
