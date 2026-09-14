//! Full-text storage — CRUD for the [`FullText`] table and FTS index
//! synchronization.
//!
//! PDFs and other document files are stored externally via
//! `OpendalFileStorage`; this table tracks the *path*, extracted
//! plain-text content, and metadata so that agents can search and
//! retrieve full text without touching the binary file.

use chrono::Utc;
use serde::Serialize;
use turso::{
    Value,
    transaction::{Transaction, TransactionBehavior},
};

use crate::bib_base::BibBase;
use crate::error::Result;
use bib_types::{ExtractStatus, FileFormat, FullText, FullTextSource, TextFormat};

/// A bounded view of a stored full text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FullTextPage {
    pub fulltext: FullText,
    pub offset: usize,
    pub limit: usize,
    pub total_chars: usize,
    pub truncated: bool,
    pub next_offset: Option<usize>,
}

/// A full-text row whose extraction has not succeeded yet.
///
/// Carries just enough to re-read the original file from the VFS and
/// re-run the extraction chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PendingExtraction {
    pub article_id: String,
    pub file_path: String,
    pub file_format: FileFormat,
}

impl BibBase {
    /// Insert or update a full-text record.
    ///
    /// As a side effect, all `collection_articles` rows referencing this
    /// article have their `fetch_status` promoted to `fulltext_available`.
    /// Without this sync, collection-level status stays stale at
    /// `metadata_only` even after a full text is stored.
    pub async fn upsert_fulltext(&self, ft: &FullText) -> Result<()> {
        let _write = self.write_gate.lock().await;
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let mut result: Result<()> = tx
            .execute(
                "INSERT INTO fulltexts \
             (article_id, file_path, file_format, text_content, source, \
              file_hash, file_size, uploaded_at, \
              extract_status, text_format, extracted_by, extract_error) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) \
             ON CONFLICT(article_id) DO UPDATE SET \
                file_path = excluded.file_path, \
                file_format = excluded.file_format, \
                text_content = excluded.text_content, \
                source = excluded.source, \
                file_hash = excluded.file_hash, \
                file_size = excluded.file_size, \
                uploaded_at = excluded.uploaded_at, \
                extract_status = excluded.extract_status, \
                text_format = excluded.text_format, \
                extracted_by = excluded.extracted_by, \
                extract_error = excluded.extract_error",
                turso::params![
                    ft.article_id.clone(),
                    ft.file_path.clone(),
                    ft.file_format.as_str(),
                    ft.text_content.clone(),
                    ft.source.as_str(),
                    ft.file_hash.clone(),
                    ft.file_size,
                    ft.uploaded_at.map(|t| t.to_rfc3339()),
                    ft.extract_status.map(|s| s.as_str()),
                    ft.text_format.map(|f| f.as_str()),
                    ft.extracted_by.clone(),
                    ft.extract_error.clone(),
                ],
            )
            .await
            .map(|_| ())
            .map_err(crate::error::Error::from);

        if result.is_ok() {
            // Sync collection_articles.fetch_status so collection listings
            // reflect the true full-text availability.
            result = tx
                .execute(
                    "UPDATE collection_articles \
             SET fetch_status = 'fulltext_available' \
             WHERE article_id = ?1 AND fetch_status != 'fulltext_available'",
                    turso::params![ft.article_id.clone()],
                )
                .await
                .map(|_| ())
                .map_err(crate::error::Error::from);
        }

        if result.is_ok() {
            result = crate::bib_base::sync_search_index(&tx, &ft.article_id).await;
        }

        if result.is_err() {
            let _ = tx.rollback().await;
            return result;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Insert or update a full-text record whose text is not extracted yet.
    ///
    /// The file bytes are already stored; a background job fills in
    /// `text_content` later via [`Self::record_extraction_success`] or
    /// [`Self::record_extraction_failure`]. Unlike [`Self::upsert_fulltext`],
    /// this does **not** promote `collection_articles.fetch_status` — that
    /// only happens once real text lands. The search index is resynced so a
    /// replaced upload immediately drops its stale full-text terms.
    pub async fn upsert_fulltext_pending(&self, ft: &FullText) -> Result<()> {
        let _write = self.write_gate.lock().await;
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let mut result: Result<()> = tx
            .execute(
                "INSERT INTO fulltexts \
             (article_id, file_path, file_format, text_content, source, \
              file_hash, file_size, uploaded_at, \
              extract_status, text_format, extracted_by, extract_error) \
             VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7, 'pending', NULL, NULL, NULL) \
             ON CONFLICT(article_id) DO UPDATE SET \
                file_path = excluded.file_path, \
                file_format = excluded.file_format, \
                text_content = NULL, \
                source = excluded.source, \
                file_hash = excluded.file_hash, \
                file_size = excluded.file_size, \
                uploaded_at = excluded.uploaded_at, \
                extract_status = 'pending', \
                text_format = NULL, \
                extracted_by = NULL, \
                extract_error = NULL",
                turso::params![
                    ft.article_id.clone(),
                    ft.file_path.clone(),
                    ft.file_format.as_str(),
                    ft.source.as_str(),
                    ft.file_hash.clone(),
                    ft.file_size,
                    ft.uploaded_at.map(|t| t.to_rfc3339()),
                ],
            )
            .await
            .map(|_| ())
            .map_err(crate::error::Error::from);

        if result.is_ok() {
            result = crate::bib_base::sync_search_index(&tx, &ft.article_id).await;
        }

        if result.is_err() {
            let _ = tx.rollback().await;
            return result;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Reset a stored full text back to `pending`, clearing its extracted
    /// text so a re-extraction run can repopulate it.
    ///
    /// Returns `false` when no full-text row exists for `article_id`.
    pub async fn restart_extraction(&self, article_id: &str) -> Result<bool> {
        let _write = self.write_gate.lock().await;
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let n = tx
            .execute(
                "UPDATE fulltexts \
             SET text_content = NULL, extract_status = 'pending', \
                 text_format = NULL, extracted_by = NULL, extract_error = NULL \
             WHERE article_id = ?1",
                turso::params![article_id],
            )
            .await?;
        if n == 0 {
            let _ = tx.rollback().await;
            return Ok(false);
        }
        crate::bib_base::sync_search_index(&tx, article_id).await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Atomically claim a pending extraction: `pending → running`.
    ///
    /// The compare-and-set means only one of concurrent claimants (HTTP
    /// re-extract, CLI backfill, startup sweep) proceeds per row; the rest
    /// see `false` and skip. Returns whether this caller claimed the row.
    pub async fn mark_extraction_running(&self, article_id: &str) -> Result<bool> {
        let _write = self.write_gate.lock().await;
        let conn = self.write_conn();
        let n = conn
            .execute(
                "UPDATE fulltexts SET extract_status = 'running' \
             WHERE article_id = ?1 AND extract_status = 'pending'",
                turso::params![article_id],
            )
            .await?;
        Ok(n > 0)
    }

    /// Persist a finished extraction: text, format, provenance, and the
    /// `collection_articles.fetch_status` promotion + search-index sync
    /// that [`Self::upsert_fulltext`] performs for inline extraction.
    pub async fn record_extraction_success(
        &self,
        article_id: &str,
        text: &str,
        format: TextFormat,
        extracted_by: &str,
    ) -> Result<()> {
        let _write = self.write_gate.lock().await;
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let mut result: Result<()> = tx
            .execute(
                "UPDATE fulltexts \
             SET text_content = ?2, extract_status = 'done', \
                 text_format = ?3, extracted_by = ?4, extract_error = NULL \
             WHERE article_id = ?1",
                turso::params![article_id, text, format.as_str(), extracted_by],
            )
            .await
            .map(|_| ())
            .map_err(crate::error::Error::from);

        if result.is_ok() {
            // Same promotion as upsert_fulltext: collection listings should
            // reflect that real full text now exists.
            result = tx
                .execute(
                    "UPDATE collection_articles \
             SET fetch_status = 'fulltext_available' \
             WHERE article_id = ?1 AND fetch_status != 'fulltext_available'",
                    turso::params![article_id],
                )
                .await
                .map(|_| ())
                .map_err(crate::error::Error::from);
        }

        if result.is_ok() {
            result = crate::bib_base::sync_search_index(&tx, article_id).await;
        }

        if result.is_err() {
            let _ = tx.rollback().await;
            return result;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Persist a failed extraction with its error message.
    ///
    /// Any previously stored text is left untouched (it is normally NULL —
    /// restarts clear it before re-running), so a failure never destroys
    /// the last good extraction of a *different* run's row.
    pub async fn record_extraction_failure(&self, article_id: &str, error: &str) -> Result<()> {
        let _write = self.write_gate.lock().await;
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        tx.execute(
            "UPDATE fulltexts SET extract_status = 'failed', extract_error = ?2 \
             WHERE article_id = ?1",
            turso::params![article_id, error],
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Return every `running` extraction to `pending` (startup sweep).
    ///
    /// `running` rows can only be left behind by a crashed or killed
    /// process — no live worker would leave them — so resetting them all
    /// is safe at startup, before any new task is spawned.
    pub async fn reset_stale_running(&self) -> Result<u64> {
        let _write = self.write_gate.lock().await;
        let conn = self.write_conn();
        conn.execute(
            "UPDATE fulltexts SET extract_status = 'pending' WHERE extract_status = 'running'",
            turso::params![],
        )
        .await
        .map_err(crate::error::Error::from)
    }

    /// List full-text rows still needing extraction: `pending` or `failed`,
    /// whose original file lives in the VFS (only `vfs://` files can be
    /// re-read and re-extracted; `europepmc:` paths carry inline text
    /// already).
    pub async fn list_articles_needing_extraction(&self) -> Result<Vec<PendingExtraction>> {
        let conn = self.conn();
        let mut rows = conn
            .query(
                "SELECT article_id, file_path, file_format FROM fulltexts \
             WHERE extract_status IN ('pending', 'failed') \
               AND file_path LIKE 'vfs://%'",
                turso::params![],
            )
            .await?;
        let mut pending = Vec::new();
        while let Some(row) = rows.next().await? {
            pending.push(PendingExtraction {
                article_id: row.get::<String>(0)?,
                file_path: row.get::<String>(1)?,
                file_format: parse_file_format(&row.get::<String>(2)?),
            });
        }
        Ok(pending)
    }

    /// Retrieve the full-text record for an article, if one exists.
    pub async fn get_fulltext(&self, article_id: &str) -> Result<Option<FullText>> {
        let conn = self.conn();
        let mut rows = conn
            .query(
                "SELECT article_id, file_path, file_format, text_content, \
                        source, file_hash, file_size, uploaded_at, \
                        extract_status, text_format, extracted_by, extract_error \
                 FROM fulltexts WHERE article_id = ?1",
                turso::params![article_id],
            )
            .await?;

        let row = match rows.next().await? {
            Some(row) => row,
            None => return Ok(None),
        };

        Ok(Some(FullText {
            article_id: row.get::<String>(0)?,
            file_path: row.get::<String>(1)?,
            file_format: parse_file_format(&row.get::<String>(2)?),
            text_content: opt_string(row.get_value(3)?),
            source: FullTextSource::from_str(&row.get::<String>(4)?),
            file_hash: opt_string(row.get_value(5)?),
            file_size: opt_int(row.get_value(6)?),
            uploaded_at: opt_string(row.get_value(7)?).as_deref().and_then(parse_dt),
            extract_status: opt_string(row.get_value(8)?)
                .as_deref()
                .map(ExtractStatus::from_str),
            text_format: opt_string(row.get_value(9)?)
                .as_deref()
                .map(TextFormat::from_str),
            extracted_by: opt_string(row.get_value(10)?),
            extract_error: opt_string(row.get_value(11)?),
        }))
    }

    /// Read one bounded character page of a full text.
    ///
    /// The database extracts only the requested substring, so API and agent
    /// callers do not need to materialize an entire paper in their response.
    pub async fn get_fulltext_page(
        &self,
        article_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Option<FullTextPage>> {
        let conn = self.conn();
        let mut rows = conn
            .query(
                "SELECT article_id, file_path, file_format, text_content, source, \
                        file_hash, file_size, uploaded_at, \
                        extract_status, text_format, extracted_by, extract_error, \
                        COALESCE(LENGTH(text_content), 0), \
                        COALESCE(SUBSTR(text_content, ?2, ?3), '') \
                 FROM fulltexts WHERE article_id = ?1",
                turso::params![article_id, offset as i64 + 1, limit as i64,],
            )
            .await?;

        let row = match rows.next().await? {
            Some(row) => row,
            None => return Ok(None),
        };
        let total_chars = row.get::<i64>(12)? as usize;
        let page_text = opt_string(row.get_value(13)?);
        let returned_chars = page_text.as_ref().map_or(0, |text| text.chars().count());
        let next_offset =
            (offset + returned_chars < total_chars).then_some(offset + returned_chars);

        let fulltext = FullText {
            article_id: row.get::<String>(0)?,
            file_path: row.get::<String>(1)?,
            file_format: parse_file_format(&row.get::<String>(2)?),
            text_content: page_text,
            source: FullTextSource::from_str(&row.get::<String>(4)?),
            file_hash: opt_string(row.get_value(5)?),
            file_size: opt_int(row.get_value(6)?),
            uploaded_at: opt_string(row.get_value(7)?).as_deref().and_then(parse_dt),
            extract_status: opt_string(row.get_value(8)?)
                .as_deref()
                .map(ExtractStatus::from_str),
            text_format: opt_string(row.get_value(9)?)
                .as_deref()
                .map(TextFormat::from_str),
            extracted_by: opt_string(row.get_value(10)?),
            extract_error: opt_string(row.get_value(11)?),
        };

        Ok(Some(FullTextPage {
            fulltext,
            offset,
            limit,
            total_chars,
            truncated: next_offset.is_some(),
            next_offset,
        }))
    }

    /// Delete the full-text record for an article.
    pub async fn delete_fulltext(&self, article_id: &str) -> Result<()> {
        let _write = self.write_gate.lock().await;
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        tx.execute(
            "DELETE FROM fulltexts WHERE article_id = ?1",
            turso::params![article_id],
        )
        .await?;
        tx.execute(
            "UPDATE collection_articles \
             SET fetch_status = 'metadata_only' \
             WHERE article_id = ?1 AND fetch_status = 'fulltext_available'",
            turso::params![article_id],
        )
        .await?;
        crate::bib_base::sync_search_index(&tx, article_id).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Check whether a full text exists for the given article.
    pub async fn has_fulltext(&self, article_id: &str) -> Result<bool> {
        let conn = self.conn();
        let mut rows = conn
            .query(
                "SELECT 1 FROM fulltexts WHERE article_id = ?1 LIMIT 1",
                turso::params![article_id],
            )
            .await?;
        Ok(rows.next().await?.is_some())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn opt_string(v: Value) -> Option<String> {
    match v {
        Value::Text(s) => Some(s),
        Value::Null => None,
        _ => None,
    }
}

fn opt_int(v: Value) -> Option<i64> {
    match v {
        Value::Integer(i) => Some(i),
        Value::Null => None,
        _ => None,
    }
}

fn parse_file_format(s: &str) -> FileFormat {
    match s {
        "pdf" => FileFormat::Pdf,
        "html" => FileFormat::Html,
        _ => FileFormat::Txt,
    }
}

fn parse_dt(s: &str) -> Option<chrono::DateTime<Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use bib_types::{AddedBy, Article, ArticleRole, Collection, FetchStatus};

    fn pending_upload(article_id: &str) -> FullText {
        FullText {
            article_id: article_id.to_owned(),
            file_path: format!("vfs://literature/{article_id}/deadbeef-paper.pdf"),
            file_format: FileFormat::Pdf,
            text_content: None,
            source: FullTextSource::UserUpload,
            file_hash: Some("deadbeef".to_owned()),
            file_size: Some(4),
            uploaded_at: Some(Utc::now()),
            extract_status: None,
            text_format: None,
            extracted_by: None,
            extract_error: None,
        }
    }

    #[tokio::test]
    async fn extraction_lifecycle_pending_claim_success_failure() {
        let base = BibBase::open_in_memory().await.unwrap();
        let article_id = "doi:10.1/lifecycle";
        base.upsert_article(&Article::new(article_id, "Lifecycle paper"))
            .await
            .unwrap();
        base.upsert_collection(&Collection::new("col-1", "Research"))
            .await
            .unwrap();
        base.add_to_collection(
            "col-1",
            article_id,
            ArticleRole::Referenced,
            AddedBy::User,
            None,
        )
        .await
        .unwrap();

        // Pending upload: no text, no fetch_status promotion yet.
        base.upsert_fulltext_pending(&pending_upload(article_id))
            .await
            .unwrap();
        let stored = base.get_fulltext(article_id).await.unwrap().unwrap();
        assert_eq!(stored.extract_status, Some(ExtractStatus::Pending));
        assert!(stored.text_content.is_none());
        let associations = base
            .list_collection_articles("col-1", None, None)
            .await
            .unwrap();
        assert_eq!(associations[0].fetch_status, FetchStatus::MetadataOnly);

        // Only one concurrent claimant wins the pending → running CAS.
        assert!(base.mark_extraction_running(article_id).await.unwrap());
        assert!(!base.mark_extraction_running(article_id).await.unwrap());

        // Success stores text + provenance, promotes fetch_status, and
        // feeds the search index.
        base.record_extraction_success(
            article_id,
            "quantum tunneling measurement notes",
            TextFormat::Markdown,
            "mineru",
        )
        .await
        .unwrap();
        let done = base.get_fulltext(article_id).await.unwrap().unwrap();
        assert_eq!(done.extract_status, Some(ExtractStatus::Done));
        assert_eq!(done.text_format, Some(TextFormat::Markdown));
        assert_eq!(done.extracted_by.as_deref(), Some("mineru"));
        assert_eq!(done.extract_error, None);
        assert_eq!(
            done.text_content.as_deref(),
            Some("quantum tunneling measurement notes")
        );
        let associations = base
            .list_collection_articles("col-1", None, None)
            .await
            .unwrap();
        assert_eq!(associations[0].fetch_status, FetchStatus::FulltextAvailable);
        let hits = base.search_articles("quantum", 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].article_id, article_id);

        // Restart clears text back to pending; failure records the error.
        assert!(base.restart_extraction(article_id).await.unwrap());
        assert!(!base.restart_extraction("doi:10.1/missing").await.unwrap());
        base.record_extraction_failure(article_id, "mineru quota exhausted")
            .await
            .unwrap();
        let failed = base.get_fulltext(article_id).await.unwrap().unwrap();
        assert_eq!(failed.extract_status, Some(ExtractStatus::Failed));
        assert!(failed.text_content.is_none());
        assert_eq!(
            failed.extract_error.as_deref(),
            Some("mineru quota exhausted")
        );
        let pending = base.list_articles_needing_extraction().await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].article_id, article_id);
        assert_eq!(pending[0].file_format, FileFormat::Pdf);

        // Startup sweep returns an orphaned running row to pending.
        base.restart_extraction(article_id).await.unwrap();
        assert!(base.mark_extraction_running(article_id).await.unwrap());
        assert_eq!(base.reset_stale_running().await.unwrap(), 1);
        let swept = base.get_fulltext(article_id).await.unwrap().unwrap();
        assert_eq!(swept.extract_status, Some(ExtractStatus::Pending));
    }

    #[tokio::test]
    async fn migration_backfills_extraction_columns_on_legacy_db() {
        let dir = std::env::temp_dir().join(format!(
            "bib-base-migration-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("legacy.db");
        let db_path = db_path.to_str().unwrap();

        // Create a pre-extraction-tracking database: 17-column articles and
        // the old 8-column fulltexts, one row with text and one without.
        let mut builder = turso::Builder::new_local(db_path);
        builder = builder.experimental_multiprocess_wal(true);
        let legacy = builder.build().await.unwrap();
        let conn = legacy.connect().unwrap();
        conn.execute_batch(
            "CREATE TABLE articles (\
                id TEXT PRIMARY KEY, title TEXT NOT NULL, abstract TEXT, year INTEGER, \
                month INTEGER, journal TEXT, volume TEXT, issue TEXT, pages TEXT, \
                issn TEXT, essn TEXT, language TEXT, pub_types TEXT, keywords TEXT, \
                source TEXT, created_at TEXT, updated_at TEXT);
             CREATE TABLE fulltexts (\
                article_id TEXT PRIMARY KEY REFERENCES articles(id) ON DELETE CASCADE, \
                file_path TEXT NOT NULL, file_format TEXT NOT NULL, text_content TEXT, \
                source TEXT NOT NULL, file_hash TEXT, file_size INTEGER, uploaded_at TEXT);
             INSERT INTO articles (id, title) VALUES ('doi:10.1/kept', 'Kept');
             INSERT INTO articles (id, title) VALUES ('doi:10.1/empty', 'Empty');
             INSERT INTO fulltexts (article_id, file_path, file_format, text_content, source) \
                VALUES ('doi:10.1/kept', 'vfs://literature/x/a.pdf', 'pdf', 'legacy text', 'user_upload');
             INSERT INTO fulltexts (article_id, file_path, file_format, text_content, source) \
                VALUES ('doi:10.1/empty', 'vfs://literature/x/b.pdf', 'pdf', NULL, 'user_upload');",
        )
        .await
        .unwrap();
        drop(conn);
        drop(legacy);

        // Opening through BibBase runs the detect-and-ALTER migration.
        let base = BibBase::open(db_path).await.unwrap();

        let kept = base.get_fulltext("doi:10.1/kept").await.unwrap().unwrap();
        assert_eq!(kept.extract_status, Some(ExtractStatus::Done));
        assert_eq!(kept.text_format, Some(TextFormat::Plain));
        assert_eq!(kept.text_content.as_deref(), Some("legacy text"));

        let empty = base.get_fulltext("doi:10.1/empty").await.unwrap().unwrap();
        assert_eq!(empty.extract_status, Some(ExtractStatus::Failed));
        assert!(empty.text_content.is_none());

        // The text-less legacy row is now retryable via the sweep/CLI path.
        let pending = base.list_articles_needing_extraction().await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].article_id, "doi:10.1/empty");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
