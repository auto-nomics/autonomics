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
        attachment_key: Option<&str>,
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
                now.clone(),
            ],
        )
        .await?;
        let annotation_model = crate::zotero_model::AnnotationSync {
            id: id.clone(),
            article_id: article_id.to_owned(),
            kind,
            content: content.to_owned(),
            page,
            color: color.clone(),
            rects: rects.clone(),
            created_at: now,
        };
        let item_id =
            crate::zotero_model::sync_annotation_item(&tx, &annotation_model, attachment_key)
                .await?;
        let item_key = crate::zotero_model::item_key(&tx, item_id).await?;
        let attachment_key = match attachment_key {
            Some(key) => Some(key.to_owned()),
            None => crate::zotero_model::annotation_attachment_key(&tx, &id).await?,
        };
        crate::bib_base::sync_search_index(&tx, &annotation_model.article_id).await?;
        tx.commit().await?;

        Ok(Annotation {
            id,
            kind,
            content: content.to_owned(),
            page,
            color,
            rects,
            item_key: Some(item_key),
            attachment_key,
            created_at: Some(Utc::now()),
        })
    }

    /// List all annotations for an article, ordered by creation time.
    pub async fn list_annotations(
        &self,
        article_id: &str,
        attachment_key: Option<&str>,
    ) -> Result<Vec<Annotation>> {
        let conn = self.conn();
        let sql = format!(
            "SELECT A.id, A.kind, A.content, A.page, A.color, A.rects, A.created_at, \
                    MI.key, PI.key \
             FROM annotations A \
             LEFT JOIN annotationItems MAP ON MAP.annotation_id = A.id \
             LEFT JOIN items MI ON MI.itemID = MAP.itemID \
             LEFT JOIN itemAnnotations IA ON IA.itemID = MAP.itemID \
             LEFT JOIN itemAttachments PA ON PA.itemID = IA.parentItemID \
             LEFT JOIN items PI ON PI.itemID = PA.itemID \
             WHERE A.article_id = ?1 {} \
             ORDER BY A.created_at",
            if attachment_key.is_some() {
                "AND PI.key = ?2"
            } else {
                ""
            }
        );
        let mut rows = conn
            .query(
                &sql,
                turso::params_from_iter(
                    [
                        Some(Value::Text(article_id.to_owned())),
                        attachment_key.map(|key| Value::Text(key.to_owned())),
                    ]
                    .into_iter()
                    .flatten(),
                ),
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
                item_key: opt_string(row.get_value(7)?),
                attachment_key: opt_string(row.get_value(8)?),
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
            crate::zotero_model::delete_annotation_item(&tx, id).await?;
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
        attachment_key: Option<&str>,
    ) -> Result<Annotation> {
        self.call(|reply| BibCommand::AddAnnotation {
            article_id: article_id.to_owned(),
            kind,
            content: content.to_owned(),
            page,
            color,
            rects,
            attachment_key: attachment_key.map(str::to_owned),
            reply,
        })
        .await
    }

    /// List all annotations for an article, ordered by creation time.
    pub async fn list_annotations(
        &self,
        article_id: &str,
        attachment_key: Option<&str>,
    ) -> Result<Vec<Annotation>> {
        self.call(|reply| BibCommand::ListAnnotations {
            article_id: article_id.to_owned(),
            attachment_key: attachment_key.map(str::to_owned),
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
