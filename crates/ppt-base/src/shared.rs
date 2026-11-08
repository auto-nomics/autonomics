//! Shared deck-engine handle: store + blob areas + exporter, the unit
//! that agent tools and HTTP handlers both consume.

use std::sync::Arc;

use ppt_types::new_deck_id;

use crate::assets::BlobArea;
use crate::deck::Deck;
use crate::error::Result;
use crate::store::{DeckRow, DeckStore, ExportFormat, ExportRecord, ExportStatus};
use crate::export::DeckExporter;

/// Enqueue sender; `None` when the worker was never spawned (tests).
pub type ExportQueueHandle = Option<tokio::sync::mpsc::Sender<crate::export::ExportJob>>;

/// Deck engine shared state.
#[derive(Clone)]
pub struct DeckShared {
    pub store: Arc<DeckStore>,
    /// Shared bibliography, for `{{cite:key}}` References generation.
    pub bib: Option<Arc<bib_base::BibBase>>,
    /// The process-wide VFS storage, for `asset_attach(vfs_path=…)` reads.
    /// `None` in in-memory test builds — path-based attach errors clearly.
    #[cfg(feature = "vfs")]
    pub vfs: Option<std::sync::Arc<vfs::OpendalFileStorage>>,
    #[cfg(not(feature = "vfs"))]
    pub vfs: Option<()>,
    /// Deck figures (prefix `deck-assets`).
    pub assets: BlobArea,
    /// Export artifacts (prefix `deck-exports`).
    pub exports: BlobArea,
    /// Export backend (headless Chromium in production, Null in tests).
    pub exporter: Arc<dyn DeckExporter>,
    /// Single-worker export queue; created via [`DeckShared::with_queue`].
    pub export_queue: ExportQueueHandle,
}

/// Serde envelope for the parsed-deck cache stored in `decks.parsed_cache`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CachedDeck {
    pub meta: ppt_types::DeckMeta,
    pub slides: Vec<ppt_types::SlideSpec>,
}

impl DeckShared {
    /// Open with explicit blob areas and exporter.
    pub async fn open_with(
        db_path: impl AsRef<str>,
        assets: BlobArea,
        exports: BlobArea,
        exporter: Arc<dyn DeckExporter>,
    ) -> Result<Self> {
        let store = DeckStore::open(db_path).await?;
        Ok(Self {
            store: Arc::new(store),
            bib: None,
            vfs: None,
            assets,
            exports,
            exporter,
            export_queue: None,
        })
    }

    /// Swap the export backend (tests inject stubs).
    pub fn with_exporter(
        mut self,
        exporter: std::sync::Arc<dyn crate::export::DeckExporter>,
    ) -> Self {
        self.exporter = exporter;
        self
    }

    /// Attach the shared VFS storage (path-based asset attachment).
    #[cfg(feature = "vfs")]
    pub fn with_vfs(mut self, storage: Option<std::sync::Arc<vfs::OpendalFileStorage>>) -> Self {
        self.vfs = storage;
        self
    }

    /// Attach the shared bibliography (citation tool support).
    pub fn with_bib(mut self, bib: Option<Arc<bib_base::BibBase>>) -> Self {
        self.bib = bib;
        self
    }

    /// Attach the export queue worker (production wiring).
    pub fn with_queue(mut self) -> Self {
        let worker = crate::export::queue::ExportWorker {
            store: self.store.clone(),
            exports: self.exports.clone(),
            exporter: self.exporter.clone(),
        };
        self.export_queue = Some(crate::export::queue::ExportWorker::spawn(worker));
        self
    }

    /// Mint a one-time print token + create a queued export row and push
    /// it onto the queue. Returns `(record, raw_token)`.
    pub async fn enqueue_export(
        &self,
        deck_id: &str,
        format: ExportFormat,
        dpi: u32,
        pages: Vec<usize>,
        canvas_width: u32,
        canvas_height: u32,
        notes: Vec<String>,
    ) -> Result<(ExportRecord, String)> {
        let raw_token = uuid::Uuid::new_v4().simple().to_string();
        let token_hash = {
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(raw_token.as_bytes());
            format!("{:x}", h.finalize())
        };
        let expires = (chrono::Utc::now() + chrono::Duration::seconds(15 * 60)).to_rfc3339();
        let export_id = format!("exp-{}", uuid::Uuid::new_v4().simple());
        let now = chrono::Utc::now().to_rfc3339();
        let record = ExportRecord {
            id: export_id.clone(),
            deck_id: deck_id.to_string(),
            format,
            status: ExportStatus::Queued,
            params: serde_json::json!({
                "dpi": dpi,
                "pages": pages,
                "notes": notes.len(),
            })
            .to_string(),
            print_token_hash: Some(token_hash),
            token_expires_at: Some(expires),
            artifact_key: None,
            artifact_mime: None,
            artifact_size: None,
            error: None,
            created_at: now.clone(),
            updated_at: now,
        };
        self.store.create_export(&record).await?;
        if let Some(tx) = &self.export_queue {
            let job = crate::export::ExportJob {
                deck_id: deck_id.to_string(),
                export_id,
                format,
                dpi,
                pages,
                canvas_width,
                canvas_height,
                notes,
                print_token: raw_token.clone(),
            };
            if tx.send(job).await.is_err() {
                tracing::warn!("export queue receiver dropped; job stays queued");
            }
        } else if self.export_queue.is_none() {
            // No queue (tests without a worker): leave the row queued —
            // callers observe the record as-is.
        }
        Ok((record, raw_token))
    }

    /// Open an in-memory instance (tests): memory blobs + NullExporter.
    pub async fn open_in_memory() -> Result<Self> {
        let store = DeckStore::open_in_memory().await?;
        Ok(Self {
            store: Arc::new(store),
            bib: None,
            vfs: None,
            assets: BlobArea::in_memory("deck-assets"),
            exports: BlobArea::in_memory("deck-exports"),
            exporter: Arc::new(crate::export::NullExporter),
            export_queue: None,
        })
    }

    /// Load a deck: parsed-cache fast path keyed by revision, assemble
    /// fallback that warms the cache (Slidev revision pattern).
    pub async fn load_deck(&self, id: &str) -> Result<Deck> {
        let row: DeckRow = self.store.get_deck_row(id).await?;
        if !row.parsed_cache.is_empty() {
            if let Ok(cached) = serde_json::from_str::<CachedDeck>(&row.parsed_cache) {
                if cached.meta.revision == row.revision && cached.meta.slide_count == cached.slides.len() {
                    return Ok(Deck {
                        meta: cached.meta,
                        slides: cached.slides,
                    });
                }
            }
        }
        let deck = crate::deck::assemble(&row.markdown, &row.id, &row.created_at, &row.updated_at);
        let cache = serde_json::to_string(&CachedDeck {
            meta: deck.meta.clone(),
            slides: deck.slides.clone(),
        })
        .unwrap_or_default();
        // Best-effort cache fill; a failure must not fail the read.
        let _ = self
            .store
            .update_parsed_cache(id, &deck.meta.revision, &cache)
            .await;
        Ok(deck)
    }

    /// Create a deck from a template.
    pub async fn create_deck(
        &self,
        title: &str,
        template: Option<&str>,
    ) -> Result<Deck> {
        let markdown = crate::deck::templates::starter_markdown(template, title);
        let id = new_deck_id();
        let deck = crate::deck::assemble(&markdown, &id, "", "");
        let cache = serde_json::to_string(&CachedDeck {
            meta: deck.meta.clone(),
            slides: deck.slides.clone(),
        })
        .unwrap_or_default();
        self.store
            .create_deck(&id, title, &markdown, &deck.meta.revision, &cache)
            .await?;
        // Reload to pick up real timestamps from the row.
        let row = self.store.get_deck_row(&id).await?;
        let mut deck = deck;
        deck.meta.created_at = row.created_at.clone();
        deck.meta.updated_at = row.updated_at;
        Ok(deck)
    }

    /// Render ONE slide to PNG for agent preview: mint an ephemeral
    /// print-token row, run the exporter synchronously (NOT via the
    /// queue — the caller is waiting), then delete the row so the token
    /// dies immediately. `no` is 1-based; `clicks` selects the reveal
    /// state (pass the slide's total for fully-revealed).
    pub async fn preview_slide_png(
        &self,
        deck_id: &str,
        no: usize,
        clicks: u32,
        dpi: u32,
    ) -> Result<Vec<u8>> {
        let deck = self.load_deck(deck_id).await?;
        let slide = deck
            .slide(no)
            .ok_or_else(|| crate::Error::SlideOutOfRange {
                deck: deck_id.to_string(),
                no,
                total: deck.len(),
            })?;

        let raw_token = uuid::Uuid::new_v4().simple().to_string();
        let token_hash = {
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(raw_token.as_bytes());
            format!("{:x}", h.finalize())
        };
        let export_id = format!("preview-{}", uuid::Uuid::new_v4().simple());
        let now = chrono::Utc::now().to_rfc3339();
        let record = ExportRecord {
            id: export_id.clone(),
            deck_id: deck_id.to_string(),
            format: crate::store::ExportFormat::Png,
            status: ExportStatus::Queued,
            params: serde_json::json!({
                "preview": true,
                "slide": no,
                "clicks": clicks,
            })
            .to_string(),
            print_token_hash: Some(token_hash),
            token_expires_at: Some(
                (chrono::Utc::now() + chrono::Duration::seconds(5 * 60)).to_rfc3339(),
            ),
            artifact_key: None,
            artifact_mime: None,
            artifact_size: None,
            error: None,
            created_at: now.clone(),
            updated_at: now,
        };
        self.store.create_export(&record).await?;

        let job = crate::export::ExportJob {
            deck_id: deck_id.to_string(),
            export_id: export_id.clone(),
            format: crate::store::ExportFormat::Png,
            dpi,
            pages: vec![no],
            canvas_width: deck.meta.canvas(),
            canvas_height: deck.meta.canvas_height(),
            notes: Vec::new(),
            print_token: raw_token,
        };
        let result = self.exporter.preview_png(&job, no as u32, clicks).await;
        // The token row dies with the preview either way.
        let _ = self.store.delete_export(&export_id).await;
        let _ = slide;
        result
    }

    /// Replace canonical markdown and return the re-assembled deck.
    pub async fn save_markdown(
        &self,
        id: &str,
        markdown: &str,
        message: &str,
    ) -> Result<Deck> {
        let row = self.store.get_deck_row(id).await?;
        let deck = crate::deck::assemble(markdown, id, &row.created_at, "");
        let cache = serde_json::to_string(&CachedDeck {
            meta: deck.meta.clone(),
            slides: deck.slides.clone(),
        })
        .unwrap_or_default();
        self.store
            .save_deck(
                id,
                &deck.meta.title,
                markdown,
                &deck.meta.revision,
                &cache,
                message,
                None,
            )
            .await?;
        Ok(deck)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_and_load_roundtrip() {
        let shared = DeckShared::open_in_memory().await.unwrap();
        let deck = shared
            .create_deck("My Meeting", Some("meeting"))
            .await
            .unwrap();
        assert!(deck.len() > 3);
        assert_eq!(deck.meta.title, "My Meeting");

        let loaded = shared.load_deck(&deck.meta.id).await.unwrap();
        assert_eq!(loaded.slides.len(), deck.slides.len());

        // Cache path: modify the row's cache, revision still matches.
        let mut modified = loaded.clone();
        modified.meta.title = "cache-poisoned".into();
        let cache = serde_json::to_string(&CachedDeck {
            meta: modified.meta,
            slides: modified.slides,
        })
        .unwrap();
        shared
            .store
            .update_parsed_cache(&deck.meta.id, &loaded.meta.revision, &cache)
            .await
            .unwrap();
        let cached = shared.load_deck(&deck.meta.id).await.unwrap();
        assert_eq!(cached.meta.title, "cache-poisoned");
    }

    #[tokio::test]
    async fn save_invalidates_cache_by_revision() {
        let shared = DeckShared::open_in_memory().await.unwrap();
        let deck = shared.create_deck("T", None).await.unwrap();
        let id = deck.meta.id.clone();

        let md = deck_raw(&deck.meta.id);
        let _ = md;
        let new_md = "# New Title Slide\n\n---\n\n## Second\n";
        let updated = shared.save_markdown(&id, new_md, "rewrite").await.unwrap();
        assert_eq!(updated.len(), 2);
        assert_eq!(updated.meta.title, "New Title Slide");

        // stale cache must not serve: revision changed
        let loaded = shared.load_deck(&id).await.unwrap();
        assert_eq!(loaded.len(), 2);
    }

    fn deck_raw(_id: &str) -> String {
        String::new()
    }
}
