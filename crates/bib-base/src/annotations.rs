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
use crate::error::{Error, Result};
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
        self.insert_annotation(AnnotationDraft {
            article_id,
            kind,
            content,
            comment: None,
            tags: Vec::new(),
            page,
            color,
            rects,
            attachment_key,
        })
        .await
    }

    /// Add an agent annotation anchored to a verified quote of the
    /// canonical text.
    ///
    /// The `quote` must exist in the article's extracted full text
    /// (whitespace-folded comparison, everything else exact) — the anchor
    /// is refused otherwise, with a closest-partial-match hint or the list
    /// of ambiguous locations. Notes and comments fall back to anchoring
    /// the abstract when no full text is stored; highlights require the
    /// full text. On success the quote's exact byte range is persisted as
    /// an anchor proof inside `rects[0]`, and the rect itself carries
    /// UTF-16 `space:"char"` offsets the GUI text reader renders directly.
    pub async fn add_anchored_annotation(
        &self,
        input: AnchoredNoteInput,
    ) -> Result<crate::types::AnchoredAnnotation> {
        use crate::anchor::{LineIndex, QuoteLocator, collapse_context, resolve_anchor, utf16_offset};

        if input.quote.trim().is_empty() {
            return Err(Error::Anchor(
                "`quote` is required: copy the passage verbatim from `vfs read`/`grep` \
                 of the article's fulltext_path so the note can anchor to it"
                    .into(),
            ));
        }

        // Canonical text: the extracted full text; notes and comments may
        // fall back to the abstract, highlights may not (a highlight not
        // grounded in the real text would be fabrication).
        let fulltext = self.get_fulltext(&input.article_id).await?;
        let canonical = fulltext
            .and_then(|ft| ft.text_content)
            .filter(|text| !text.trim().is_empty());
        let (source, text) = match canonical {
            Some(text) => ("fulltext", text),
            None if input.kind != AnnotationKind::Highlight => {
                let article = self
                    .get_article(&input.article_id)
                    .await?
                    .ok_or_else(|| Error::NotFound(input.article_id.clone()))?;
                match article.abstract_text.filter(|a| !a.trim().is_empty()) {
                    Some(abstract_text) => ("abstract", abstract_text),
                    None => {
                        return Err(Error::Anchor(format!(
                            "article '{}' has neither extracted full text nor an abstract to \
                             anchor to; save it with a `bib_save` DAG node and attach the file",
                            input.article_id
                        )));
                    }
                }
            }
            None => {
                return Err(Error::Anchor(format!(
                    "kind=highlight requires the article's full text, but none is stored for \
                     '{}'; request it with bib_request_fulltext or upload the file, or use \
                     kind=note to anchor to the abstract instead",
                    input.article_id
                )));
            }
        };

        // Locate the quote; every refusal tells the agent how to fix its
        // input instead of leaving it guessing.
        let locator = QuoteLocator::new(&text);
        let matches = locator.find_all(input.quote.trim(), MAX_QUOTE_HITS);
        let matched = match matches.len() {
            0 => {
                let hint = locator.longest_prefix_match(input.quote.trim(), MIN_HINT_CHARS);
                return Err(Error::Anchor(not_found_message(
                    source,
                    &text,
                    hint,
                )));
            }
            1 => matches[0],
            _ if input.near_line.is_none() => {
                return Err(Error::Anchor(ambiguous_message(&text, &matches)));
            }
            _ => {
                let lines = LineIndex::new(&text);
                let near = input.near_line.expect("checked above") as i64;
                *matches
                    .iter()
                    .min_by_key(|matched| {
                        (lines.line_of_byte(matched.start_byte) as i64 - near).abs()
                    })
                    .expect("non-empty")
            }
        };
        let resolved = resolve_anchor(source, &text, matched);

        let note = input.note.as_deref().map(str::trim).filter(|n| !n.is_empty());
        let (content, comment) = match input.kind {
            AnnotationKind::Highlight => {
                let note = note.ok_or_else(|| {
                    Error::Anchor(
                        "kind=highlight requires `note`: the comment explaining why this \
                         passage is highlighted (the quote itself is stored as the text)"
                            .into(),
                    )
                })?;
                (resolved.quote.clone(), Some(note.to_owned()))
            }
            AnnotationKind::Note | AnnotationKind::Comment => {
                let note = note.ok_or_else(|| {
                    Error::Anchor(format!(
                        "kind={} requires `note` (the annotation body); `quote` only anchors \
                         it to the source text",
                        input.kind.as_str()
                    ))
                })?;
                (note.to_owned(), None)
            }
        };

        // `space:"char"` rects in UTF-16 units render in the GUI text
        // reader; the anchor proof records the verified byte range.
        let utf16_start = utf16_offset(&text, matched.start_byte);
        let utf16_end = utf16_offset(&text, matched.end_byte);
        let rects = vec![crate::types::AnnotationRect {
            left: utf16_start as f64,
            top: 0.0,
            width: (utf16_end - utf16_start) as f64,
            height: 0.0,
            space: Some("char".into()),
            text_range: None,
            anchor: Some(crate::types::AnnotationAnchorProof {
                source_hash: resolved.source_hash.clone(),
                quote: resolved.quote.clone(),
                start_byte: matched.start_byte,
                end_byte: matched.end_byte,
            }),
        }];

        let annotation = self
            .insert_annotation(AnnotationDraft {
                article_id: &input.article_id,
                kind: input.kind,
                content: &content,
                comment: comment.as_deref(),
                tags: input.tags,
                // GUI text-reader annotations are page 1 by convention.
                page: Some(1),
                color: input.color,
                rects: Some(rects),
                attachment_key: input.attachment_key.as_deref(),
            })
            .await?;
        Ok(crate::types::AnchoredAnnotation {
            annotation,
            anchor: resolved,
        })
    }

    /// Shared insert core for the plain and anchored paths: one
    /// transaction writing the annotation row, its Zotero mirror, and the
    /// search-index bump.
    async fn insert_annotation(&self, draft: AnnotationDraft<'_>) -> Result<Annotation> {
        let id = format!("ann-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let now = Utc::now().to_rfc3339();
        let tags = normalize_tags(draft.tags);
        let rect_json = draft
            .rects
            .as_ref()
            .and_then(|rects| serde_json::to_string(rects).ok());

        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        tx.execute(
            "INSERT INTO annotations (id, article_id, kind, content, page, color, rects, comment, tags, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            turso::params![
                id.clone(),
                draft.article_id,
                draft.kind.as_str(),
                draft.content,
                draft.page.map(|p| p as i64),
                draft.color.clone(),
                rect_json,
                draft.comment.unwrap_or_default(),
                serde_json::to_string(&tags)?,
                now.clone(),
            ],
        )
        .await?;
        let annotation_model = crate::zotero_model::AnnotationSync {
            id: id.clone(),
            article_id: draft.article_id.to_owned(),
            kind: draft.kind,
            content: draft.content.to_owned(),
            page: draft.page,
            color: draft.color.clone(),
            rects: draft.rects.clone(),
            comment: draft.comment.map(str::to_owned),
            created_at: now,
        };
        let item_id =
            crate::zotero_model::sync_annotation_item(&tx, &annotation_model, draft.attachment_key)
                .await?;
        let item_key = crate::zotero_model::item_key(&tx, item_id).await?;
        let attachment_key = match draft.attachment_key {
            Some(key) => Some(key.to_owned()),
            None => crate::zotero_model::annotation_attachment_key(&tx, &id).await?,
        };
        crate::bib_base::sync_search_index(&tx, draft.article_id).await?;
        tx.commit().await?;

        Ok(Annotation {
            id,
            kind: draft.kind,
            content: draft.content.to_owned(),
            comment: draft.comment.unwrap_or_default().to_owned(),
            tags,
            page: draft.page,
            color: draft.color.clone(),
            rects: draft.rects.clone(),
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
// Anchored-note helpers
// ---------------------------------------------------------------------------

/// Everything [`BibCore::insert_annotation`] needs in one place.
struct AnnotationDraft<'a> {
    article_id: &'a str,
    kind: AnnotationKind,
    content: &'a str,
    comment: Option<&'a str>,
    tags: Vec<String>,
    page: Option<u32>,
    color: Option<String>,
    rects: Option<Vec<crate::types::AnnotationRect>>,
    attachment_key: Option<&'a str>,
}

/// Input for [`BibCore::add_anchored_annotation`].
pub struct AnchoredNoteInput {
    pub article_id: String,
    pub kind: AnnotationKind,
    /// Verbatim passage from the canonical text the note anchors to.
    pub quote: String,
    /// The agent's annotation body (highlight comment, note/comment text).
    pub note: Option<String>,
    /// 1-based line hint used to disambiguate repeated quotes.
    pub near_line: Option<u32>,
    pub color: Option<String>,
    pub tags: Vec<String>,
    pub attachment_key: Option<String>,
}

/// Cap on tracked occurrences of one quote; enough to describe any real
/// ambiguity without unbounded work on degenerate quotes.
const MAX_QUOTE_HITS: usize = 64;

/// Smallest prefix still worth reporting as a partial-match hint.
const MIN_HINT_CHARS: usize = 4;

fn normalize_tags(tags: Vec<String>) -> Vec<String> {
    tags.into_iter()
        .map(|tag| tag.trim().to_owned())
        .filter(|tag| !tag.is_empty())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn not_found_message(source: &str, text: &str, hint: Option<crate::anchor::Match>) -> String {
    use crate::anchor::{LineIndex, collapse_context};
    let lines = LineIndex::new(text);
    let mut message = format!(
        "quote not found in the article's {source}; copy it verbatim from `vfs read`/`grep` \
         of the article's fulltext_path (whitespace may differ, wording may not)"
    );
    match hint {
        Some(hint) => {
            let line = lines.line_of_byte(hint.start_byte);
            message.push_str(&format!(
                ". The text matching the first words of the quote is at line {line}: \"{}\"",
                collapse_context(text, hint.start_byte, hint.end_byte)
            ));
        }
        None => {
            message.push_str(&format!(
                ". The {source} has {} lines; re-read the relevant part first",
                lines.line_count()
            ));
        }
    }
    message
}

fn ambiguous_message(text: &str, matches: &[crate::anchor::Match]) -> String {
    use crate::anchor::LineIndex;
    let lines = LineIndex::new(text);
    let shown = matches
        .iter()
        .take(10)
        .map(|matched| format!("line {}", lines.line_of_byte(matched.start_byte)))
        .collect::<Vec<_>>()
        .join(", ");
    let more = if matches.len() > 10 {
        format!(" (and {} more)", matches.len() - 10)
    } else {
        String::new()
    };
    format!(
        "quote matches {} locations — pass `near_line` (the 1-based line of the quote in the \
         text) to disambiguate: {shown}{more}",
        matches.len()
    )
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

    /// Add an agent annotation anchored to a verified quote of the
    /// canonical text.
    pub async fn add_anchored_annotation(
        &self,
        input: AnchoredNoteInput,
    ) -> Result<crate::types::AnchoredAnnotation> {
        self.call(|reply| BibCommand::AddAnchoredAnnotation { input, reply })
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

    // -----------------------------------------------------------------------
    // add_anchored_annotation
    // -----------------------------------------------------------------------

    async fn anchored_fixture(text: &str) -> BibCore {
        let core = BibCore::open(":memory:").await.unwrap();
        core.upsert_article(&Article::new("test", "Anchored note"))
            .await
            .unwrap();
        core.upsert_fulltext(&FullText {
            article_id: "test".into(),
            attachment_key: None,
            file_path: "vfs:///paper.pdf".into(),
            file_format: FileFormat::Pdf,
            text_content: Some(text.into()),
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
        core
    }

    fn anchored_input(kind: AnnotationKind, quote: &str, note: &str) -> AnchoredNoteInput {
        AnchoredNoteInput {
            article_id: "test".into(),
            kind,
            quote: quote.into(),
            note: Some(note.into()),
            near_line: None,
            color: None,
            tags: Vec::new(),
            attachment_key: None,
        }
    }

    #[tokio::test]
    async fn anchored_note_verifies_quote_and_records_proof() {
        let text = "前置句。\n方法显著降低了\n死亡率。";
        let core = anchored_fixture(text).await;

        // The quote copied across the line break matches by folding; CJK
        // bytes and code points agree here so the UTF-16 rect is checkable.
        let anchored = core
            .add_anchored_annotation(anchored_input(
                AnnotationKind::Note,
                "方法显著降低了 死亡率。",
                "efficacy signal",
            ))
            .await
            .unwrap();

        assert_eq!(anchored.annotation.kind, AnnotationKind::Note);
        assert_eq!(anchored.annotation.content, "efficacy signal");
        assert_eq!(anchored.anchor.source, "fulltext");
        assert_eq!(anchored.anchor.quote, "方法显著降低了\n死亡率。");
        assert_eq!(anchored.anchor.line_start, 2);
        assert_eq!(anchored.anchor.line_end, 3);
        assert_eq!(
            &text[anchored.anchor.start_byte..anchored.anchor.end_byte],
            anchored.anchor.quote
        );

        let rect = &anchored.annotation.rects.as_ref().expect("rects")[0];
        assert_eq!(rect.space.as_deref(), Some("char"));
        // "前置句。" is 4 BMP chars + newline → 5 UTF-16 units before the quote.
        assert_eq!(rect.left, 5.0);
        assert_eq!(rect.width, 12.0); // 7 + 4 CJK chars + folded newline, 1 unit each
        let proof = rect.anchor.as_ref().expect("anchor proof");
        assert_eq!(proof.quote, anchored.anchor.quote);
        assert_eq!(proof.start_byte, anchored.anchor.start_byte);
        assert_eq!(proof.end_byte, anchored.anchor.end_byte);
        assert_eq!(proof.source_hash, anchored.anchor.source_hash);

        // Note bodies are searchable, tags persisted on the row.
        let hits = core.search_articles("efficacy signal", 10, 0).await.unwrap();
        assert_eq!(hits[0].article_id, "test");
        let listed = core.list_annotations("test", None).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].content, "efficacy signal");
    }

    #[tokio::test]
    async fn anchored_highlight_stores_verified_text_with_comment() {
        let text = "the treatment reduced mortality overall";
        let core = anchored_fixture(text).await;

        let anchored = core
            .add_anchored_annotation(anchored_input(
                AnnotationKind::Highlight,
                "reduced mortality",
                "key efficacy finding",
            ))
            .await
            .unwrap();

        // Highlight content is the verified verbatim slice, note → comment.
        assert_eq!(anchored.annotation.content, "reduced mortality");
        assert_eq!(anchored.annotation.comment, "key efficacy finding");

        // Zotero mirror: itemAnnotations.text = quote, comment = the note.
        let conn = core.conn();
        let mut rows = conn
            .query(
                "SELECT IA.text, IA.comment FROM itemAnnotations IA \
                 JOIN annotationItems M ON M.itemID = IA.itemID \
                 WHERE M.annotation_id = ?1",
                turso::params![anchored.annotation.id.clone()],
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "reduced mortality");
        assert_eq!(row.get::<String>(1).unwrap(), "key efficacy finding");
        drop(rows);
    }

    #[tokio::test]
    async fn anchored_note_falls_back_to_abstract_but_highlight_refuses() {
        let core = BibCore::open(":memory:").await.unwrap();
        let mut article = Article::new("test", "Abstract only");
        article.abstract_text = Some("Mortality declined after the intervention began.".into());
        core.upsert_article(&article).await.unwrap();

        let note = core
            .add_anchored_annotation(anchored_input(
                AnnotationKind::Note,
                "Mortality declined",
                "from the abstract",
            ))
            .await
            .unwrap();
        assert_eq!(note.anchor.source, "abstract");

        let error = core
            .add_anchored_annotation(anchored_input(
                AnnotationKind::Highlight,
                "Mortality declined",
                "no fulltext",
            ))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("requires the article's full text"));

        // Neither text nor abstract → both kinds refuse.
        let bare = BibCore::open(":memory:").await.unwrap();
        bare.upsert_article(&Article::new("bare", "No text")).await.unwrap();
        let error = bare
            .add_anchored_annotation(AnchoredNoteInput {
                article_id: "bare".into(),
                kind: AnnotationKind::Note,
                quote: "anything".into(),
                note: Some("body".into()),
                near_line: None,
                color: None,
                tags: Vec::new(),
                attachment_key: None,
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("neither extracted full text"));
    }

    #[tokio::test]
    async fn anchored_quote_failures_are_actionable() {
        let text = "repeat here\nmiddle line\nrepeat here again";
        let core = anchored_fixture(text).await;

        // Fabricated wording with a real prefix → closest-partial hint.
        let error = core
            .add_anchored_annotation(anchored_input(
                AnnotationKind::Note,
                "repeat here midX",
                "drifted",
            ))
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("not found"), "{message}");
        assert!(message.contains("first words"), "{message}");
        assert!(message.contains("line 1"), "{message}");

        // Repeated quote without a hint → the ambiguous-locations list.
        let error = core
            .add_anchored_annotation(anchored_input(
                AnnotationKind::Note,
                "repeat here",
                "ambiguous",
            ))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("matches 2 locations"));

        // With near_line the nearest occurrence wins.
        let anchored = core
            .add_anchored_annotation(AnchoredNoteInput {
                near_line: Some(3),
                ..anchored_input(AnnotationKind::Note, "repeat here", "disambiguated")
            })
            .await
            .unwrap();
        assert_eq!(anchored.anchor.line_start, 3);
        assert_eq!(anchored.anchor.quote, "repeat here");

        // Highlight without a note is refused.
        let error = core
            .add_anchored_annotation(AnchoredNoteInput {
                note: None,
                ..anchored_input(AnnotationKind::Highlight, "middle line", "")
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("requires `note`"));
    }
}
