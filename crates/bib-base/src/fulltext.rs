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
use bib_types::{FileFormat, FullText, FullTextSource};

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
              file_hash, file_size, uploaded_at, parse_status, parse_engine, parse_error) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) \
             ON CONFLICT(article_id) DO UPDATE SET \
                file_path = excluded.file_path, \
                file_format = excluded.file_format, \
                text_content = excluded.text_content, \
                source = excluded.source, \
                file_hash = excluded.file_hash, \
                file_size = excluded.file_size, \
                uploaded_at = excluded.uploaded_at, \
                parse_status = excluded.parse_status, \
                parse_engine = excluded.parse_engine, \
                parse_error = excluded.parse_error",
                turso::params![
                    ft.article_id.clone(),
                    ft.file_path.clone(),
                    ft.file_format.as_str(),
                    ft.text_content.clone(),
                    ft.source.as_str(),
                    ft.file_hash.clone(),
                    ft.file_size,
                    ft.uploaded_at.map(|t| t.to_rfc3339()),
                    ft.parse_status.clone(),
                    ft.parse_engine.clone(),
                    ft.parse_error.clone(),
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

    /// Retrieve the full-text record for an article, if one exists.
    pub async fn get_fulltext(&self, article_id: &str) -> Result<Option<FullText>> {
        let conn = self.conn();
        let mut rows = conn
            .query(
                "SELECT article_id, file_path, file_format, text_content, \
                        source, file_hash, file_size, uploaded_at, \
                        parse_status, parse_engine, parse_error \
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
            parse_status: parse_status_value(row.get_value(8)?),
            parse_engine: opt_string(row.get_value(9)?),
            parse_error: opt_string(row.get_value(10)?),
        }))
    }

    /// IDs of every article with a stored full text.
    ///
    /// Lets the HTTP list endpoint flag `has_fulltext` per row without
    /// loading each `FullText` record. Local libraries are small, so one
    /// scan beats N point lookups.
    pub async fn list_fulltext_article_ids(&self) -> Result<std::collections::HashSet<String>> {
        let conn = self.conn();
        let mut rows = conn
            .query("SELECT article_id FROM fulltexts", turso::params![])
            .await?;
        let mut ids = std::collections::HashSet::new();
        while let Some(row) = rows.next().await? {
            ids.insert(row.get::<String>(0)?);
        }
        Ok(ids)
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
                        file_hash, file_size, uploaded_at, parse_status, parse_engine, parse_error, \
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
        let total_chars = row.get::<i64>(11)? as usize;
        let page_text = opt_string(row.get_value(12)?);
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
            parse_status: parse_status_value(row.get_value(8)?),
            parse_engine: opt_string(row.get_value(9)?),
            parse_error: opt_string(row.get_value(10)?),
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

    /// Update only the parse-pipeline columns of a full-text row.
    ///
    /// Used by the async MinerU pipeline for its pending → processing →
    /// done/failed state machine; `text_content` and the FTS index are left
    /// untouched so a previously parsed text stays readable (and searchable)
    /// while a re-parse runs, and a failed first parse simply leaves the row
    /// un-indexed.
    pub async fn set_parse_status(
        &self,
        article_id: &str,
        status: &str,
        error: Option<&str>,
    ) -> Result<()> {
        let _write = self.write_gate.lock().await;
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        tx.execute(
            "UPDATE fulltexts \
             SET parse_status = ?2, parse_error = ?3 \
             WHERE article_id = ?1",
            turso::params![article_id, status, error],
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Parse status of every stored full text (small table, full scan).
    ///
    /// The web list view joins this client-side over `/articles`, mirroring
    /// `/collections` and `/journals/metrics`.
    pub async fn list_parse_statuses(&self) -> Result<Vec<FullTextParseStatus>> {
        let conn = self.conn();
        let mut rows = conn
            .query(
                "SELECT article_id, parse_status, parse_engine, parse_error FROM fulltexts",
                turso::params![],
            )
            .await?;

        let mut statuses = Vec::new();
        while let Some(row) = rows.next().await? {
            statuses.push(FullTextParseStatus {
                article_id: row.get::<String>(0)?,
                parse_status: parse_status_value(row.get_value(1)?),
                parse_engine: opt_string(row.get_value(2)?),
                parse_error: opt_string(row.get_value(3)?),
            });
        }
        Ok(statuses)
    }
}

/// Parse-pipeline columns of one full-text row (list-view projection).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FullTextParseStatus {
    pub article_id: String,
    pub parse_status: String,
    pub parse_engine: Option<String>,
    pub parse_error: Option<String>,
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

/// `parse_status` column → struct field; NULL (legacy pre-migration rows)
/// reads as `done`.
fn parse_status_value(v: Value) -> String {
    match v {
        Value::Text(s) if !s.is_empty() => s,
        _ => "done".to_owned(),
    }
}
