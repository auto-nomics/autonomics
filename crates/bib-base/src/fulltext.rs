//! Full-text storage — CRUD for the [`FullText`] table and FTS index
//! synchronization.
//!
//! PDFs and other document files are stored externally via
//! `OpendalFileStorage`; this table tracks the *path*, extracted
//! plain-text content, and metadata so that agents can search and
//! retrieve full text without touching the binary file.

use chrono::Utc;
use turso::{
    Value,
    transaction::{Transaction, TransactionBehavior},
};

use crate::bib_base::BibBase;
use crate::error::Result;
use bib_types::{FileFormat, FullText, FullTextSource};

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
              file_hash, file_size, uploaded_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
             ON CONFLICT(article_id) DO UPDATE SET \
                file_path = excluded.file_path, \
                file_format = excluded.file_format, \
                text_content = excluded.text_content, \
                source = excluded.source, \
                file_hash = excluded.file_hash, \
                file_size = excluded.file_size, \
                uploaded_at = excluded.uploaded_at",
                turso::params![
                    ft.article_id.clone(),
                    ft.file_path.clone(),
                    ft.file_format.as_str(),
                    ft.text_content.clone(),
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
                        source, file_hash, file_size, uploaded_at \
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
        }))
    }

    /// Delete the full-text record for an article.
    pub async fn delete_fulltext(&self, article_id: &str) -> Result<()> {
        let _write = self.write_gate.lock().await;
        self.write_conn()
            .execute(
                "DELETE FROM fulltexts WHERE article_id = ?1",
                turso::params![article_id],
            )
            .await?;
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
