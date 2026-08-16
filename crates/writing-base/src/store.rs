//! Turso (libSQL) storage for writing documents, versions, and templates.

use std::sync::Arc;

use chrono::Utc;
use turso::{Builder, Connection};

use writing_types::Document;

use crate::{Error, Result};

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

const SCHEMA_SQL: &str = "\
CREATE TABLE IF NOT EXISTS documents (
    id          TEXT PRIMARY KEY,
    title       TEXT NOT NULL,
    data        TEXT NOT NULL,
    version     INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS document_versions (
    id          TEXT PRIMARY KEY,
    document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    version     INTEGER NOT NULL,
    data        TEXT NOT NULL,
    author      TEXT,
    message     TEXT,
    created_at  TEXT NOT NULL,
    UNIQUE(document_id, version)
);

CREATE INDEX IF NOT EXISTS idx_doc_versions
    ON document_versions(document_id, version);
";

// ---------------------------------------------------------------------------
// WritingStore
// ---------------------------------------------------------------------------

/// Document database handle.
///
/// Wraps a single turso [`Connection`]. Because `Connection` is cheaply
/// cloneable (internally an `Arc`), every method takes `&self`.
#[derive(Clone)]
pub struct WritingStore {
    conn: Connection,
}

/// Lightweight document summary for list views.
#[derive(Debug, Clone)]
pub struct DocumentSummary {
    pub id: String,
    pub title: String,
    pub version: u32,
    pub created_at: String,
    pub updated_at: String,
}

/// Lightweight version summary.
#[derive(Debug, Clone)]
pub struct VersionSummary {
    pub version: u32,
    pub author: Option<String>,
    pub message: Option<String>,
    pub created_at: String,
}

impl WritingStore {
    /// Open (or create) a local SQLite database file.
    pub async fn open(path: impl AsRef<str>) -> Result<Self> {
        let path = path.as_ref();
        let mut builder = Builder::new_local(path);
        if path != ":memory:" {
            builder = builder.experimental_multiprocess_wal(true);
        }
        let db = builder.build().await?;
        let conn = db.connect()?;
        if path != ":memory:" {
            conn.pragma_update("busy_timeout", 5000).await?;
        }
        let store = Self { conn };
        store.migrate().await?;
        Ok(store)
    }

    /// Open an in-memory database (for tests).
    pub async fn open_in_memory() -> Result<Self> {
        Self::open(":memory:").await
    }

    /// Run migrations (idempotent).
    pub async fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(SCHEMA_SQL).await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Document CRUD
    // -----------------------------------------------------------------------

    /// Create a new document.
    pub async fn create_document(&self, id: &str, title: &str) -> Result<Document> {
        let mut doc = Document::new(id, title);
        self.save_document(&mut doc, "create", None).await?;
        Ok(doc)
    }

    /// Save a document, creating a version snapshot.
    ///
    /// Increments `version` and stores the current state as an immutable
    /// version snapshot.
    pub async fn save_document(
        &self,
        doc: &mut Document,
        message: &str,
        author: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        doc.version += 1;
        doc.updated_at = Some(Utc::now());
        let data = serde_json::to_string(doc)?;

        let conn = self.conn.clone();

        // Upsert the document row.
        conn.execute(
            "INSERT OR REPLACE INTO documents (id, title, data, version, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            turso::params![
                doc.id.clone(),
                doc.title.clone(),
                data.clone(),
                doc.version as i64,
                doc.created_at
                    .map(|t| t.to_rfc3339())
                    .unwrap_or_else(|| now.clone()),
                now.clone(),
            ],
        )
        .await?;

        // Snapshot into versions table.
        let version_id = format!("{}_v{}", doc.id, doc.version);
        conn.execute(
            "INSERT OR REPLACE INTO document_versions \
             (id, document_id, version, data, author, message, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            turso::params![
                version_id,
                doc.id.clone(),
                doc.version as i64,
                data,
                author.map(|s| s.to_string()),
                message,
                now,
            ],
        )
        .await?;

        Ok(())
    }

    /// Get a document by ID.
    pub async fn get_document(&self, id: &str) -> Result<Document> {
        let conn = self.conn.clone();
        let mut rows = conn
            .query(
                "SELECT data FROM documents WHERE id = ?1",
                turso::params![id],
            )
            .await?;

        let row = match rows.next().await? {
            Some(row) => row,
            None => return Err(Error::NotFound(format!("document {id}"))),
        };

        let data: String = row.get(0)?;
        let doc: Document = serde_json::from_str(&data)?;
        Ok(doc)
    }

    /// List all document summaries.
    pub async fn list_documents(&self) -> Result<Vec<DocumentSummary>> {
        let conn = self.conn.clone();
        let mut rows = conn
            .query(
                "SELECT id, title, version, created_at, updated_at FROM documents \
                 ORDER BY updated_at DESC",
                turso::params![],
            )
            .await?;

        let mut summaries = Vec::new();
        while let Some(row) = rows.next().await? {
            summaries.push(DocumentSummary {
                id: row.get::<String>(0)?,
                title: row.get::<String>(1)?,
                version: row.get::<i64>(2)? as u32,
                created_at: row.get::<String>(3)?,
                updated_at: row.get::<String>(4)?,
            });
        }
        Ok(summaries)
    }

    /// Delete a document and all its versions.
    pub async fn delete_document(&self, id: &str) -> Result<()> {
        let conn = self.conn.clone();
        // Explicitly delete versions first (cascade may not fire in all turso configs).
        conn.execute(
            "DELETE FROM document_versions WHERE document_id = ?1",
            turso::params![id],
        )
        .await?;
        conn.execute("DELETE FROM documents WHERE id = ?1", turso::params![id])
            .await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Version history
    // -----------------------------------------------------------------------

    /// List all version snapshots for a document.
    pub async fn list_versions(&self, doc_id: &str) -> Result<Vec<VersionSummary>> {
        let conn = self.conn.clone();
        let mut rows = conn
            .query(
                "SELECT version, author, message, created_at FROM document_versions \
                 WHERE document_id = ?1 ORDER BY version DESC",
                turso::params![doc_id],
            )
            .await?;

        let mut versions = Vec::new();
        while let Some(row) = rows.next().await? {
            versions.push(VersionSummary {
                version: row.get::<i64>(0)? as u32,
                author: row.get_value(1)?.as_text().map(|s| s.to_string()),
                message: row.get_value(2)?.as_text().map(|s| s.to_string()),
                created_at: row.get::<String>(3)?,
            });
        }
        Ok(versions)
    }

    /// Get a specific version of a document.
    pub async fn get_version(&self, doc_id: &str, version: u32) -> Result<Document> {
        let conn = self.conn.clone();
        let mut rows = conn
            .query(
                "SELECT data FROM document_versions WHERE document_id = ?1 AND version = ?2",
                turso::params![doc_id, version as i64],
            )
            .await?;

        let row = match rows.next().await? {
            Some(row) => row,
            None => {
                return Err(Error::NotFound(format!(
                    "version {version} of document {doc_id}"
                )));
            }
        };

        let data: String = row.get(0)?;
        let doc: Document = serde_json::from_str(&data)?;
        Ok(doc)
    }

    /// Check if a document exists.
    pub async fn exists(&self, id: &str) -> Result<bool> {
        let conn = self.conn.clone();
        let mut rows = conn
            .query(
                "SELECT 1 FROM documents WHERE id = ?1 LIMIT 1",
                turso::params![id],
            )
            .await?;
        Ok(rows.next().await?.is_some())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use writing_types::{Block, EditOp, EditScript, Inline, ParagraphBlock, Section, SectionLevel};

    async fn setup() -> WritingStore {
        WritingStore::open_in_memory().await.unwrap()
    }

    #[tokio::test]
    async fn create_and_get_document() {
        let store = setup().await;
        store.create_document("d1", "My Paper").await.unwrap();

        let loaded = store.get_document("d1").await.unwrap();
        assert_eq!(loaded.id, "d1");
        assert_eq!(loaded.title, "My Paper");
        assert_eq!(loaded.version, 1);
    }

    #[tokio::test]
    async fn get_nonexistent_document() {
        let store = setup().await;
        assert!(store.get_document("nonexistent").await.is_err());
    }

    #[tokio::test]
    async fn save_and_version() {
        let store = setup().await;
        let mut doc = store.create_document("d1", "Paper").await.unwrap();

        // Add a section.
        let sec = Section::new(SectionLevel::Section, "Introduction");
        let sec_id = sec.id.clone();
        crate::ast::apply_edit(
            &mut doc,
            &EditOp::InsertSection {
                parent_id: None,
                after_section: None,
                section: sec,
            },
        )
        .unwrap();
        store
            .save_document(&mut doc, "add intro", None)
            .await
            .unwrap();

        // Add a paragraph.
        crate::ast::apply_edit(
            &mut doc,
            &EditOp::InsertBlock {
                section_id: sec_id,
                after_block: None,
                block: Block::Paragraph(ParagraphBlock::text("Hello world.")),
            },
        )
        .unwrap();
        store
            .save_document(&mut doc, "add paragraph", None)
            .await
            .unwrap();

        // Three versions should exist.
        let versions = store.list_versions("d1").await.unwrap();
        assert_eq!(versions.len(), 3); // create + 2 edits

        // Latest version should have the paragraph.
        let latest = store.get_document("d1").await.unwrap();
        assert_eq!(latest.root.children.len(), 1);
        assert_eq!(latest.root.children[0].blocks.len(), 1);

        // First version should be empty.
        let v1 = store.get_version("d1", 1).await.unwrap();
        assert_eq!(v1.root.children.len(), 0);
    }

    #[tokio::test]
    async fn list_documents() {
        let store = setup().await;
        store.create_document("d1", "Paper A").await.unwrap();
        store.create_document("d2", "Paper B").await.unwrap();

        let docs = store.list_documents().await.unwrap();
        assert_eq!(docs.len(), 2);
    }

    #[tokio::test]
    async fn delete_document() {
        let store = setup().await;
        store.create_document("d1", "Paper").await.unwrap();
        assert!(store.exists("d1").await.unwrap());

        store.delete_document("d1").await.unwrap();
        assert!(!store.exists("d1").await.unwrap());

        // Versions should also be gone.
        let versions = store.list_versions("d1").await.unwrap();
        assert!(versions.is_empty());
    }

    #[tokio::test]
    async fn version_retrieval() {
        let store = setup().await;
        let mut doc = store.create_document("d1", "Test").await.unwrap();

        doc.title = "Updated Title".into();
        store.save_document(&mut doc, "rename", None).await.unwrap();

        let v1 = store.get_version("d1", 1).await.unwrap();
        assert_eq!(v1.title, "Test");

        let v2 = store.get_version("d1", 2).await.unwrap();
        assert_eq!(v2.title, "Updated Title");
    }

    #[tokio::test]
    async fn version_message_and_author() {
        let store = setup().await;
        let mut doc = store.create_document("d1", "Test").await.unwrap();
        store
            .save_document(&mut doc, "first commit", Some("agent_writer"))
            .await
            .unwrap();

        let versions = store.list_versions("d1").await.unwrap();
        // Latest version should have the message.
        let latest = &versions[0];
        assert!(latest.message.as_deref() == Some("first commit"));
    }

    #[tokio::test]
    async fn full_edit_lifecycle() {
        let store = setup().await;
        store.create_document("d1", "Lifecycle Test").await.unwrap();

        // Apply a complex edit script.
        let mut doc = store.get_document("d1").await.unwrap();
        let script = EditScript::new()
            .op(EditOp::InsertSection {
                parent_id: None,
                after_section: None,
                section: Section::new(SectionLevel::Section, "Introduction"),
            })
            .op(EditOp::InsertSection {
                parent_id: None,
                after_section: None,
                section: Section::new(SectionLevel::Section, "Methods"),
            })
            .op(EditOp::InsertSection {
                parent_id: None,
                after_section: None,
                section: Section::new(SectionLevel::Section, "Results"),
            })
            .message("Initial structure");

        crate::ast::apply_edit_script(&mut doc, &script).unwrap();
        store
            .save_document(&mut doc, "Initial structure", None)
            .await
            .unwrap();

        // Verify.
        let loaded = store.get_document("d1").await.unwrap();
        assert_eq!(loaded.root.children.len(), 3);
        assert_eq!(loaded.root.children[0].title, "Introduction");
        assert_eq!(loaded.root.children[1].title, "Methods");
        assert_eq!(loaded.root.children[2].title, "Results");

        // Add content to Introduction.
        let intro_id = loaded.root.children[0].id.clone();
        let mut doc = store.get_document("d1").await.unwrap();
        crate::ast::apply_edit(
            &mut doc,
            &EditOp::InsertBlock {
                section_id: intro_id,
                after_block: None,
                block: Block::Paragraph(ParagraphBlock::new(vec![
                    Inline::text("Genome-wide association studies (GWAS) have "),
                    Inline::Citation(writing_types::CitationCluster {
                        keys: vec![writing_types::CiteKey::new("smith2024")],
                        style: writing_types::CitationStyle::Parenthetical,
                        prefix: None,
                        suffix: None,
                        claim_id: None,
                    }),
                    Inline::text(" identified thousands of loci."),
                ])),
            },
        )
        .unwrap();
        store
            .save_document(&mut doc, "add intro paragraph", None)
            .await
            .unwrap();

        // Verify citation was stored.
        let loaded = store.get_document("d1").await.unwrap();
        let keys = crate::ast::all_cite_keys(&loaded);
        assert_eq!(keys, vec!["smith2024"]);
    }
}
