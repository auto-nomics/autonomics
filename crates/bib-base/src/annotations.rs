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
            comment: String::new(),
            tags: Vec::new(),
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
                    MI.key, PI.key, A.comment, A.tags \
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
                comment: row.get::<String>(9)?,
                tags: serde_json::from_str(&row.get::<String>(10)?)?,
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

    /// Update compatibility metadata and Zotero item metadata atomically.
    pub async fn update_annotation(
        &self,
        id: &str,
        patch: crate::types::AnnotationUpdate,
    ) -> Result<Option<Annotation>> {
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let outcome: Result<Option<String>> = async {
        let mut rows = tx.query("SELECT article_id FROM annotations WHERE id = ?1", turso::params![id]).await?;
        let Some(row) = rows.next().await? else { return Ok(None); };
        let article_id = row.get::<String>(0)?;
        drop(rows);
        if let Some(comment) = patch.comment {
            tx.execute("UPDATE annotations SET comment = ?2 WHERE id = ?1", turso::params![id, comment.clone()]).await?;
            tx.execute("UPDATE itemAnnotations SET comment = ?2, commentNormalized = ?3 WHERE itemID = (SELECT itemID FROM annotationItems WHERE annotation_id = ?1)", turso::params![id, comment.clone(), comment.to_lowercase()]).await?;
        }
        if let Some(color) = patch.color {
            tx.execute("UPDATE annotations SET color = ?2 WHERE id = ?1", turso::params![id, color.clone()]).await?;
            tx.execute("UPDATE itemAnnotations SET color = ?2 WHERE itemID = (SELECT itemID FROM annotationItems WHERE annotation_id = ?1)", turso::params![id, color]).await?;
        }
        if let Some(tags) = patch.tags {
            let tags: Vec<String> = tags.into_iter().map(|tag| tag.trim().to_owned()).filter(|tag| !tag.is_empty()).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
            tx.execute("UPDATE annotations SET tags = ?2 WHERE id = ?1", turso::params![id, serde_json::to_string(&tags)?]).await?;
            tx.execute("DELETE FROM itemTags WHERE itemID = (SELECT itemID FROM annotationItems WHERE annotation_id = ?1)", turso::params![id]).await?;
            for tag in tags {
                let tag_id = crate::zotero_model::tag_id(&tx, &tag).await?;
                tx.execute("INSERT INTO itemTags (itemID, tagID, type) SELECT itemID, ?2, 0 FROM annotationItems WHERE annotation_id = ?1", turso::params![id, tag_id]).await?;
            }
        }
        tx.execute("UPDATE items SET dateModified = ?2, clientDateModified = ?2, version = version + 1 WHERE itemID = (SELECT itemID FROM annotationItems WHERE annotation_id = ?1)", turso::params![id, Utc::now().to_rfc3339()]).await?;
        crate::bib_base::sync_search_index(&tx, &article_id).await?;
        Ok(Some(article_id))
        }.await;
        match outcome {
            Ok(Some(article_id)) => {
                tx.commit().await?;
                Ok(self
                    .list_annotations(&article_id, None)
                    .await?
                    .into_iter()
                    .find(|annotation| annotation.id == id))
            }
            Ok(None) => {
                tx.rollback().await?;
                Ok(None)
            }
            Err(error) => {
                let _ = tx.rollback().await;
                Err(error)
            }
        }
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

    pub async fn update_annotation(
        &self,
        id: &str,
        patch: crate::types::AnnotationUpdate,
    ) -> Result<Option<Annotation>> {
        self.call(|reply| BibCommand::UpdateAnnotation {
            id: id.to_owned(),
            patch,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AnnotationUpdate, Article, FileFormat, FullText, FullTextSource};

    #[tokio::test]
    async fn metadata_is_mirrored_into_zotero_items_without_replacing_highlight_text() {
        let core = BibCore::open(":memory:").await.unwrap();
        core.upsert_article(&Article::new("test", "Metadata"))
            .await
            .unwrap();
        core.upsert_fulltext(&FullText {
            article_id: "test".into(),
            attachment_key: None,
            file_path: "vfs:///paper.pdf".into(),
            file_format: FileFormat::Pdf,
            text_content: None,
            source: FullTextSource::UserUpload,
            file_hash: None,
            file_size: None,
            uploaded_at: None,
            extract_status: None,
            text_format: None,
            extracted_by: None,
            extract_error: None,
        })
        .await
        .unwrap();
        let annotation = core
            .add_annotation(
                "test",
                AnnotationKind::Highlight,
                "Original quoted text",
                Some(5),
                Some("yellow".into()),
                None,
                None,
            )
            .await
            .unwrap();
        core.update_annotation(
            &annotation.id,
            AnnotationUpdate {
                comment: Some("Interpretation".into()),
                tags: Some(vec!["brain".into()]),
                color: Some("#ffd400".into()),
            },
        )
        .await
        .unwrap()
        .unwrap();
        let hits = core.search_articles("Interpretation", 10, 0).await.unwrap();
        assert_eq!(hits[0].article_id, "test");
        assert!(hits[0].snippet.contains("Interpretation"));
        assert_eq!(
            core.search_articles("brain", 10, 0).await.unwrap()[0].article_id,
            "test"
        );
        let conn = core.conn();
        let mut rows = conn.query("SELECT IA.text, IA.comment, IA.commentNormalized, IA.color FROM itemAnnotations IA JOIN annotationItems M ON M.itemID = IA.itemID WHERE M.annotation_id = ?1", turso::params![annotation.id.clone()]).await.unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "Original quoted text");
        assert_eq!(row.get::<String>(1).unwrap(), "Interpretation");
        assert_eq!(row.get::<String>(2).unwrap(), "interpretation");
        assert_eq!(row.get::<String>(3).unwrap(), "#ffd400");
        drop(rows);
        let mut rows = conn.query("SELECT T.name FROM tags T JOIN itemTags IT ON IT.tagID = T.tagID JOIN annotationItems M ON M.itemID = IT.itemID WHERE M.annotation_id = ?1", turso::params![annotation.id]).await.unwrap();
        assert_eq!(
            rows.next()
                .await
                .unwrap()
                .unwrap()
                .get::<String>(0)
                .unwrap(),
            "brain"
        );
    }

    #[tokio::test]
    async fn older_annotation_schema_gains_metadata_columns_idempotently() {
        let directory =
            std::env::temp_dir().join(format!("autonomics-annotation-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("library.db");
        {
            let db = turso::Builder::new_local(path.to_str().unwrap())
                .build()
                .await
                .unwrap();
            let conn = db.connect().unwrap();
            conn.execute_batch("CREATE TABLE annotations (id TEXT PRIMARY KEY, article_id TEXT NOT NULL, kind TEXT NOT NULL, content TEXT NOT NULL, page INTEGER, color TEXT, rects TEXT, created_at TEXT);").await.unwrap();
        }
        for _ in 0..2 {
            let core = BibCore::open(path.to_str().unwrap()).await.unwrap();
            let conn = core.conn();
            let mut rows = conn
                .query("SELECT comment, tags FROM annotations", turso::params![])
                .await
                .unwrap();
            assert!(rows.next().await.unwrap().is_none());
        }
        let _ = std::fs::remove_dir_all(directory);
    }
}
