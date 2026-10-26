//! Annotation CRUD — notes, highlights, and comments attached to articles.
//!
//! Storage methods are on [`BibCore`] (the actor-owned implementation)
//! and reach callers through the [`BibBase`] handle delegators below.

use chrono::Utc;
use turso::{
    Value,
    transaction::{Transaction, TransactionBehavior},
};

use crate::actor::BibCommand;
use crate::bib_base::{BibBase, BibCore};
use crate::error::Result;
use crate::types::{Annotation, AnnotationKind};

impl BibCore {
    /// Add an annotation to an article.
    pub async fn add_annotation(
        &self,
        article_id: &str,
        kind: AnnotationKind,
        content: &str,
        page: Option<u32>,
        color: Option<String>,
        rects: Option<Vec<crate::types::AnnotationRect>>,
    ) -> Result<Annotation> {
        let id = format!("ann-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let now = Utc::now().to_rfc3339();
        let rect_json = rects
            .as_ref()
            .and_then(|rects| serde_json::to_string(rects).ok());

        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        tx.execute(
            "INSERT INTO annotations (id, article_id, kind, content, page, color, rects, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            turso::params![
                id.clone(),
                article_id,
                kind.as_str(),
                content,
                page.map(|p| p as i64),
                color.clone(),
                rect_json,
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
            color,
            rects,
            created_at: Some(Utc::now()),
        })
    }

    /// List all annotations for an article, ordered by creation time.
    pub async fn list_annotations(&self, article_id: &str) -> Result<Vec<Annotation>> {
        let conn = self.conn();
        let mut rows = conn
            .query(
                "SELECT id, kind, content, page, color, rects, created_at \
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
                color: opt_string(row.get_value(4)?),
                rects: opt_string(row.get_value(5)?)
                    .and_then(|value| serde_json::from_str(&value).ok()),
                created_at: opt_string(row.get_value(6)?).as_deref().and_then(parse_dt),
            });
        }
        Ok(out)
    }

    /// Delete an annotation by ID.
    pub async fn delete_annotation(&self, id: &str) -> Result<()> {
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

// ---------------------------------------------------------------------------
// BibBase — thin handle methods, one command round-trip each
// ---------------------------------------------------------------------------

impl BibBase {
    /// Add an annotation to an article.
    pub async fn add_annotation(
        &self,
        article_id: &str,
        kind: AnnotationKind,
        content: &str,
        page: Option<u32>,
        color: Option<String>,
        rects: Option<Vec<crate::types::AnnotationRect>>,
    ) -> Result<Annotation> {
        self.call(|reply| BibCommand::AddAnnotation {
            article_id: article_id.to_owned(),
            kind,
            content: content.to_owned(),
            page,
            color,
            rects,
            reply,
        })
        .await
    }

    /// List all annotations for an article, ordered by creation time.
    pub async fn list_annotations(&self, article_id: &str) -> Result<Vec<Annotation>> {
        self.call(|reply| BibCommand::ListAnnotations {
            article_id: article_id.to_owned(),
            reply,
        })
        .await
    }

    /// Delete an annotation by ID.
    pub async fn delete_annotation(&self, id: &str) -> Result<()> {
        self.call(|reply| BibCommand::DeleteAnnotation {
            id: id.to_owned(),
            reply,
        })
        .await
    }
}

fn opt_string(v: Value) -> Option<String> {
    match v {
        Value::Text(s) => Some(s),
        _ => None,
    }
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
