//! Shared deck-engine handle: store + blob areas + exporter, the unit
//! that agent tools and HTTP handlers both consume.

use std::sync::Arc;

use ppt_types::new_deck_id;

use crate::assets::BlobArea;
use crate::deck::Deck;
use crate::error::Result;
use crate::store::{DeckRow, DeckStore};
use crate::export::DeckExporter;

/// Deck engine shared state.
#[derive(Clone)]
pub struct DeckShared {
    pub store: Arc<DeckStore>,
    /// Deck figures (prefix `deck-assets`).
    pub assets: BlobArea,
    /// Export artifacts (prefix `deck-exports`).
    pub exports: BlobArea,
    /// Export backend (headless Chromium in production, Null in tests).
    pub exporter: Arc<dyn DeckExporter>,
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
            assets,
            exports,
            exporter,
        })
    }

    /// Open an in-memory instance (tests): memory blobs + NullExporter.
    pub async fn open_in_memory() -> Result<Self> {
        let store = DeckStore::open_in_memory().await?;
        Ok(Self {
            store: Arc::new(store),
            assets: BlobArea::in_memory("deck-assets"),
            exports: BlobArea::in_memory("deck-exports"),
            exporter: Arc::new(crate::export::NullExporter),
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
