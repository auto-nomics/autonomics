//! Turso (libSQL) storage for decks, deck versions, assets, and exports.
//!
//! Pattern follows `writing-base::store`: one cheaply-cloneable
//! [`turso::Connection`], idempotent `CREATE TABLE IF NOT EXISTS`
//! migrations, JSON-blob rows. The canonical deck source is markdown;
//! the parsed [`ppt_types`] deck is stored as a JSON cache keyed by the
//! source revision (Slidev's revision-hash cache-busting pattern).

use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use turso::Builder;

use crate::{Error, Result};

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

const SCHEMA_SQL: &str = "\
CREATE TABLE IF NOT EXISTS decks (
    id           TEXT PRIMARY KEY,
    title        TEXT NOT NULL,
    markdown     TEXT NOT NULL,
    revision     TEXT NOT NULL,
    parsed_cache TEXT NOT NULL DEFAULT '',
    version      INTEGER NOT NULL DEFAULT 1,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS deck_versions (
    id          TEXT PRIMARY KEY,
    deck_id     TEXT NOT NULL REFERENCES decks(id) ON DELETE CASCADE,
    version     INTEGER NOT NULL,
    markdown    TEXT NOT NULL,
    author      TEXT,
    message     TEXT,
    created_at  TEXT NOT NULL,
    UNIQUE(deck_id, version)
);

CREATE INDEX IF NOT EXISTS idx_deck_versions
    ON deck_versions(deck_id, version);

CREATE TABLE IF NOT EXISTS deck_assets (
    id          TEXT PRIMARY KEY,
    deck_id     TEXT NOT NULL REFERENCES decks(id) ON DELETE CASCADE,
    filename    TEXT NOT NULL,
    mime        TEXT NOT NULL,
    size        INTEGER NOT NULL,
    sha256      TEXT NOT NULL,
    caption     TEXT,
    provenance  TEXT,
    blob_key    TEXT NOT NULL,
    created_at  TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_deck_assets
    ON deck_assets(deck_id);

CREATE TABLE IF NOT EXISTS deck_exports (
    id                TEXT PRIMARY KEY,
    deck_id           TEXT NOT NULL REFERENCES decks(id) ON DELETE CASCADE,
    format            TEXT NOT NULL,
    status            TEXT NOT NULL,
    params            TEXT NOT NULL DEFAULT '{}',
    print_token_hash  TEXT,
    token_expires_at  TEXT,
    artifact_key      TEXT,
    artifact_mime     TEXT,
    artifact_size     INTEGER,
    error             TEXT,
    created_at        TEXT NOT NULL,
    updated_at        TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_deck_exports
    ON deck_exports(deck_id);
";

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// Lightweight deck summary for list views.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeckSummary {
    pub id: String,
    pub title: String,
    pub slide_count: i64,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// A deck row: canonical markdown plus the parsed-cache envelope.
#[derive(Debug, Clone)]
pub struct DeckRow {
    pub id: String,
    pub title: String,
    pub markdown: String,
    pub revision: String,
    pub parsed_cache: String,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// Asset metadata row (blob bytes live in the vfs-backed asset store).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetMeta {
    pub id: String,
    pub deck_id: String,
    pub filename: String,
    pub mime: String,
    pub size: i64,
    pub sha256: String,
    pub caption: Option<String>,
    pub provenance: Option<String>,
    pub blob_key: String,
    pub created_at: String,
}

/// Export job lifecycle states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportStatus {
    Queued,
    Running,
    Done,
    Error,
}

impl ExportStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExportStatus::Queued => "queued",
            ExportStatus::Running => "running",
            ExportStatus::Done => "done",
            ExportStatus::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "running" => ExportStatus::Running,
            "done" => ExportStatus::Done,
            "error" => ExportStatus::Error,
            _ => ExportStatus::Queued,
        }
    }
}

/// Export artifact formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Pdf,
    Png,
    Pptx,
}

impl ExportFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExportFormat::Pdf => "pdf",
            ExportFormat::Png => "png",
            ExportFormat::Pptx => "pptx",
        }
    }

    pub fn mime(&self) -> &'static str {
        match self {
            ExportFormat::Pdf => "application/pdf",
            ExportFormat::Png => "image/png",
            ExportFormat::Pptx => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pdf" => Some(ExportFormat::Pdf),
            "png" => Some(ExportFormat::Png),
            "pptx" => Some(ExportFormat::Pptx),
            _ => None,
        }
    }
}

/// One export job row.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportRecord {
    pub id: String,
    pub deck_id: String,
    pub format: ExportFormat,
    pub status: ExportStatus,
    /// Job params as JSON (dpi, range, …).
    pub params: String,
    /// Hashed one-time print token (never the raw token).
    pub print_token_hash: Option<String>,
    pub token_expires_at: Option<String>,
    pub artifact_key: Option<String>,
    pub artifact_mime: Option<String>,
    pub artifact_size: Option<i64>,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

// ---------------------------------------------------------------------------
// DeckStore
// ---------------------------------------------------------------------------

/// Deck database handle (cheaply cloneable; every method takes `&self`).
#[derive(Clone)]
pub struct DeckStore {
    conn: Arc<turso::Connection>,
}

impl DeckStore {
    /// Open (or create) a local database file.
    pub async fn open(path: impl AsRef<str>) -> Result<Self> {
        let path = path.as_ref();
        let mut builder = Builder::new_local(path);
        if path != ":memory:" {
            builder = builder.experimental_multiprocess_wal(true);
        }
        let db = builder.build().await?;
        let conn = db.connect()?;
        if path != ":memory:" {
            conn.pragma_update("busy_timeout", 5000).await?;
        }
        let store = Self {
            conn: Arc::new(conn),
        };
        store.migrate().await?;
        Ok(store)
    }

    /// Open an in-memory database (tests).
    pub async fn open_in_memory() -> Result<Self> {
        Self::open(":memory:").await
    }

    /// Run migrations (idempotent).
    pub async fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(SCHEMA_SQL).await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Deck CRUD
    // -----------------------------------------------------------------------

    /// Insert a new deck row with its initial markdown.
    pub async fn create_deck(
        &self,
        id: &str,
        title: &str,
        markdown: &str,
        revision: &str,
        parsed_cache: &str,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.conn
            .execute(
                "INSERT INTO decks (id, title, markdown, revision, parsed_cache, version, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7)",
                turso::params![id, title, markdown, revision, parsed_cache, now.clone(), now],
            )
            .await?;
        self.snapshot_version(id, 1, markdown, None, "create").await?;
        Ok(())
    }

    /// Replace the canonical markdown: recompute revision/cache, bump
    /// version, snapshot.
    pub async fn save_deck(
        &self,
        id: &str,
        title: &str,
        markdown: &str,
        revision: &str,
        parsed_cache: &str,
        message: &str,
        author: Option<&str>,
    ) -> Result<i64> {
        let now = Utc::now().to_rfc3339();
        let existing = self.get_deck_row(id).await?;
        let version = existing.version + 1;
        let n = self
            .conn
            .execute(
                "UPDATE decks SET title = ?2, markdown = ?3, revision = ?4, parsed_cache = ?5, \
                 version = ?6, updated_at = ?7 WHERE id = ?1",
                turso::params![id, title, markdown, revision, parsed_cache, version, now.clone()],
            )
            .await?;
        if n == 0 {
            return Err(Error::DeckNotFound(id.to_string()));
        }
        self.snapshot_version(id, version, markdown, author, message)
            .await?;
        Ok(version)
    }

    /// Update only the parsed cache (cache warm-up after a read).
    pub async fn update_parsed_cache(&self, id: &str, revision: &str, parsed_cache: &str) -> Result<()> {
        self.conn
            .execute(
                "UPDATE decks SET parsed_cache = ?2, revision = ?3 WHERE id = ?1",
                turso::params![id, parsed_cache, revision],
            )
            .await?;
        Ok(())
    }

    async fn snapshot_version(
        &self,
        deck_id: &str,
        version: i64,
        markdown: &str,
        author: Option<&str>,
        message: &str,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let version_id = format!("{deck_id}_v{version}");
        self.conn
            .execute(
                "INSERT OR REPLACE INTO deck_versions \
                 (id, deck_id, version, markdown, author, message, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                turso::params![
                    version_id,
                    deck_id,
                    version,
                    markdown,
                    author,
                    message,
                    now
                ],
            )
            .await?;
        Ok(())
    }

    /// Full deck row (markdown + cache envelope).
    pub async fn get_deck_row(&self, id: &str) -> Result<DeckRow> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, title, markdown, revision, parsed_cache, version, created_at, updated_at \
                 FROM decks WHERE id = ?1",
                turso::params![id],
            )
            .await?;
        match rows.next().await? {
            Some(row) => Ok(DeckRow {
                id: row.get(0)?,
                title: row.get(1)?,
                markdown: row.get(2)?,
                revision: row.get(3)?,
                parsed_cache: row.get(4)?,
                version: row.get::<i64>(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            }),
            None => Err(Error::DeckNotFound(id.to_string())),
        }
    }

    /// List deck summaries.
    pub async fn list_decks(&self) -> Result<Vec<DeckSummary>> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, title, version, created_at, updated_at FROM decks ORDER BY updated_at DESC",
                turso::params![],
            )
            .await?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            out.push(DeckSummary {
                id: row.get(0)?,
                title: row.get(1)?,
                slide_count: 0,
                version: row.get::<i64>(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            });
        }
        Ok(out)
    }

    /// Delete a deck and all child rows.
    pub async fn delete_deck(&self, id: &str) -> Result<()> {
        for table in ["deck_versions", "deck_assets", "deck_exports"] {
            self.conn
                .execute(&format!("DELETE FROM {table} WHERE deck_id = ?1"), turso::params![id])
                .await?;
        }
        let n = self
            .conn
            .execute("DELETE FROM decks WHERE id = ?1", turso::params![id])
            .await?;
        if n == 0 {
            return Err(Error::DeckNotFound(id.to_string()));
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Assets
    // -----------------------------------------------------------------------

    /// Insert asset metadata.
    pub async fn add_asset(&self, meta: &AssetMeta) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO deck_assets \
                 (id, deck_id, filename, mime, size, sha256, caption, provenance, blob_key, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                turso::params![
                    meta.id.clone(),
                    meta.deck_id.clone(),
                    meta.filename.clone(),
                    meta.mime.clone(),
                    meta.size,
                    meta.sha256.clone(),
                    meta.caption.clone(),
                    meta.provenance.clone(),
                    meta.blob_key.clone(),
                    meta.created_at.clone(),
                ],
            )
            .await?;
        Ok(())
    }

    /// List a deck's assets.
    pub async fn list_assets(&self, deck_id: &str) -> Result<Vec<AssetMeta>> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, deck_id, filename, mime, size, sha256, caption, provenance, blob_key, created_at \
                 FROM deck_assets WHERE deck_id = ?1 ORDER BY created_at",
                turso::params![deck_id],
            )
            .await?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            out.push(AssetMeta {
                id: row.get(0)?,
                deck_id: row.get(1)?,
                filename: row.get(2)?,
                mime: row.get(3)?,
                size: row.get::<i64>(4)?,
                sha256: row.get(5)?,
                caption: row.get(6)?,
                provenance: row.get(7)?,
                blob_key: row.get(8)?,
                created_at: row.get(9)?,
            });
        }
        Ok(out)
    }

    /// One asset.
    pub async fn get_asset(&self, deck_id: &str, asset_id: &str) -> Result<AssetMeta> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, deck_id, filename, mime, size, sha256, caption, provenance, blob_key, created_at \
                 FROM deck_assets WHERE deck_id = ?1 AND id = ?2",
                turso::params![deck_id, asset_id],
            )
            .await?;
        match rows.next().await? {
            Some(row) => Ok(AssetMeta {
                id: row.get(0)?,
                deck_id: row.get(1)?,
                filename: row.get(2)?,
                mime: row.get(3)?,
                size: row.get::<i64>(4)?,
                sha256: row.get(5)?,
                caption: row.get(6)?,
                provenance: row.get(7)?,
                blob_key: row.get(8)?,
                created_at: row.get(9)?,
            }),
            None => Err(Error::Asset(format!("asset {asset_id} not found"))),
        }
    }

    /// Delete asset metadata (caller removes the blob).
    pub async fn delete_asset(&self, deck_id: &str, asset_id: &str) -> Result<String> {
        let meta = self.get_asset(deck_id, asset_id).await?;
        self.conn
            .execute(
                "DELETE FROM deck_assets WHERE deck_id = ?1 AND id = ?2",
                turso::params![deck_id, asset_id],
            )
            .await?;
        Ok(meta.blob_key)
    }

    // -----------------------------------------------------------------------
    // Exports
    // -----------------------------------------------------------------------

    /// Insert a queued export job.
    pub async fn create_export(&self, record: &ExportRecord) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO deck_exports \
                 (id, deck_id, format, status, params, print_token_hash, token_expires_at, \
                  artifact_key, artifact_mime, artifact_size, error, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                turso::params![
                    record.id.clone(),
                    record.deck_id.clone(),
                    record.format.as_str(),
                    record.status.as_str(),
                    record.params.clone(),
                    record.print_token_hash.clone(),
                    record.token_expires_at.clone(),
                    record.artifact_key.clone(),
                    record.artifact_mime.clone(),
                    record.artifact_size,
                    record.error.clone(),
                    record.created_at.clone(),
                    record.updated_at.clone(),
                ],
            )
            .await?;
        Ok(())
    }

    /// Transition an export job (status / artifact / error fields).
    pub async fn update_export(&self, record: &ExportRecord) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.conn
            .execute(
                "UPDATE deck_exports SET status = ?2, artifact_key = ?3, artifact_mime = ?4, \
                 artifact_size = ?5, error = ?6, print_token_hash = ?7, token_expires_at = ?8, \
                 updated_at = ?9 WHERE id = ?1",
                turso::params![
                    record.id.clone(),
                    record.status.as_str(),
                    record.artifact_key.clone(),
                    record.artifact_mime.clone(),
                    record.artifact_size,
                    record.error.clone(),
                    record.print_token_hash.clone(),
                    record.token_expires_at.clone(),
                    now,
                ],
            )
            .await?;
        Ok(())
    }

    /// One export job.
    pub async fn get_export(&self, export_id: &str) -> Result<ExportRecord> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, deck_id, format, status, params, print_token_hash, token_expires_at, \
                 artifact_key, artifact_mime, artifact_size, error, created_at, updated_at \
                 FROM deck_exports WHERE id = ?1",
                turso::params![export_id],
            )
            .await?;
        match rows.next().await? {
            Some(row) => Ok(Self::export_from_row(&row)?),
            None => Err(Error::Export(format!("export {export_id} not found"))),
        }
    }

    /// List a deck's export jobs.
    pub async fn list_exports(&self, deck_id: &str) -> Result<Vec<ExportRecord>> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, deck_id, format, status, params, print_token_hash, token_expires_at, \
                 artifact_key, artifact_mime, artifact_size, error, created_at, updated_at \
                 FROM deck_exports WHERE deck_id = ?1 ORDER BY created_at DESC",
                turso::params![deck_id],
            )
            .await?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            out.push(Self::export_from_row(&row)?);
        }
        Ok(out)
    }

    /// Mark every queued/running export as errored (restart sweep: print
    /// tokens are hash-only, so those jobs cannot be re-driven).
    pub async fn fail_stale_exports(&self, message: &str) -> Result<()> {
        self.conn
            .execute(
                "UPDATE deck_exports SET status = 'error', error = ?1, updated_at = ?2                  WHERE status IN ('queued', 'running')",
                turso::params![message, chrono::Utc::now().to_rfc3339()],
            )
            .await?;
        Ok(())
    }

    /// Find an export by deck id + valid print token hash.
    pub async fn get_export_by_token(&self, deck_id: &str, token_hash: &str) -> Result<ExportRecord> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, deck_id, format, status, params, print_token_hash, token_expires_at, \
                 artifact_key, artifact_mime, artifact_size, error, created_at, updated_at \
                 FROM deck_exports WHERE deck_id = ?1 AND print_token_hash = ?2",
                turso::params![deck_id, token_hash],
            )
            .await?;
        match rows.next().await? {
            Some(row) => Ok(Self::export_from_row(&row)?),
            None => Err(Error::Export("invalid print token".into())),
        }
    }

    fn export_from_row(row: &turso::Row) -> Result<ExportRecord> {
        Ok(ExportRecord {
            id: row.get(0)?,
            deck_id: row.get(1)?,
            format: ExportFormat::parse(&row.get::<String>(2)?).unwrap_or(ExportFormat::Pdf),
            status: ExportStatus::parse(&row.get::<String>(3)?),
            params: row.get(4)?,
            print_token_hash: row.get(5)?,
            token_expires_at: row.get(6)?,
            artifact_key: row.get(7)?,
            artifact_mime: row.get(8)?,
            artifact_size: row.get(9)?,
            error: row.get(10)?,
            created_at: row.get(11)?,
            updated_at: row.get(12)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> DeckStore {
        DeckStore::open_in_memory().await.unwrap()
    }

    #[tokio::test]
    async fn deck_crud_and_versions() {
        let s = store().await;
        s.create_deck("deck-1", "T", "# a\n", "rev1", "cache1")
            .await
            .unwrap();

        let row = s.get_deck_row("deck-1").await.unwrap();
        assert_eq!(row.markdown, "# a\n");
        assert_eq!(row.version, 1);

        let v = s
            .save_deck("deck-1", "T2", "# b\n", "rev2", "cache2", "edit", Some("agent"))
            .await
            .unwrap();
        assert_eq!(v, 2);

        let row = s.get_deck_row("deck-1").await.unwrap();
        assert_eq!(row.markdown, "# b\n");
        assert_eq!(row.revision, "rev2");

        let decks = s.list_decks().await.unwrap();
        assert_eq!(decks.len(), 1);
        assert_eq!(decks[0].title, "T2");

        s.delete_deck("deck-1").await.unwrap();
        assert!(s.get_deck_row("deck-1").await.is_err());
    }

    #[tokio::test]
    async fn missing_deck_is_not_found() {
        let s = store().await;
        let err = s.get_deck_row("nope").await.unwrap_err();
        assert!(matches!(err, Error::DeckNotFound(_)));
    }

    #[tokio::test]
    async fn parsed_cache_update() {
        let s = store().await;
        s.create_deck("deck-1", "T", "md", "rev1", "").await.unwrap();
        s.update_parsed_cache("deck-1", "rev1", "{\"ok\":true}")
            .await
            .unwrap();
        let row = s.get_deck_row("deck-1").await.unwrap();
        assert_eq!(row.parsed_cache, "{\"ok\":true}");
    }

    #[tokio::test]
    async fn asset_rows() {
        let s = store().await;
        s.create_deck("deck-1", "T", "md", "rev", "").await.unwrap();
        let meta = AssetMeta {
            id: "asset-1".into(),
            deck_id: "deck-1".into(),
            filename: "fig1.png".into(),
            mime: "image/png".into(),
            size: 1234,
            sha256: "abc".into(),
            caption: Some("Panel A".into()),
            provenance: Some("dag://run/42/plot".into()),
            blob_key: "deck-assets/deck-1/asset-1.png".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        s.add_asset(&meta).await.unwrap();
        let list = s.list_assets("deck-1").await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].caption.as_deref(), Some("Panel A"));
        assert_eq!(list[0].provenance.as_deref(), Some("dag://run/42/plot"));

        let key = s.delete_asset("deck-1", "asset-1").await.unwrap();
        assert_eq!(key, meta.blob_key);
        assert!(s.list_assets("deck-1").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn export_lifecycle() {
        let s = store().await;
        s.create_deck("deck-1", "T", "md", "rev", "").await.unwrap();

        let rec = ExportRecord {
            id: "export-1".into(),
            deck_id: "deck-1".into(),
            format: ExportFormat::Pdf,
            status: ExportStatus::Queued,
            params: "{\"dpi\":192}".into(),
            print_token_hash: Some("hash123".into()),
            token_expires_at: Some("2026-01-01T00:15:00Z".into()),
            artifact_key: None,
            artifact_mime: None,
            artifact_size: None,
            error: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        };
        s.create_export(&rec).await.unwrap();

        let mut rec = s.get_export("export-1").await.unwrap();
        assert_eq!(rec.status, ExportStatus::Queued);
        assert_eq!(rec.format, ExportFormat::Pdf);

        rec.status = ExportStatus::Done;
        rec.artifact_key = Some("deck-exports/deck-1/export-1.pdf".into());
        rec.artifact_mime = Some(ExportFormat::Pdf.mime().into());
        rec.artifact_size = Some(9999);
        s.update_export(&rec).await.unwrap();

        let rec = s.get_export("export-1").await.unwrap();
        assert_eq!(rec.status, ExportStatus::Done);
        assert_eq!(rec.artifact_size, Some(9999));

        let by_token = s.get_export_by_token("deck-1", "hash123").await.unwrap();
        assert_eq!(by_token.id, "export-1");
        assert!(s.get_export_by_token("deck-1", "wrong").await.is_err());

        let list = s.list_exports("deck-1").await.unwrap();
        assert_eq!(list.len(), 1);
    }

    #[tokio::test]
    async fn delete_deck_removes_children() {
        let s = store().await;
        s.create_deck("deck-1", "T", "md", "rev", "").await.unwrap();
        s.add_asset(&AssetMeta {
            id: "a".into(),
            deck_id: "deck-1".into(),
            filename: "f".into(),
            mime: "image/png".into(),
            size: 1,
            sha256: "h".into(),
            caption: None,
            provenance: None,
            blob_key: "k".into(),
            created_at: "t".into(),
        })
        .await
        .unwrap();
        s.delete_deck("deck-1").await.unwrap();
        assert!(s.list_assets("deck-1").await.unwrap().is_empty());
    }
}
