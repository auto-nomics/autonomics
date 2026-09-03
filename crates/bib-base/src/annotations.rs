//! Annotation CRUD — notes, highlights, and comments attached to articles.
//!
//! All methods are on [`BibBase`] and share the same Turso connection.

use chrono::Utc;
use turso::{
    Value,
    transaction::{Transaction, TransactionBehavior},
};

use crate::bib_base::BibBase;
use crate::error::Result;
use crate::types::{Annotation, AnnotationKind};

impl BibBase {
    /// Add an annotation to an article.
    pub async fn add_annotation(
        &self,
        article_id: &str,
        kind: AnnotationKind,
        content: &str,
        page: Option<u32>,
        data: Option<serde_json::Value>,
    ) -> Result<Annotation> {
        let id = format!("ann-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let now = Utc::now().to_rfc3339();
        // Encoded up front so a serialisation failure is reported instead of
        // silently storing NULL.
        let encoded_data = data
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;

        let _write = self.write_gate.lock().await;
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        tx.execute(
            "INSERT INTO annotations (id, article_id, kind, content, page, data, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            turso::params![
                id.clone(),
                article_id,
                kind.as_str(),
                content,
                page.map(|p| p as i64),
                encoded_data,
                now,
            ],
        )
        .await?;
        crate::bib_base::sync_search_index(&tx, article_id).await?;
        tx.commit().await?;

        Ok(Annotation {
            id,
            kind,
            content: content.to_owned(),
            page,
            data,
            created_at: Some(Utc::now()),
        })
    }

    /// List all annotations for an article, ordered by creation time.
    pub async fn list_annotations(&self, article_id: &str) -> Result<Vec<Annotation>> {
        let conn = self.conn();
        let mut rows = conn
            .query(
                "SELECT id, kind, content, page, data, created_at \
                 FROM annotations WHERE article_id = ?1 \
                 ORDER BY created_at",
                turso::params![article_id],
            )
            .await?;

        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            out.push(Annotation {
                id: row.get::<String>(0)?,
                kind: AnnotationKind::from_str(&row.get::<String>(1)?),
                content: row.get::<String>(2)?,
                page: opt_int(row.get_value(3)?).map(|p| p as u32),
                data: parse_json_value(&opt_string(row.get_value(4)?)),
                created_at: opt_string(row.get_value(5)?).as_deref().and_then(parse_dt),
            });
        }
        Ok(out)
    }

    /// Update the editable fields of an annotation, leaving every field the
    /// caller passed as `None` untouched.
    ///
    /// `page`/`data` are doubly wrapped so a client can distinguish "leave
    /// as is" (`None`) from "clear it" (`Some(None)`); annotation content is
    /// searchable, so a content change also refreshes the search index.
    ///
    /// Returns `None` when the annotation does not exist — callers turn that
    /// into a 404 instead of guessing whether the write landed.
    pub async fn update_annotation(
        &self,
        id: &str,
        content: Option<&str>,
        page: Option<Option<u32>>,
        data: Option<Option<serde_json::Value>>,
    ) -> Result<Option<Annotation>> {
        let _write = self.write_gate.lock().await;
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;

        let current = {
            let mut rows = tx
                .query(
                    "SELECT article_id, kind, content, page, data, created_at \
                     FROM annotations WHERE id = ?1",
                    turso::params![id],
                )
                .await?;
            match rows.next().await? {
                Some(row) => Some((
                    row.get::<String>(0)?,
                    AnnotationKind::from_str(&row.get::<String>(1)?),
                    row.get::<String>(2)?,
                    opt_int(row.get_value(3)?).map(|p| p as u32),
                    parse_json_value(&opt_string(row.get_value(4)?)),
                    opt_string(row.get_value(5)?).as_deref().and_then(parse_dt),
                )),
                None => None,
            }
        };

        let Some((article_id, kind, current_content, current_page, current_data, created_at)) =
            current
        else {
            // Nothing to update; roll back so the open transaction is not
            // left dangling on the shared writer connection.
            let _ = tx.rollback().await;
            return Ok(None);
        };

        let new_content = content.map(str::to_owned).unwrap_or(current_content);
        let new_page = page.unwrap_or(current_page);
        let new_data = data.unwrap_or(current_data);
        let encoded_data = new_data
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;

        tx.execute(
            "UPDATE annotations SET content = ?1, page = ?2, data = ?3 WHERE id = ?4",
            turso::params![
                new_content.clone(),
                new_page.map(|p| p as i64),
                encoded_data,
                id,
            ],
        )
        .await?;

        // Only re-tokenise when the indexed text actually moved; page and
        // geometry changes never affect the search index.
        if content.is_some() {
            crate::bib_base::sync_search_index(&tx, &article_id).await?;
        }
        tx.commit().await?;

        Ok(Some(Annotation {
            id: id.to_owned(),
            kind,
            content: new_content,
            page: new_page,
            data: new_data,
            created_at,
        }))
    }

    /// Delete an annotation by ID.
    pub async fn delete_annotation(&self, id: &str) -> Result<()> {
        let _write = self.write_gate.lock().await;
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let mut rows = tx
            .query(
                "SELECT article_id FROM annotations WHERE id = ?1",
                turso::params![id],
            )
            .await?;
        let article_id = rows
            .next()
            .await?
            .map(|row| row.get::<String>(0))
            .transpose()?;
        drop(rows);
        if let Some(article_id) = article_id {
            tx.execute("DELETE FROM annotations WHERE id = ?1", turso::params![id])
                .await?;
            crate::bib_base::sync_search_index(&tx, &article_id).await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

fn opt_string(v: Value) -> Option<String> {
    match v {
        Value::Text(s) => Some(s),
        _ => None,
    }
}

/// Decode the JSON `data` column.
///
/// A malformed payload degrades to `None` rather than failing the whole
/// listing: the geometry is a render-time nicety, and losing one note's
/// highlight boxes is better than returning 500 for every annotation on the
/// page.
fn parse_json_value(raw: &Option<String>) -> Option<serde_json::Value> {
    raw.as_ref().and_then(|s| serde_json::from_str(s).ok())
}

fn opt_int(v: Value) -> Option<i64> {
    match v {
        Value::Integer(i) => Some(i),
        _ => None,
    }
}

fn parse_dt(s: &str) -> Option<chrono::DateTime<Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}
