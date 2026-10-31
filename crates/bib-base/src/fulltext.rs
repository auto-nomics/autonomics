//! Full-text storage — CRUD for the [`FullText`] table and FTS index
//! synchronization.
//!
//! PDFs and other document files are stored externally via
//! `OpendalFileStorage`; this table tracks the *path*, extracted
//! plain-text content, and metadata so that agents can search and
//! retrieve full text without touching the binary file.
//!
//! Storage methods are on [`BibCore`] (the actor-owned implementation)
//! and reach callers through the [`BibBase`] handle delegators below.

use chrono::Utc;
use serde::Serialize;
use turso::{
    Value,
    transaction::{Transaction, TransactionBehavior},
};

use crate::actor::BibCommand;
use crate::bib_base::{BibBase, BibCore};
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

/// An attachment-keyed extraction work item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PendingAttachmentExtraction {
    pub article_id: String,
    pub attachment_key: String,
    pub file_path: String,
    pub file_format: FileFormat,
}

/// A completed attachment extraction whose derived VFS object may be missing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExtractedAttachmentBackfill {
    pub article_id: String,
    pub attachment_key: String,
    pub text: String,
    pub format: TextFormat,
}

/// A `done` extraction whose materialized VFS object must match
/// `text_content` byte-for-byte. `attachment_key` mirrors the router in
/// `bib_get_article` (preferred attachment, else article-level), so the
/// repair writes exactly the objects agents are directed at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExtractedTextRepair {
    pub article_id: String,
    pub attachment_key: Option<String>,
    pub text: String,
    pub format: TextFormat,
}

impl BibCore {
    pub(crate) async fn write_fulltext_row(conn: &turso::Connection, ft: &FullText) -> Result<()> {
        let result: Result<()> = conn
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

        result
    }

    /// Insert or update a full-text record.
    ///
    /// As a side effect, all `collection_articles` rows referencing this
    /// article have their `fetch_status` promoted to `fulltext_available`.
    /// Without this sync, collection-level status stays stale at
    /// `metadata_only` even after a full text is stored.
    ///
    /// `text_content` is normalized ([`normalize_extracted_text`]) so the
    /// DB text, the materialized extracted object, and the GUI readers all
    /// stay byte-identical for annotation anchoring.
    pub async fn upsert_fulltext(&self, ft: &FullText) -> Result<()> {
        let mut ft = ft.clone();
        if let Some(text) = ft.text_content.take() {
            ft.text_content = Some(normalize_extracted_text(&text));
        }
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let mut result = Self::write_fulltext_row(&tx, &ft).await;

        if result.is_ok() {
            // Sync collection_articles.fetch_status so collection listings
            // reflect the true full-text availability.
            result = tx
                .execute(
                    "UPDATE collectionItems \
             SET fetch_status = 'fulltext_available' \
             WHERE itemID = (SELECT itemID FROM articleItems WHERE article_id=?1) AND fetch_status != 'fulltext_available'",
                    turso::params![ft.article_id.clone()],
                )
                .await
                .map(|_| ())
                .map_err(crate::error::Error::from);
        }

        if result.is_ok() {
            result = crate::bib_base::sync_search_index(&tx, &ft.article_id).await;
        }
        if result.is_ok() {
            let legacy = crate::zotero_model::LegacyFulltext::from_fulltext(&ft);
            result = crate::zotero_model::sync_fulltext_attachment(&tx, &legacy).await;
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

    /// Load full-text state by its Zotero attachment item key.
    pub async fn get_attachment_fulltext(
        &self,
        article_id: &str,
        attachment_key: &str,
    ) -> Result<Option<FullText>> {
        let conn = self.conn();
        crate::zotero_model::attachment_fulltext(&conn, article_id, attachment_key).await
    }

    /// Reset one attachment's extraction state.
    pub async fn restart_attachment_extraction(
        &self,
        attachment_key: &str,
    ) -> Result<Option<String>> {
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result =
            crate::zotero_model::restart_attachment_extraction(&tx, attachment_key).await?;
        tx.commit().await?;
        Ok(result)
    }

    /// Atomically claim extraction for one attachment key.
    pub async fn mark_attachment_extraction_running(&self, attachment_key: &str) -> Result<bool> {
        let conn = self.write_conn();
        crate::zotero_model::mark_attachment_extraction_running(&conn, attachment_key).await
    }

    /// Record extraction success for one attachment key.
    ///
    /// `text` is normalized ([`normalize_extracted_text`]) before it lands
    /// in `fulltextItems.textContent`.
    pub async fn record_attachment_extraction_success(
        &self,
        attachment_key: &str,
        text: &str,
        format: TextFormat,
        extracted_by: &str,
        extracted_path: &str,
    ) -> Result<Option<String>> {
        let text = normalize_extracted_text(text);
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result = crate::zotero_model::record_attachment_extraction_success(
            &tx,
            attachment_key,
            &text,
            format,
            extracted_by,
            extracted_path,
        )
        .await?;
        tx.commit().await?;
        Ok(result)
    }

    /// Record extraction failure for one attachment key.
    pub async fn record_attachment_extraction_failure(
        &self,
        attachment_key: &str,
        error: &str,
    ) -> Result<Option<String>> {
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result =
            crate::zotero_model::record_attachment_extraction_failure(&tx, attachment_key, error)
                .await?;
        tx.commit().await?;
        Ok(result)
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
        if result.is_ok() {
            let mut legacy = crate::zotero_model::LegacyFulltext::from_fulltext(ft);
            legacy.text_content = None;
            legacy.extract_status = Some(ExtractStatus::Pending);
            legacy.text_format = None;
            legacy.extracted_by = None;
            legacy.extract_error = None;
            result = crate::zotero_model::sync_fulltext_attachment(&tx, &legacy).await;
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

    /// Reset a stored full text back to `pending`, clearing its extracted
    /// text so a re-extraction run can repopulate it.
    ///
    /// Returns `false` when no full-text row exists for `article_id`.
    pub async fn restart_extraction(&self, article_id: &str) -> Result<bool> {
        if let Some(attachment) =
            crate::zotero_model::attachment_for_article(&self.conn(), article_id).await?
        {
            return Ok(self
                .restart_attachment_extraction(&attachment.key)
                .await?
                .is_some());
        }
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
        if let Some(legacy) = crate::zotero_model::LegacyFulltext::load(&tx, article_id).await? {
            crate::zotero_model::sync_fulltext_attachment(&tx, &legacy).await?;
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
        if let Some(attachment) =
            crate::zotero_model::attachment_for_article(&self.conn(), article_id).await?
        {
            return self
                .mark_attachment_extraction_running(&attachment.key)
                .await;
        }
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
    ///
    /// `text` is normalized ([`normalize_extracted_text`]) before it is
    /// stored.
    pub async fn record_extraction_success(
        &self,
        article_id: &str,
        text: &str,
        format: TextFormat,
        extracted_by: &str,
    ) -> Result<()> {
        let text = normalize_extracted_text(text);
        if let Some(attachment) =
            crate::zotero_model::attachment_for_article(&self.conn(), article_id).await?
        {
            let path = crate::stored_files::extracted_attachment_object_path(
                article_id,
                &attachment.key,
                format,
            );
            return self
                .record_attachment_extraction_success(
                    &attachment.key,
                    &text,
                    format,
                    extracted_by,
                    &path,
                )
                .await
                .map(|_| ());
        }
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
                    "UPDATE collectionItems \
             SET fetch_status = 'fulltext_available' \
             WHERE itemID = (SELECT itemID FROM articleItems WHERE article_id=?1) AND fetch_status != 'fulltext_available'",
                    turso::params![article_id],
                )
                .await
                .map(|_| ())
                .map_err(crate::error::Error::from);
        }

        if result.is_ok() {
            result = crate::bib_base::sync_search_index(&tx, article_id).await;
        }
        if result.is_ok() {
            if let Some(legacy) = crate::zotero_model::LegacyFulltext::load(&tx, article_id).await?
            {
                result = crate::zotero_model::sync_fulltext_attachment(&tx, &legacy).await;
            }
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
        if let Some(attachment) =
            crate::zotero_model::attachment_for_article(&self.conn(), article_id).await?
        {
            return self
                .record_attachment_extraction_failure(&attachment.key, error)
                .await
                .map(|_| ());
        }
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        tx.execute(
            "UPDATE fulltexts SET extract_status = 'failed', extract_error = ?2 \
             WHERE article_id = ?1",
            turso::params![article_id, error],
        )
        .await?;
        if let Some(legacy) = crate::zotero_model::LegacyFulltext::load(&tx, article_id).await? {
            crate::zotero_model::sync_fulltext_attachment(&tx, &legacy).await?;
        }
        crate::bib_base::sync_search_index(&tx, article_id).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Return every `running` extraction to `pending` (startup sweep).
    ///
    /// `running` rows can only be left behind by a crashed or killed
    /// process — no live worker would leave them — so resetting them all
    /// is safe at startup, before any new task is spawned.
    pub async fn reset_stale_running(&self) -> Result<u64> {
        let attachment_count = self.reset_stale_attachment_running().await?;
        let conn = self.write_conn();
        let legacy_count = conn
            .execute(
                "UPDATE fulltexts SET extract_status = 'pending' WHERE extract_status = 'running'",
                turso::params![],
            )
            .await
            .map_err(crate::error::Error::from)?;
        Ok(attachment_count.max(legacy_count))
    }

    /// Return attachment-keyed `running` rows to `pending`.
    pub async fn reset_stale_attachment_running(&self) -> Result<u64> {
        let conn = self.write_conn();
        conn.execute(
            "UPDATE fulltextItems SET extractionStatus = 'pending' \
             WHERE extractionStatus = 'running'",
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

    /// List attachment items whose keyed extraction state is unfinished.
    pub async fn list_attachments_needing_extraction(
        &self,
    ) -> Result<Vec<PendingAttachmentExtraction>> {
        let conn = self.conn();
        let mut rows = conn
            .query(
                "SELECT MAP.article_id, I.key, AI.vfsPath, AI.contentType \
                 FROM articleItems MAP \
                 JOIN itemAttachments AI ON AI.parentItemID = MAP.itemID \
                 JOIN items I ON I.itemID = AI.itemID \
                 LEFT JOIN fulltextItems F ON F.itemID = AI.itemID \
                 WHERE COALESCE(F.extractionStatus, 'pending') IN ('pending', 'failed') \
                   AND AI.vfsPath LIKE 'vfs://%'",
                turso::params![],
            )
            .await?;
        let mut pending = Vec::new();
        while let Some(row) = rows.next().await? {
            pending.push(PendingAttachmentExtraction {
                article_id: row.get::<String>(0)?,
                attachment_key: row.get::<String>(1)?,
                file_path: row.get::<String>(2)?,
                file_format: match row.get::<String>(3)?.as_str() {
                    "application/pdf" => FileFormat::Pdf,
                    "text/html" => FileFormat::Html,
                    _ => FileFormat::Txt,
                },
            });
        }
        Ok(pending)
    }

    /// List `done` rows that carry extracted text, for the startup
    /// backfill that materializes their canonical `extracted/` VFS
    /// objects. Returns `(article_id, text, format)` — rows with NULL
    /// text or an unparseable format are skipped (nothing to write).
    pub async fn list_extracted_for_backfill(&self) -> Result<Vec<ExtractedAttachmentBackfill>> {
        let conn = self.conn();
        let mut rows = conn
            .query(
                "SELECT MAP.article_id, I.key, L.text_content, L.text_format \
                 FROM fulltexts L \
                 JOIN articleItems MAP ON MAP.article_id = L.article_id \
                 JOIN itemAttachments AI ON AI.itemID = MAP.attachmentItemID \
                 JOIN items I ON I.itemID = AI.itemID \
                 JOIN fulltextItems F ON F.itemID = AI.itemID \
                 WHERE L.extract_status = 'done' AND L.text_content IS NOT NULL",
                turso::params![],
            )
            .await?;
        let mut done = Vec::new();
        while let Some(row) = rows.next().await? {
            let format = match opt_string(row.get_value(3)?) {
                Some(raw) => TextFormat::from_str(&raw),
                None => continue,
            };
            if let Some(text) = opt_string(row.get_value(2)?) {
                done.push(ExtractedAttachmentBackfill {
                    article_id: row.get::<String>(0)?,
                    attachment_key: row.get::<String>(1)?,
                    text,
                    format,
                });
            }
        }
        Ok(done)
    }

    /// List every `done` extraction with text for the one-time
    /// byte-identity repair of materialized extracted objects. The
    /// attachment key resolves the same way the `bib_get_article` router
    /// does (preferred attachment, else article-level), so repairing these
    /// objects is exactly repairing what agents read.
    pub async fn list_extracted_for_repair(&self) -> Result<Vec<ExtractedTextRepair>> {
        let conn = self.conn();
        let mut rows = conn
            .query(
                "SELECT article_id, text_content, text_format FROM fulltexts \
                 WHERE extract_status = 'done' AND text_content IS NOT NULL",
                turso::params![],
            )
            .await?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            let article_id = row.get::<String>(0)?;
            let (text, format) =
                match (opt_string(row.get_value(1)?), opt_string(row.get_value(2)?)) {
                    (Some(text), Some(raw_format)) => (text, TextFormat::from_str(&raw_format)),
                    _ => continue,
                };
            let attachment_key = crate::zotero_model::attachment_for_article(&conn, &article_id)
                .await?
                .map(|attachment| attachment.key);
            out.push(ExtractedTextRepair {
                article_id,
                attachment_key,
                text,
                format,
            });
        }
        Ok(out)
    }

    /// Claim the one-time extracted-object repair left pending by the
    /// text-normalization migration (`repair`/`repairing` → `repairing`).
    /// `true` obliges the caller to verify the objects and close the claim
    /// with [`Self::finish_text_normalize_repair`]; a crash before that
    /// keeps the claim, so the next startup retries.
    pub async fn claim_text_normalize_repair(&self) -> Result<bool> {
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let mut rows = tx
            .query(
                "SELECT value FROM bib_meta WHERE key = 'text_normalize_v1'",
                turso::params![],
            )
            .await?;
        let state = rows
            .next()
            .await?
            .map(|row| row.get::<String>(0))
            .transpose()?
            .unwrap_or_default();
        drop(rows);
        if state != "repair" && state != "repairing" {
            tx.rollback().await?;
            return Ok(false);
        }
        tx.execute(
            "INSERT INTO bib_meta(key,value) VALUES ('text_normalize_v1','repairing') \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            turso::params![],
        )
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Close the one-time extracted-object repair claim.
    pub async fn finish_text_normalize_repair(&self) -> Result<()> {
        let conn = self.write_conn();
        conn.execute(
            "INSERT INTO bib_meta(key,value) VALUES ('text_normalize_v1','done') \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            turso::params![],
        )
        .await
        .map(|_| ())
        .map_err(crate::error::Error::from)
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
        drop(rows);
        let attachment_key = crate::zotero_model::attachment_for_article(&conn, article_id)
            .await?
            .map(|attachment| attachment.key);

        Ok(Some(FullText {
            article_id: row.get::<String>(0)?,
            attachment_key,
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
        drop(rows);
        let attachment_key = crate::zotero_model::attachment_for_article(&conn, article_id)
            .await?
            .map(|attachment| attachment.key);
        let total_chars = row.get::<i64>(12)? as usize;
        let page_text = opt_string(row.get_value(13)?);
        let returned_chars = page_text.as_ref().map_or(0, |text| text.chars().count());
        let next_offset =
            (offset + returned_chars < total_chars).then_some(offset + returned_chars);

        let fulltext = FullText {
            article_id: row.get::<String>(0)?,
            attachment_key,
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
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        crate::zotero_model::delete_attachment(&tx, article_id).await?;
        tx.execute(
            "DELETE FROM fulltexts WHERE article_id = ?1",
            turso::params![article_id],
        )
        .await?;
        tx.execute(
            "UPDATE collectionItems \
             SET fetch_status = 'metadata_only' \
             WHERE itemID = (SELECT itemID FROM articleItems WHERE article_id=?1) AND fetch_status = 'fulltext_available'",
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
// BibBase — thin handle methods, one command round-trip each
// ---------------------------------------------------------------------------

impl BibBase {
    /// Insert or update a full-text record.
    pub async fn upsert_fulltext(&self, ft: &FullText) -> Result<()> {
        self.call(|reply| BibCommand::UpsertFulltext {
            ft: ft.clone(),
            reply,
        })
        .await
    }

    /// Load full-text state by Zotero attachment key.
    pub async fn get_attachment_fulltext(
        &self,
        article_id: &str,
        attachment_key: &str,
    ) -> Result<Option<FullText>> {
        self.call(|reply| BibCommand::GetAttachmentFulltext {
            article_id: article_id.to_owned(),
            attachment_key: attachment_key.to_owned(),
            reply,
        })
        .await
    }

    /// Reset and claim extraction by Zotero attachment key.
    pub async fn restart_attachment_extraction(
        &self,
        attachment_key: &str,
    ) -> Result<Option<String>> {
        self.call(|reply| BibCommand::RestartAttachmentExtraction {
            attachment_key: attachment_key.to_owned(),
            reply,
        })
        .await
    }

    pub async fn mark_attachment_extraction_running(&self, attachment_key: &str) -> Result<bool> {
        self.call(|reply| BibCommand::MarkAttachmentExtractionRunning {
            attachment_key: attachment_key.to_owned(),
            reply,
        })
        .await
    }

    pub async fn record_attachment_extraction_success(
        &self,
        attachment_key: &str,
        text: &str,
        format: TextFormat,
        extracted_by: &str,
        extracted_path: &str,
    ) -> Result<Option<String>> {
        self.call(|reply| BibCommand::RecordAttachmentExtractionSuccess {
            attachment_key: attachment_key.to_owned(),
            text: text.to_owned(),
            format,
            extracted_by: extracted_by.to_owned(),
            extracted_path: extracted_path.to_owned(),
            reply,
        })
        .await
    }

    pub async fn record_attachment_extraction_failure(
        &self,
        attachment_key: &str,
        error: &str,
    ) -> Result<Option<String>> {
        self.call(|reply| BibCommand::RecordAttachmentExtractionFailure {
            attachment_key: attachment_key.to_owned(),
            error: error.to_owned(),
            reply,
        })
        .await
    }

    /// Insert or update a full-text record whose text is not extracted yet.
    pub async fn upsert_fulltext_pending(&self, ft: &FullText) -> Result<()> {
        self.call(|reply| BibCommand::UpsertFulltextPending {
            ft: ft.clone(),
            reply,
        })
        .await
    }

    /// Reset a stored full text back to `pending`. Returns `false` when
    /// no full-text row exists for `article_id`.
    pub async fn restart_extraction(&self, article_id: &str) -> Result<bool> {
        self.call(|reply| BibCommand::RestartExtraction {
            article_id: article_id.to_owned(),
            reply,
        })
        .await
    }

    /// Atomically claim a pending extraction (`pending → running`).
    pub async fn mark_extraction_running(&self, article_id: &str) -> Result<bool> {
        self.call(|reply| BibCommand::MarkExtractionRunning {
            article_id: article_id.to_owned(),
            reply,
        })
        .await
    }

    /// Persist a finished extraction.
    pub async fn record_extraction_success(
        &self,
        article_id: &str,
        text: &str,
        format: TextFormat,
        extracted_by: &str,
    ) -> Result<()> {
        self.call(|reply| BibCommand::RecordExtractionSuccess {
            article_id: article_id.to_owned(),
            text: text.to_owned(),
            format,
            extracted_by: extracted_by.to_owned(),
            reply,
        })
        .await
    }

    /// Persist a failed extraction with its error message.
    pub async fn record_extraction_failure(&self, article_id: &str, error: &str) -> Result<()> {
        self.call(|reply| BibCommand::RecordExtractionFailure {
            article_id: article_id.to_owned(),
            error: error.to_owned(),
            reply,
        })
        .await
    }

    /// Return every `running` extraction to `pending` (startup sweep).
    pub async fn reset_stale_running(&self) -> Result<u64> {
        self.call(|reply| BibCommand::ResetStaleRunning { reply })
            .await
    }

    /// List full-text rows still needing extraction.
    pub async fn list_articles_needing_extraction(&self) -> Result<Vec<PendingExtraction>> {
        self.call(|reply| BibCommand::ListArticlesNeedingExtraction { reply })
            .await
    }

    /// List unfinished attachment-keyed extractions.
    pub async fn list_attachments_needing_extraction(
        &self,
    ) -> Result<Vec<PendingAttachmentExtraction>> {
        self.call(|reply| BibCommand::ListAttachmentsNeedingExtraction { reply })
            .await
    }

    /// List `done` rows carrying extracted text, for the startup backfill.
    pub async fn list_extracted_for_backfill(&self) -> Result<Vec<ExtractedAttachmentBackfill>> {
        self.call(|reply| BibCommand::ListExtractedForBackfill { reply })
            .await
    }

    /// List `done` extractions for the one-time object byte-identity repair.
    pub async fn list_extracted_for_repair(&self) -> Result<Vec<ExtractedTextRepair>> {
        self.call(|reply| BibCommand::ListExtractedForRepair { reply })
            .await
    }

    /// Claim the one-time extracted-object repair (`true` = claimed).
    pub async fn claim_text_normalize_repair(&self) -> Result<bool> {
        self.call(|reply| BibCommand::ClaimTextNormalizeRepair { reply })
            .await
    }

    /// Close the one-time extracted-object repair claim.
    pub async fn finish_text_normalize_repair(&self) -> Result<()> {
        self.call(|reply| BibCommand::FinishTextNormalizeRepair { reply })
            .await
    }

    /// Retrieve the full-text record for an article, if one exists.
    pub async fn get_fulltext(&self, article_id: &str) -> Result<Option<FullText>> {
        self.call(|reply| BibCommand::GetFulltext {
            article_id: article_id.to_owned(),
            reply,
        })
        .await
    }

    /// Return attachment-keyed `running` rows to `pending`.
    pub async fn reset_stale_attachment_running(&self) -> Result<u64> {
        self.call(|reply| BibCommand::ResetStaleAttachmentRunning { reply })
            .await
    }

    /// Read one bounded character page of a full text.
    pub async fn get_fulltext_page(
        &self,
        article_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Option<FullTextPage>> {
        self.call(|reply| BibCommand::GetFulltextPage {
            article_id: article_id.to_owned(),
            offset,
            limit,
            reply,
        })
        .await
    }

    /// Delete the full-text record for an article.
    pub async fn delete_fulltext(&self, article_id: &str) -> Result<()> {
        self.call(|reply| BibCommand::DeleteFulltext {
            article_id: article_id.to_owned(),
            reply,
        })
        .await
    }

    /// Check whether a full text exists for the given article.
    pub async fn has_fulltext(&self, article_id: &str) -> Result<bool> {
        self.call(|reply| BibCommand::HasFulltext {
            article_id: article_id.to_owned(),
            reply,
        })
        .await
    }
}

// ---------------------------------------------------------------------------
// Extracted-text normalization
// ---------------------------------------------------------------------------

/// Canonical normalization for extracted full text: strip every leading
/// `U+FEFF` BOM, then fold `CRLF` and lone `CR` line endings to `LF`.
///
/// This replicates the GUI text reader's decoding chain and stays ahead of
/// it: the reader strips one BOM after decoding, so the stored text must
/// carry none for the two views to stay byte-identical — stripping *all*
/// leading BOMs guarantees that for any source. `text_content`, the
/// materialized extracted VFS object, and the reader's rendered text then
/// share one coordinate space for character-offset annotation anchors.
/// The function is idempotent; applying it twice changes nothing.
pub(crate) fn normalize_extracted_text(text: &str) -> String {
    let stripped = text.trim_start_matches('\u{feff}');
    if !stripped.contains('\r') {
        return stripped.to_owned();
    }
    let mut normalized = String::with_capacity(stripped.len());
    let mut chars = stripped.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            normalized.push('\n');
        } else {
            normalized.push(character);
        }
    }
    normalized
}

/// One-time migration: normalize legacy `fulltexts.text_content` rows that
/// carry a BOM or CR line endings, mirror each change into the Zotero
/// item tables, and bump the search index so the outbox reindexes the
/// article.
///
/// The `text_normalize_v1` sentinel in `bib_meta` makes this idempotent and
/// coordinates with the startup sweep: `done` skips everything; a fresh
/// run that changed rows records `repair` so the sweep verifies the
/// materialized extracted objects against the new text; `repair` /
/// `repairing` states are left for the sweep to claim and finish (the rows
/// are already normalized by then).
pub(crate) async fn migrate_normalize_text(conn: &turso::Connection) -> Result<()> {
    let mut rows = conn
        .query(
            "SELECT value FROM bib_meta WHERE key = 'text_normalize_v1'",
            turso::params![],
        )
        .await?;
    let state = rows
        .next()
        .await?
        .map(|row| row.get::<String>(0))
        .transpose()?
        .unwrap_or_default();
    drop(rows);
    if state == "done" || state == "repair" || state == "repairing" {
        return Ok(());
    }

    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate).await?;
    let outcome: Result<usize> = async {
        let mut changed = 0usize;
        let mut pending = tx
            .query(
                "SELECT article_id, text_content FROM fulltexts \
                 WHERE text_content LIKE '%' || char(13) || '%' \
                    OR substr(text_content, 1, 1) = char(65279)",
                turso::params![],
            )
            .await?;
        let mut affected = Vec::new();
        while let Some(row) = pending.next().await? {
            let article_id = row.get::<String>(0)?;
            let raw = row.get::<String>(1)?;
            let normalized = normalize_extracted_text(&raw);
            if normalized == raw {
                continue;
            }
            tx.execute(
                "UPDATE fulltexts SET text_content = ?2 WHERE article_id = ?1",
                turso::params![article_id.clone(), normalized],
            )
            .await?;
            affected.push(article_id);
        }
        drop(pending);
        for article_id in &affected {
            if let Some(legacy) = crate::zotero_model::LegacyFulltext::load(&tx, article_id).await?
            {
                crate::zotero_model::sync_fulltext_attachment(&tx, &legacy).await?;
            }
            crate::bib_base::sync_search_index(&tx, article_id).await?;
            changed += 1;
        }
        let sentinel = if changed > 0 { "repair" } else { "done" };
        tx.execute(
            "INSERT INTO bib_meta(key,value) VALUES ('text_normalize_v1',?1) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            turso::params![sentinel],
        )
        .await?;
        Ok(changed)
    }
    .await;
    match outcome {
        Ok(changed) => {
            tx.commit().await?;
            if changed > 0 {
                tracing::info!(
                    articles = changed,
                    "normalized legacy extracted text; object repair scheduled"
                );
            }
            Ok(())
        }
        Err(error) => {
            let _ = tx.rollback().await;
            Err(error)
        }
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
            attachment_key: None,
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
        let hits = base.search_articles("quantum", 10, 0).await.unwrap();
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

    #[test]
    fn normalization_replicates_the_gui_reader_chain() {
        let assert_normalizes = |raw: &str, want: &str| {
            let once = normalize_extracted_text(raw);
            assert_eq!(once, want);
            assert_eq!(
                normalize_extracted_text(&once),
                once,
                "normalization must be idempotent for {raw:?}"
            );
        };
        // CRLF and lone CR fold to LF; existing LF passes through. A
        // trailing CR becomes a trailing LF, matching the reader.
        assert_normalizes("a\r\nb\rc\nd", "a\nb\nc\nd");
        // Every leading BOM is stripped — the reader strips one more after
        // decode, so none may remain for the views to stay byte-identical.
        assert_normalizes("\u{feff}body", "body");
        assert_normalizes("\u{feff}\u{feff}keep", "keep");
        assert_normalizes("\u{feff}a\r\nb", "a\nb");
        assert_normalizes("clean text\n", "clean text\n");
        // CJK and emoji are untouched beyond line endings — the anchor
        // coordinate space must be encoding-stable.
        assert_normalizes(
            "方法显著\r\n降低了死亡率 🚀\r\n",
            "方法显著\n降低了死亡率 🚀\n",
        );
    }

    #[tokio::test]
    async fn fulltext_write_paths_store_normalized_text() {
        let base = BibBase::open_in_memory().await.unwrap();
        let article_id = "doi:10.1/norm";
        base.upsert_article(&Article::new(article_id, "Normalized"))
            .await
            .unwrap();

        // Inline-text rows normalize on upsert.
        base.upsert_fulltext(&FullText {
            article_id: article_id.into(),
            attachment_key: None,
            file_path: "vfs:///literature/x/inline.html".into(),
            file_format: FileFormat::Html,
            text_content: Some("\u{feff}inline\r\ntext\r".into()),
            source: FullTextSource::OpenAccess,
            file_hash: None,
            file_size: None,
            uploaded_at: None,
            extract_status: Some(ExtractStatus::Done),
            text_format: Some(TextFormat::Plain),
            extracted_by: Some("test".into()),
            extract_error: None,
        })
        .await
        .unwrap();
        let stored = base.get_fulltext(article_id).await.unwrap().unwrap();
        assert_eq!(
            stored.text_content.as_deref(),
            Some("inline\ntext\n"),
            "trailing CR folds to LF like the reader"
        );

        // The extraction-success path normalizes too, whether it lands in
        // the legacy `fulltexts` UPDATE or the attachment mirror.
        base.upsert_fulltext(&FullText {
            article_id: article_id.into(),
            attachment_key: None,
            file_path: "vfs:///literature/x/replaced.pdf".into(),
            file_format: FileFormat::Pdf,
            text_content: None,
            source: FullTextSource::UserUpload,
            file_hash: None,
            file_size: None,
            uploaded_at: None,
            extract_status: Some(ExtractStatus::Pending),
            text_format: None,
            extracted_by: None,
            extract_error: None,
        })
        .await
        .unwrap();
        assert!(base.mark_extraction_running(article_id).await.unwrap());
        base.record_extraction_success(
            article_id,
            "\u{feff}body\r\nlines\r",
            TextFormat::Plain,
            "test",
        )
        .await
        .unwrap();
        let stored = base.get_fulltext(article_id).await.unwrap().unwrap();
        assert_eq!(
            stored.text_content.as_deref(),
            Some("body\nlines\n"),
            "record_extraction_success must store normalized text"
        );
        // A mid-text U+FEFF is content, not an encoding marker — preserved.
        base.record_extraction_success(article_id, "bom:\u{feff}kept", TextFormat::Plain, "test")
            .await
            .unwrap();
        let stored = base.get_fulltext(article_id).await.unwrap().unwrap();
        assert_eq!(stored.text_content.as_deref(), Some("bom:\u{feff}kept"));
    }

    #[tokio::test]
    async fn normalization_migration_rewrites_crlf_rows_and_is_idempotent() {
        let directory = std::env::temp_dir().join(format!(
            "bib-base-normalize-migration-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let db_path = directory.join("legacy.db");
        let db_path = db_path.to_str().unwrap();

        // Pre-normalization database: raw CRLF and BOM-prefixed texts.
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
             INSERT INTO articles (id, title) VALUES ('doi:10.1/crlf', 'Crlf');
             INSERT INTO articles (id, title) VALUES ('doi:10.1/clean', 'Clean');
             INSERT INTO fulltexts (article_id, file_path, file_format, text_content, source) \
                VALUES ('doi:10.1/crlf', 'vfs://literature/x/a.txt', 'txt', \
                        '方法\r\n显著 🚀', 'user_upload');
             INSERT INTO fulltexts (article_id, file_path, file_format, text_content, source) \
                VALUES ('doi:10.1/clean', 'vfs://literature/x/b.txt', 'txt', \
                        'already clean', 'user_upload');",
        )
        .await
        .unwrap();
        drop(conn);
        drop(legacy);

        // Opening through BibBase runs the normalization migration.
        let core = BibCore::open(db_path).await.unwrap();
        let crlf = core.get_fulltext("doi:10.1/crlf").await.unwrap().unwrap();
        assert_eq!(crlf.text_content.as_deref(), Some("方法\n显著 🚀"));
        let clean = core.get_fulltext("doi:10.1/clean").await.unwrap().unwrap();
        assert_eq!(clean.text_content.as_deref(), Some("already clean"));

        // The Zotero mirror received the normalized text too.
        let conn = core.conn();
        let mut rows = conn
            .query(
                "SELECT textContent FROM fulltextItems WHERE textContent = ?1",
                turso::params!["方法\n显著 🚀"],
            )
            .await
            .unwrap();
        assert!(rows.next().await.unwrap().is_some());
        drop(rows);

        // The search outbox reindexes the rewritten article...
        let mut rows = conn
            .query(
                "SELECT 1 FROM searchOutbox WHERE articleID = 'doi:10.1/crlf'",
                turso::params![],
            )
            .await
            .unwrap();
        assert!(rows.next().await.unwrap().is_some());
        drop(rows);
        // ...and the sentinel schedules the object repair for the sweep.
        let mut rows = conn
            .query(
                "SELECT value FROM bib_meta WHERE key = 'text_normalize_v1'",
                turso::params![],
            )
            .await
            .unwrap();
        assert_eq!(
            rows.next()
                .await
                .unwrap()
                .unwrap()
                .get::<String>(0)
                .unwrap(),
            "repair"
        );
        drop(rows);
        drop(conn);

        // A second open re-runs nothing: the scan is skipped once the
        // sentinel is set, and the text is already normalized.
        let core = BibCore::open(db_path).await.unwrap();
        let crlf = core.get_fulltext("doi:10.1/crlf").await.unwrap().unwrap();
        assert_eq!(crlf.text_content.as_deref(), Some("方法\n显著 🚀"));

        let _ = std::fs::remove_dir_all(directory);
    }
}
