//! Annotation CRUD — notes, highlights, and comments attached to articles.
//!
//! All methods are on [`BibBase`] and share the same Turso connection.

use chrono::Utc;
use turso::Value;

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
    ) -> Result<Annotation> {
        let id = format!("ann-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let now = Utc::now().to_rfc3339();

        let _write = self.write_gate.lock().await;
        self.write_conn()
            .execute(
                "INSERT INTO annotations (id, article_id, kind, content, page, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                turso::params![
                    id.clone(),
                    article_id,
                    kind.as_str(),
                    content,
                    page.map(|p| p as i64),
                    now,
                ],
            )
            .await?;

        Ok(Annotation {
            id,
            kind,
            content: content.to_owned(),
            page,
            created_at: Some(Utc::now()),
        })
    }

    /// List all annotations for an article, ordered by creation time.
    pub async fn list_annotations(&self, article_id: &str) -> Result<Vec<Annotation>> {
        let conn = self.conn();
        let mut rows = conn
            .query(
                "SELECT id, kind, content, page, created_at \
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
                created_at: opt_string(row.get_value(4)?).as_deref().and_then(parse_dt),
            });
        }
        Ok(out)
    }

    /// Delete an annotation by ID.
    pub async fn delete_annotation(&self, id: &str) -> Result<()> {
        let _write = self.write_gate.lock().await;
        self.write_conn()
            .execute("DELETE FROM annotations WHERE id = ?1", turso::params![id])
            .await?;
        Ok(())
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
