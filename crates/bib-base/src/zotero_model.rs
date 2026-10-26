//! Zotero-compatible item and attachment storage.
//!
//! The legacy `articles` and `fulltexts` tables remain the compatibility
//! surface while callers migrate. New writes mirror their data into the
//! Zotero-style tables here. `vfsPath` and extraction provenance columns are
//! deliberate extensions: they retain this project's VFS-backed object store
//! and extraction pipeline without turning a local path into a sync-hostile
//! database path.

use chrono::Utc;
use serde::Serialize;
use turso::Connection;
use turso::transaction::{Transaction, TransactionBehavior};

use crate::error::Result;
use bib_types::{ExtractStatus, FileFormat, FullText, TextFormat};

pub const DEFAULT_LIBRARY_ID: i64 = 1;
pub const ITEM_TYPE_JOURNAL_ARTICLE: i64 = 1;
pub const ITEM_TYPE_ATTACHMENT: i64 = 2;

pub const LINK_MODE_IMPORTED_FILE: i64 = 0;
pub const LINK_MODE_IMPORTED_URL: i64 = 1;
pub const LINK_MODE_LINKED_FILE: i64 = 2;
pub const LINK_MODE_LINKED_URL: i64 = 3;
pub const LINK_MODE_EMBEDDED_IMAGE: i64 = 4;

pub const FIELD_TITLE: i64 = 1;
pub const FIELD_ABSTRACT: i64 = 2;
pub const FIELD_DATE: i64 = 3;
pub const FIELD_JOURNAL: i64 = 4;
pub const FIELD_VOLUME: i64 = 5;
pub const FIELD_ISSUE: i64 = 6;
pub const FIELD_PAGES: i64 = 7;
pub const FIELD_ISSN: i64 = 8;
pub const FIELD_EISSN: i64 = 9;
pub const FIELD_LANGUAGE: i64 = 10;
pub const FIELD_SOURCE: i64 = 11;
pub const FIELD_URL: i64 = 12;

const ZOTERO_SCHEMA_SQL: &str = "\
CREATE TABLE IF NOT EXISTS libraries (
    libraryID INTEGER PRIMARY KEY,
    type TEXT NOT NULL,
    editable INTEGER NOT NULL DEFAULT 1,
    filesEditable INTEGER NOT NULL DEFAULT 1,
    version INTEGER NOT NULL DEFAULT 0,
    clientVersion INTEGER NOT NULL DEFAULT 0,
    storageVersion INTEGER NOT NULL DEFAULT 0,
    lastSync INTEGER NOT NULL DEFAULT 0,
    archived INTEGER NOT NULL DEFAULT 0,
    isAdmin INTEGER NOT NULL DEFAULT 1
);

INSERT OR IGNORE INTO libraries \
    (libraryID, type, editable, filesEditable, isAdmin) \
VALUES (1, 'user', 1, 1, 1);

CREATE TABLE IF NOT EXISTS itemTypes (
    itemTypeID INTEGER PRIMARY KEY,
    typeName TEXT NOT NULL UNIQUE
);

INSERT OR IGNORE INTO itemTypes (itemTypeID, typeName) VALUES
    (1, 'journalArticle'),
    (2, 'attachment');

CREATE TABLE IF NOT EXISTS fields (
    fieldID INTEGER PRIMARY KEY,
    fieldName TEXT NOT NULL UNIQUE
);

INSERT OR IGNORE INTO fields (fieldID, fieldName) VALUES
    (1, 'title'),
    (2, 'abstractNote'),
    (3, 'date'),
    (4, 'journalTitle'),
    (5, 'volume'),
    (6, 'issue'),
    (7, 'pages'),
    (8, 'ISSN'),
    (9, 'EISSN'),
    (10, 'language'),
    (11, 'source'),
    (12, 'url');

CREATE TABLE IF NOT EXISTS itemDataValues (
    valueID INTEGER PRIMARY KEY AUTOINCREMENT,
    value TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS itemData (
    itemID INTEGER NOT NULL REFERENCES items(itemID) ON DELETE CASCADE,
    fieldID INTEGER NOT NULL REFERENCES fields(fieldID),
    valueID INTEGER NOT NULL REFERENCES itemDataValues(valueID),
    PRIMARY KEY (itemID, fieldID)
);
CREATE INDEX IF NOT EXISTS itemData_fieldID ON itemData(fieldID);
CREATE INDEX IF NOT EXISTS itemData_valueID ON itemData(valueID);

CREATE TABLE IF NOT EXISTS creatorTypes (
    creatorTypeID INTEGER PRIMARY KEY,
    creatorTypeName TEXT NOT NULL UNIQUE
);
INSERT OR IGNORE INTO creatorTypes (creatorTypeID, creatorTypeName) VALUES (1, 'author');

CREATE TABLE IF NOT EXISTS creators (
    creatorID INTEGER PRIMARY KEY AUTOINCREMENT,
    firstName TEXT,
    lastName TEXT,
    fieldMode INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS itemCreators (
    itemID INTEGER NOT NULL REFERENCES items(itemID) ON DELETE CASCADE,
    creatorID INTEGER NOT NULL REFERENCES creators(creatorID) ON DELETE CASCADE,
    creatorTypeID INTEGER NOT NULL DEFAULT 1 REFERENCES creatorTypes(creatorTypeID),
    orderIndex INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (itemID, orderIndex)
);
CREATE INDEX IF NOT EXISTS itemCreators_creatorID ON itemCreators(creatorID);

CREATE TABLE IF NOT EXISTS tags (
    tagID INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS itemTags (
    itemID INTEGER NOT NULL REFERENCES items(itemID) ON DELETE CASCADE,
    tagID INTEGER NOT NULL REFERENCES tags(tagID) ON DELETE CASCADE,
    type INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (itemID, tagID)
);
CREATE INDEX IF NOT EXISTS itemTags_tagID ON itemTags(tagID);

CREATE TABLE IF NOT EXISTS items (
    itemID INTEGER PRIMARY KEY AUTOINCREMENT,
    itemTypeID INTEGER NOT NULL REFERENCES itemTypes(itemTypeID),
    dateAdded TEXT NOT NULL,
    dateModified TEXT NOT NULL,
    clientDateModified TEXT NOT NULL,
    libraryID INTEGER NOT NULL DEFAULT 1 REFERENCES libraries(libraryID) ON DELETE CASCADE,
    key TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 0,
    clientVersion INTEGER NOT NULL DEFAULT 0,
    synced INTEGER NOT NULL DEFAULT 0,
    UNIQUE (libraryID, key)
);
CREATE INDEX IF NOT EXISTS items_synced ON items(synced);

CREATE TABLE IF NOT EXISTS articleItems (
    article_id TEXT PRIMARY KEY REFERENCES articles(id) ON DELETE CASCADE,
    itemID INTEGER NOT NULL UNIQUE REFERENCES items(itemID) ON DELETE CASCADE,
    attachmentItemID INTEGER UNIQUE REFERENCES items(itemID) ON DELETE SET NULL
);

CREATE TABLE IF NOT EXISTS itemAttachments (
    itemID INTEGER PRIMARY KEY REFERENCES items(itemID) ON DELETE CASCADE,
    parentItemID INTEGER REFERENCES items(itemID) ON DELETE CASCADE,
    linkMode INTEGER NOT NULL,
    contentType TEXT,
    charsetID INTEGER,
    path TEXT,
    syncState INTEGER NOT NULL DEFAULT 0,
    storageModTime INTEGER,
    storageHash TEXT,
    lastProcessedModificationTime INTEGER,
    lastRead INTEGER,
    title TEXT,
    sourceUrl TEXT,
    vfsPath TEXT,
    originalFilename TEXT,
    fileSize INTEGER
);
CREATE INDEX IF NOT EXISTS itemAttachments_parentItemID ON itemAttachments(parentItemID);
CREATE INDEX IF NOT EXISTS itemAttachments_contentType ON itemAttachments(contentType);
CREATE INDEX IF NOT EXISTS itemAttachments_syncState ON itemAttachments(syncState);
CREATE INDEX IF NOT EXISTS itemAttachments_vfsPath ON itemAttachments(vfsPath);

CREATE TABLE IF NOT EXISTS fulltextItems (
    itemID INTEGER PRIMARY KEY REFERENCES items(itemID) ON DELETE CASCADE,
    indexedPages INTEGER,
    totalPages INTEGER,
    indexedChars INTEGER,
    totalChars INTEGER,
    version INTEGER NOT NULL DEFAULT 0,
    synced INTEGER NOT NULL DEFAULT 0,
    extractionStatus TEXT,
    extractedTextFormat TEXT,
    extractedBy TEXT,
    extractError TEXT,
    vfsExtractedPath TEXT
);
CREATE INDEX IF NOT EXISTS fulltextItems_synced ON fulltextItems(synced);
CREATE INDEX IF NOT EXISTS fulltextItems_version ON fulltextItems(version);
CREATE INDEX IF NOT EXISTS fulltextItems_extractionStatus ON fulltextItems(extractionStatus);
";

/// A Zotero-style stored attachment plus project-specific VFS metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoredAttachment {
    pub item_id: i64,
    pub parent_item_id: i64,
    pub key: String,
    pub link_mode: i64,
    pub content_type: Option<String>,
    /// Zotero-native relative storage path (`storage:paper.pdf`).
    pub path: Option<String>,
    pub title: Option<String>,
    pub source_url: Option<String>,
    /// Project extension: resolvable object-store URI (`vfs:///...`).
    pub vfs_path: Option<String>,
    pub original_filename: Option<String>,
    pub storage_hash: Option<String>,
    pub file_size: Option<i64>,
}

/// A deleted attachment and its previous compatibility role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeletedAttachment {
    pub attachment: StoredAttachment,
    pub was_preferred: bool,
}

/// Metadata needed to insert one externally stored attachment item.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct NewAttachment {
    pub article_id: String,
    pub title: Option<String>,
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub file_format: FileFormat,
    pub link_mode: i64,
    pub path: Option<String>,
    pub source_url: Option<String>,
    pub vfs_path: Option<String>,
    pub storage_hash: Option<String>,
    pub file_size: Option<i64>,
}

/// Zotero-style full-text index state for one attachment item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FulltextItemState {
    pub item_id: i64,
    pub indexed_pages: Option<i64>,
    pub total_pages: Option<i64>,
    pub indexed_chars: Option<i64>,
    pub total_chars: Option<i64>,
    pub version: i64,
    pub synced: bool,
    pub extraction_status: Option<ExtractStatus>,
    pub extracted_text_format: Option<TextFormat>,
    pub extracted_by: Option<String>,
    pub extract_error: Option<String>,
    pub vfs_extracted_path: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct LegacyFulltext {
    pub(crate) article_id: String,
    pub(crate) file_path: String,
    pub(crate) file_format: FileFormat,
    pub(crate) text_content: Option<String>,
    pub(crate) file_hash: Option<String>,
    pub(crate) extract_status: Option<ExtractStatus>,
    pub(crate) text_format: Option<TextFormat>,
    pub(crate) extracted_by: Option<String>,
    pub(crate) extract_error: Option<String>,
}

impl LegacyFulltext {
    pub(crate) fn from_fulltext(value: &FullText) -> Self {
        Self {
            article_id: value.article_id.clone(),
            file_path: value.file_path.clone(),
            file_format: value.file_format,
            text_content: value.text_content.clone(),
            file_hash: value.file_hash.clone(),
            extract_status: value.extract_status,
            text_format: value.text_format,
            extracted_by: value.extracted_by.clone(),
            extract_error: value.extract_error.clone(),
        }
    }

    pub(crate) async fn load(conn: &Connection, article_id: &str) -> Result<Option<Self>> {
        let mut rows = conn
            .query(
                "SELECT article_id, file_path, file_format, text_content, file_hash, \
                        extract_status, text_format, extracted_by, extract_error \
                 FROM fulltexts WHERE article_id = ?1",
                turso::params![article_id],
            )
            .await?;
        let Some(row) = rows.next().await? else {
            return Ok(None);
        };
        Ok(Some(Self {
            article_id: row.get::<String>(0)?,
            file_path: row.get::<String>(1)?,
            file_format: parse_file_format(&row.get::<String>(2)?),
            text_content: opt_string(row.get_value(3)?),
            file_hash: opt_string(row.get_value(4)?),
            extract_status: opt_string(row.get_value(5)?)
                .as_deref()
                .map(ExtractStatus::from_str),
            text_format: opt_string(row.get_value(6)?)
                .as_deref()
                .map(TextFormat::from_str),
            extracted_by: opt_string(row.get_value(7)?),
            extract_error: opt_string(row.get_value(8)?),
        }))
    }
}

/// Create and idempotently populate Zotero-compatible tables.
pub(crate) async fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(ZOTERO_SCHEMA_SQL).await?;

    // Existing branch databases created before the explicit size extension
    // need the column added without recreating an already-migrated table.
    let mut has_file_size = false;
    {
        let mut rows = conn
            .query("PRAGMA table_info(itemAttachments)", turso::params![])
            .await?;
        while let Some(row) = rows.next().await? {
            if row.get::<String>(1)? == "fileSize" {
                has_file_size = true;
            }
        }
    }
    if !has_file_size {
        conn.execute_batch("ALTER TABLE itemAttachments ADD COLUMN fileSize INTEGER;")
            .await?;
    }

    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate).await?;

    // Snapshot legacy rows before opening nested statements: turso cursors
    // own connection statement state and cannot overlap child queries.
    let mut articles: Vec<String> = Vec::new();
    {
        let mut rows = tx
            .query("SELECT id FROM articles", turso::params![])
            .await?;
        while let Some(row) = rows.next().await? {
            articles.push(row.get::<String>(0)?);
        }
    }
    for article_id in articles {
        sync_article_item(&tx, &article_id).await?;
    }

    let mut fulltexts: Vec<LegacyFulltext> = Vec::new();
    {
        let mut rows = tx
            .query(
                "SELECT article_id, file_path, file_format, text_content, file_hash, \
                        extract_status, text_format, extracted_by, extract_error \
                 FROM fulltexts",
                turso::params![],
            )
            .await?;
        while let Some(row) = rows.next().await? {
            fulltexts.push(LegacyFulltext {
                article_id: row.get::<String>(0)?,
                file_path: row.get::<String>(1)?,
                file_format: parse_file_format(&row.get::<String>(2)?),
                text_content: opt_string(row.get_value(3)?),
                file_hash: opt_string(row.get_value(4)?),
                extract_status: opt_string(row.get_value(5)?)
                    .as_deref()
                    .map(ExtractStatus::from_str),
                text_format: opt_string(row.get_value(6)?)
                    .as_deref()
                    .map(TextFormat::from_str),
                extracted_by: opt_string(row.get_value(7)?),
                extract_error: opt_string(row.get_value(8)?),
            });
        }
    }
    for fulltext in fulltexts {
        sync_fulltext_attachment(&tx, &fulltext).await?;
    }

    tx.commit().await?;
    Ok(())
}

/// Return the current preferred attachment for a legacy article.
pub(crate) async fn attachment_for_article(
    conn: &Connection,
    article_id: &str,
) -> Result<Option<StoredAttachment>> {
    let mut rows = conn
        .query(
            "SELECT AI.itemID, AI.parentItemID, I.key, AI.linkMode, AI.contentType, AI.path, \
                    AI.title, AI.sourceUrl, AI.vfsPath, AI.originalFilename, AI.storageHash, \
                    AI.fileSize \
             FROM articleItems MAP \
             JOIN itemAttachments AI ON AI.itemID = MAP.attachmentItemID \
             JOIN items I ON I.itemID = AI.itemID \
             WHERE MAP.article_id = ?1",
            turso::params![article_id],
        )
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };

    Ok(Some(StoredAttachment {
        item_id: row.get::<i64>(0)?,
        parent_item_id: row.get::<i64>(1)?,
        key: row.get::<String>(2)?,
        link_mode: row.get::<i64>(3)?,
        content_type: opt_string(row.get_value(4)?),
        path: opt_string(row.get_value(5)?),
        title: opt_string(row.get_value(6)?),
        source_url: opt_string(row.get_value(7)?),
        vfs_path: opt_string(row.get_value(8)?),
        original_filename: opt_string(row.get_value(9)?),
        storage_hash: opt_string(row.get_value(10)?),
        file_size: opt_int(row.get_value(11)?),
    }))
}

/// Return every attachment child item in Zotero order.
pub(crate) async fn list_attachments_for_article(
    conn: &Connection,
    article_id: &str,
) -> Result<Vec<StoredAttachment>> {
    let mut rows = conn
        .query(
            "SELECT AI.itemID, AI.parentItemID, I.key, AI.linkMode, AI.contentType, AI.path, \
                    AI.title, AI.sourceUrl, AI.vfsPath, AI.originalFilename, AI.storageHash, \
                    AI.fileSize \
             FROM articleItems MAP \
             JOIN itemAttachments AI ON AI.parentItemID = MAP.itemID \
             JOIN items I ON I.itemID = AI.itemID \
             WHERE MAP.article_id = ?1 \
             ORDER BY I.dateAdded, I.itemID",
            turso::params![article_id],
        )
        .await?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        out.push(StoredAttachment {
            item_id: row.get::<i64>(0)?,
            parent_item_id: row.get::<i64>(1)?,
            key: row.get::<String>(2)?,
            link_mode: row.get::<i64>(3)?,
            content_type: opt_string(row.get_value(4)?),
            path: opt_string(row.get_value(5)?),
            title: opt_string(row.get_value(6)?),
            source_url: opt_string(row.get_value(7)?),
            vfs_path: opt_string(row.get_value(8)?),
            original_filename: opt_string(row.get_value(9)?),
            storage_hash: opt_string(row.get_value(10)?),
            file_size: opt_int(row.get_value(11)?),
        });
    }
    Ok(out)
}

/// Return one attachment by its Zotero item key.
pub(crate) async fn attachment_by_key(
    conn: &Connection,
    article_id: &str,
    key: &str,
) -> Result<Option<StoredAttachment>> {
    let mut rows = conn
        .query(
            "SELECT AI.itemID, AI.parentItemID, I.key, AI.linkMode, AI.contentType, AI.path, \
                    AI.title, AI.sourceUrl, AI.vfsPath, AI.originalFilename, AI.storageHash, \
                    AI.fileSize \
             FROM articleItems MAP \
             JOIN itemAttachments AI ON AI.parentItemID = MAP.itemID \
             JOIN items I ON I.itemID = AI.itemID \
             WHERE MAP.article_id = ?1 AND I.key = ?2",
            turso::params![article_id, key],
        )
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    Ok(Some(StoredAttachment {
        item_id: row.get::<i64>(0)?,
        parent_item_id: row.get::<i64>(1)?,
        key: row.get::<String>(2)?,
        link_mode: row.get::<i64>(3)?,
        content_type: opt_string(row.get_value(4)?),
        path: opt_string(row.get_value(5)?),
        title: opt_string(row.get_value(6)?),
        source_url: opt_string(row.get_value(7)?),
        vfs_path: opt_string(row.get_value(8)?),
        original_filename: opt_string(row.get_value(9)?),
        storage_hash: opt_string(row.get_value(10)?),
        file_size: opt_int(row.get_value(11)?),
    }))
}

/// Insert a new attachment child item backed by the VFS or an external URL.
pub(crate) async fn create_attachment(
    conn: &Connection,
    input: &NewAttachment,
    make_preferred: bool,
) -> Result<StoredAttachment> {
    let parent_item_id = ensure_article_item(conn, &input.article_id).await?;
    let attachment_item_id = insert_item(conn, ITEM_TYPE_ATTACHMENT).await?;
    let key = item_key(conn, attachment_item_id).await?;
    let title = input
        .title
        .clone()
        .or_else(|| input.filename.clone())
        .unwrap_or_else(|| "attachment".to_owned());
    let path = input.path.clone().or_else(|| {
        input
            .filename
            .as_deref()
            .map(|filename| format!("storage:{filename}"))
    });

    conn.execute(
        "INSERT INTO itemAttachments \
            (itemID, parentItemID, linkMode, contentType, path, title, sourceUrl, vfsPath, \
             originalFilename, storageHash, fileSize) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        turso::params![
            attachment_item_id,
            parent_item_id,
            input.link_mode,
            input.content_type.clone(),
            path.clone(),
            title.clone(),
            input.source_url.clone(),
            input.vfs_path.clone(),
            input.filename.clone(),
            input.storage_hash.clone(),
            input.file_size,
        ],
    )
    .await?;

    let title_id = item_data_value_id(conn, &title).await?;
    conn.execute(
        "INSERT INTO itemData (itemID, fieldID, valueID) VALUES (?1, ?2, ?3) \
         ON CONFLICT(itemID, fieldID) DO UPDATE SET valueID = excluded.valueID",
        turso::params![attachment_item_id, FIELD_TITLE, title_id],
    )
    .await?;
    if let Some(url) = input.source_url.as_deref() {
        let url_id = item_data_value_id(conn, url).await?;
        conn.execute(
            "INSERT INTO itemData (itemID, fieldID, valueID) VALUES (?1, ?2, ?3) \
             ON CONFLICT(itemID, fieldID) DO UPDATE SET valueID = excluded.valueID",
            turso::params![attachment_item_id, FIELD_URL, url_id],
        )
        .await?;
    }

    conn.execute(
        "INSERT INTO fulltextItems (itemID, version, synced, extractionStatus) \
         VALUES (?1, 0, 0, 'pending')",
        turso::params![attachment_item_id],
    )
    .await?;

    let current_preferred = mapped_attachment_item(conn, &input.article_id).await?;
    if make_preferred || current_preferred.is_none() {
        conn.execute(
            "UPDATE articleItems SET attachmentItemID = ?2 WHERE article_id = ?1",
            turso::params![input.article_id.clone(), attachment_item_id],
        )
        .await?;
    }

    Ok(StoredAttachment {
        item_id: attachment_item_id,
        parent_item_id,
        key,
        link_mode: input.link_mode,
        content_type: input.content_type.clone(),
        path,
        title: Some(title),
        source_url: input.source_url.clone(),
        vfs_path: input.vfs_path.clone(),
        original_filename: input.filename.clone(),
        storage_hash: input.storage_hash.clone(),
        file_size: input.file_size,
    })
}

/// Make an existing attachment the preferred compatibility full text.
pub(crate) async fn set_preferred_attachment(
    conn: &Connection,
    article_id: &str,
    key: &str,
) -> Result<bool> {
    let Some(attachment) = attachment_by_key(conn, article_id, key).await? else {
        return Ok(false);
    };
    conn.execute(
        "UPDATE articleItems SET attachmentItemID = ?2 WHERE article_id = ?1",
        turso::params![article_id, attachment.item_id],
    )
    .await?;
    Ok(true)
}

/// Attach VFS object metadata after an API handler has written the bytes.
pub(crate) async fn set_attachment_storage(
    conn: &Connection,
    article_id: &str,
    key: &str,
    vfs_path: &str,
    storage_hash: &str,
    file_size: i64,
) -> Result<Option<StoredAttachment>> {
    let Some(attachment) = attachment_by_key(conn, article_id, key).await? else {
        return Ok(None);
    };
    let filename = attachment
        .original_filename
        .clone()
        .or_else(|| filename_from_vfs_uri(vfs_path).into())
        .unwrap_or_else(|| "attachment".to_owned());
    conn.execute(
        "UPDATE itemAttachments SET path = ?2, vfsPath = ?3, originalFilename = ?4, \
                storageHash = ?5, fileSize = ?6 WHERE itemID = ?1",
        turso::params![
            attachment.item_id,
            format!("storage:{filename}"),
            vfs_path,
            filename,
            storage_hash,
            file_size,
        ],
    )
    .await?;
    attachment_by_key(conn, article_id, key).await
}

/// Delete one attachment child item and report whether it was preferred.
pub(crate) async fn delete_attachment_by_key(
    conn: &Connection,
    article_id: &str,
    key: &str,
) -> Result<Option<DeletedAttachment>> {
    let Some(attachment) = attachment_by_key(conn, article_id, key).await? else {
        return Ok(None);
    };
    let was_preferred = mapped_attachment_item(conn, article_id)
        .await?
        .is_some_and(|item_id| item_id == attachment.item_id);
    conn.execute(
        "DELETE FROM items WHERE itemID = ?1",
        turso::params![attachment.item_id],
    )
    .await?;
    if was_preferred {
        conn.execute(
            "UPDATE articleItems SET attachmentItemID = NULL WHERE article_id = ?1",
            turso::params![article_id],
        )
        .await?;
    }
    Ok(Some(DeletedAttachment {
        attachment,
        was_preferred,
    }))
}

/// Return Zotero full-text index state for an attachment item.
pub(crate) async fn fulltext_state(
    conn: &Connection,
    item_id: i64,
) -> Result<Option<FulltextItemState>> {
    let mut rows = conn
        .query(
            "SELECT itemID, indexedPages, totalPages, indexedChars, totalChars, version, synced, \
                    extractionStatus, extractedTextFormat, extractedBy, extractError, \
                    vfsExtractedPath \
             FROM fulltextItems WHERE itemID = ?1",
            turso::params![item_id],
        )
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };

    Ok(Some(FulltextItemState {
        item_id: row.get::<i64>(0)?,
        indexed_pages: opt_int(row.get_value(1)?),
        total_pages: opt_int(row.get_value(2)?),
        indexed_chars: opt_int(row.get_value(3)?),
        total_chars: opt_int(row.get_value(4)?),
        version: row.get::<i64>(5)?,
        synced: row.get::<i64>(6)? != 0,
        extraction_status: opt_string(row.get_value(7)?)
            .as_deref()
            .map(ExtractStatus::from_str),
        extracted_text_format: opt_string(row.get_value(8)?)
            .as_deref()
            .map(TextFormat::from_str),
        extracted_by: opt_string(row.get_value(9)?),
        extract_error: opt_string(row.get_value(10)?),
        vfs_extracted_path: opt_string(row.get_value(11)?),
    }))
}

pub(crate) async fn ensure_article_item(conn: &Connection, article_id: &str) -> Result<i64> {
    let mut rows = conn
        .query(
            "SELECT itemID FROM articleItems WHERE article_id = ?1",
            turso::params![article_id],
        )
        .await?;
    let mapped = match rows.next().await? {
        Some(row) => Some(row.get::<i64>(0)?),
        None => None,
    };
    drop(rows);

    if let Some(item_id) = mapped {
        let mut rows = conn
            .query(
                "SELECT 1 FROM items WHERE itemID = ?1 AND itemTypeID = ?2",
                turso::params![item_id, ITEM_TYPE_JOURNAL_ARTICLE],
            )
            .await?;
        let exists = rows.next().await?.is_some();
        drop(rows);
        if exists {
            return Ok(item_id);
        }
        conn.execute(
            "DELETE FROM articleItems WHERE article_id = ?1",
            turso::params![article_id],
        )
        .await?;
    }

    let item_id = insert_item(conn, ITEM_TYPE_JOURNAL_ARTICLE).await?;
    conn.execute(
        "INSERT INTO articleItems (article_id, itemID) VALUES (?1, ?2) \
         ON CONFLICT(article_id) DO UPDATE SET itemID = excluded.itemID",
        turso::params![article_id, item_id],
    )
    .await?;
    Ok(item_id)
}

/// Ensure the item exists and mirror normalized article metadata into
/// Zotero's EAV, creator, and tag tables.
pub(crate) async fn sync_article_item(conn: &Connection, article_id: &str) -> Result<i64> {
    let item_id = ensure_article_item(conn, article_id).await?;

    let (
        title,
        abstract_text,
        year,
        month,
        journal,
        volume,
        issue,
        pages,
        issn,
        essn,
        language,
        source,
        keywords,
    ) = {
        let mut rows = conn
            .query(
                "SELECT title, abstract, year, month, journal, volume, issue, pages, issn, essn, \
                        language, source, keywords \
                 FROM articles WHERE id = ?1",
                turso::params![article_id],
            )
            .await?;
        let Some(row) = rows.next().await? else {
            return Ok(item_id);
        };
        (
            row.get::<String>(0)?,
            opt_string(row.get_value(1)?),
            opt_int(row.get_value(2)?),
            opt_int(row.get_value(3)?),
            opt_string(row.get_value(4)?),
            opt_string(row.get_value(5)?),
            opt_string(row.get_value(6)?),
            opt_string(row.get_value(7)?),
            opt_string(row.get_value(8)?),
            opt_string(row.get_value(9)?),
            opt_string(row.get_value(10)?),
            opt_string(row.get_value(11)?),
            parse_json_strings(&opt_string(row.get_value(12)?).unwrap_or_default()),
        )
    };

    let date = match (year, month) {
        (Some(year), Some(month)) => Some(format!("{year:04}-{month:02}")),
        (Some(year), None) => Some(year.to_string()),
        _ => None,
    };
    let values = [
        (FIELD_TITLE, Some(title)),
        (FIELD_ABSTRACT, abstract_text),
        (FIELD_DATE, date),
        (FIELD_JOURNAL, journal),
        (FIELD_VOLUME, volume),
        (FIELD_ISSUE, issue),
        (FIELD_PAGES, pages),
        (FIELD_ISSN, issn),
        (FIELD_EISSN, essn),
        (FIELD_LANGUAGE, language),
        (FIELD_SOURCE, source),
    ];

    conn.execute(
        "DELETE FROM itemData WHERE itemID = ?1",
        turso::params![item_id],
    )
    .await?;
    for (field_id, value) in values
        .into_iter()
        .filter_map(|(field_id, value)| value.map(|value| (field_id, value)))
    {
        let value_id = item_data_value_id(conn, &value).await?;
        conn.execute(
            "INSERT INTO itemData (itemID, fieldID, valueID) VALUES (?1, ?2, ?3)",
            turso::params![item_id, field_id, value_id],
        )
        .await?;
    }

    // Author data is intentionally re-linked on every metadata sync. Creator
    // rows themselves are reused so ordinary refreshes do not grow the table.
    let mut author_rows = conn
        .query(
            "SELECT last_name, fore_name FROM authors WHERE article_id = ?1 ORDER BY position",
            turso::params![article_id],
        )
        .await?;
    let mut authors = Vec::new();
    while let Some(row) = author_rows.next().await? {
        authors.push((row.get::<String>(0)?, opt_string(row.get_value(1)?)));
    }
    drop(author_rows);
    conn.execute(
        "DELETE FROM itemCreators WHERE itemID = ?1",
        turso::params![item_id],
    )
    .await?;
    for (order_index, (last_name, first_name)) in authors.into_iter().enumerate() {
        let creator_id = creator_id(conn, &last_name, first_name.as_deref()).await?;
        conn.execute(
            "INSERT INTO itemCreators (itemID, creatorID, creatorTypeID, orderIndex) \
             VALUES (?1, ?2, 1, ?3)",
            turso::params![item_id, creator_id, order_index as i64],
        )
        .await?;
    }

    conn.execute(
        "DELETE FROM itemTags WHERE itemID = ?1",
        turso::params![item_id],
    )
    .await?;
    for keyword in keywords {
        if keyword.trim().is_empty() {
            continue;
        }
        let tag_id = tag_id(conn, keyword.trim()).await?;
        conn.execute(
            "INSERT OR IGNORE INTO itemTags (itemID, tagID) VALUES (?1, ?2)",
            turso::params![item_id, tag_id],
        )
        .await?;
    }

    Ok(item_id)
}

pub(crate) async fn sync_fulltext_attachment(
    conn: &Connection,
    fulltext: &LegacyFulltext,
) -> Result<()> {
    let parent_item_id = ensure_article_item(conn, &fulltext.article_id).await?;
    let attachment_item_id = mapped_attachment_item(conn, &fulltext.article_id)
        .await?
        .unwrap_or_else(|| 0);

    let attachment_item_id = if attachment_item_id != 0 {
        attachment_item_id
    } else {
        let item_id = insert_item(conn, ITEM_TYPE_ATTACHMENT).await?;
        conn.execute(
            "UPDATE articleItems SET attachmentItemID = ?2 WHERE article_id = ?1",
            turso::params![fulltext.article_id.clone(), item_id],
        )
        .await?;
        item_id
    };

    let is_vfs = fulltext.file_path.starts_with("vfs://");
    let original_filename = if is_vfs {
        Some(filename_from_vfs_uri(&fulltext.file_path))
    } else {
        None
    };
    let (link_mode, path, source_url) = if is_vfs {
        (
            LINK_MODE_IMPORTED_FILE,
            original_filename
                .as_deref()
                .map(|filename| format!("storage:{filename}")),
            None,
        )
    } else {
        (
            LINK_MODE_IMPORTED_URL,
            None,
            Some(fulltext.file_path.clone()),
        )
    };

    conn.execute(
        "INSERT INTO itemAttachments \
            (itemID, parentItemID, linkMode, contentType, path, title, sourceUrl, vfsPath, \
             originalFilename, storageHash) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
         ON CONFLICT(itemID) DO UPDATE SET \
            parentItemID = excluded.parentItemID, \
            linkMode = excluded.linkMode, \
            contentType = excluded.contentType, \
            path = excluded.path, \
            title = excluded.title, \
            sourceUrl = excluded.sourceUrl, \
            vfsPath = excluded.vfsPath, \
            originalFilename = excluded.originalFilename, \
            storageHash = excluded.storageHash",
        turso::params![
            attachment_item_id,
            parent_item_id,
            link_mode,
            content_type(fulltext.file_format),
            path.clone(),
            original_filename.clone(),
            source_url.clone(),
            Some(fulltext.file_path.clone()),
            original_filename.clone(),
            fulltext.file_hash.clone(),
        ],
    )
    .await?;

    if let Some(title) = original_filename.as_deref() {
        let value_id = item_data_value_id(conn, title).await?;
        conn.execute(
            "INSERT INTO itemData (itemID, fieldID, valueID) VALUES (?1, ?2, ?3) \
             ON CONFLICT(itemID, fieldID) DO UPDATE SET valueID = excluded.valueID",
            turso::params![attachment_item_id, FIELD_TITLE, value_id],
        )
        .await?;
    }
    if let Some(url) = source_url.as_deref() {
        let value_id = item_data_value_id(conn, url).await?;
        conn.execute(
            "INSERT INTO itemData (itemID, fieldID, valueID) VALUES (?1, ?2, ?3) \
             ON CONFLICT(itemID, fieldID) DO UPDATE SET valueID = excluded.valueID",
            turso::params![attachment_item_id, FIELD_URL, value_id],
        )
        .await?;
    }

    let text_chars = fulltext
        .text_content
        .as_deref()
        .map(char_count)
        .unwrap_or(0);
    let indexed = fulltext.extract_status == Some(ExtractStatus::Done) && text_chars > 0;
    let extracted_path = if is_vfs {
        Some(crate::stored_files::extracted_object_path(
            &fulltext.article_id,
            fulltext.text_format.unwrap_or_default(),
        ))
    } else {
        None
    };

    conn.execute(
        "INSERT INTO fulltextItems \
            (itemID, indexedPages, totalPages, indexedChars, totalChars, version, synced, \
             extractionStatus, extractedTextFormat, extractedBy, extractError, vfsExtractedPath) \
         VALUES (?1, NULL, NULL, ?2, ?3, 1, ?4, ?5, ?6, ?7, ?8, ?9) \
         ON CONFLICT(itemID) DO UPDATE SET \
            indexedChars = excluded.indexedChars, \
            totalChars = excluded.totalChars, \
            version = excluded.version, \
            synced = excluded.synced, \
            extractionStatus = excluded.extractionStatus, \
            extractedTextFormat = excluded.extractedTextFormat, \
            extractedBy = excluded.extractedBy, \
            extractError = excluded.extractError, \
            vfsExtractedPath = excluded.vfsExtractedPath",
        turso::params![
            attachment_item_id,
            if indexed { Some(text_chars) } else { None },
            if indexed { Some(text_chars) } else { None },
            if indexed { 1 } else { 0 },
            fulltext.extract_status.map(|status| status.as_str()),
            fulltext.text_format.map(|format| format.as_str()),
            fulltext.extracted_by.clone(),
            fulltext.extract_error.clone(),
            extracted_path,
        ],
    )
    .await?;
    Ok(())
}

pub(crate) async fn delete_attachment(conn: &Connection, article_id: &str) -> Result<Option<i64>> {
    let item_id = mapped_attachment_item(conn, article_id).await?;
    if let Some(item_id) = item_id {
        conn.execute(
            "DELETE FROM items WHERE itemID = ?1",
            turso::params![item_id],
        )
        .await?;
        conn.execute(
            "UPDATE articleItems SET attachmentItemID = NULL WHERE article_id = ?1",
            turso::params![article_id],
        )
        .await?;
    }
    Ok(item_id)
}

pub(crate) async fn delete_article_item(conn: &Connection, article_id: &str) -> Result<()> {
    let mut rows = conn
        .query(
            "SELECT itemID FROM articleItems WHERE article_id = ?1",
            turso::params![article_id],
        )
        .await?;
    let item_id = match rows.next().await? {
        Some(row) => Some(row.get::<i64>(0)?),
        None => None,
    };
    drop(rows);

    if let Some(item_id) = item_id {
        conn.execute(
            "DELETE FROM items WHERE itemID = ?1",
            turso::params![item_id],
        )
        .await?;
    }
    Ok(())
}

async fn mapped_attachment_item(conn: &Connection, article_id: &str) -> Result<Option<i64>> {
    let mut rows = conn
        .query(
            "SELECT attachmentItemID FROM articleItems WHERE article_id = ?1",
            turso::params![article_id],
        )
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    Ok(opt_int(row.get_value(0)?))
}

async fn item_data_value_id(conn: &Connection, value: &str) -> Result<i64> {
    let mut rows = conn
        .query(
            "SELECT valueID FROM itemDataValues WHERE value = ?1",
            turso::params![value],
        )
        .await?;
    let existing = match rows.next().await? {
        Some(row) => Some(row.get::<i64>(0)?),
        None => None,
    };
    drop(rows);
    if let Some(value_id) = existing {
        return Ok(value_id);
    }
    conn.execute(
        "INSERT INTO itemDataValues (value) VALUES (?1)",
        turso::params![value],
    )
    .await?;
    last_insert_id(conn).await
}

async fn creator_id(conn: &Connection, last_name: &str, first_name: Option<&str>) -> Result<i64> {
    let mut rows = conn
        .query(
            "SELECT creatorID FROM creators WHERE lastName = ?1 \
             AND firstName IS ?2 AND fieldMode = 0",
            turso::params![last_name, first_name],
        )
        .await?;
    let existing = match rows.next().await? {
        Some(row) => Some(row.get::<i64>(0)?),
        None => None,
    };
    drop(rows);
    if let Some(creator_id) = existing {
        return Ok(creator_id);
    }
    conn.execute(
        "INSERT INTO creators (firstName, lastName, fieldMode) VALUES (?1, ?2, 0)",
        turso::params![first_name, last_name],
    )
    .await?;
    last_insert_id(conn).await
}

async fn tag_id(conn: &Connection, name: &str) -> Result<i64> {
    let mut rows = conn
        .query(
            "SELECT tagID FROM tags WHERE name = ?1",
            turso::params![name],
        )
        .await?;
    let existing = match rows.next().await? {
        Some(row) => Some(row.get::<i64>(0)?),
        None => None,
    };
    drop(rows);
    if let Some(tag_id) = existing {
        return Ok(tag_id);
    }
    conn.execute("INSERT INTO tags (name) VALUES (?1)", turso::params![name])
        .await?;
    last_insert_id(conn).await
}

fn parse_json_strings(value: &str) -> Vec<String> {
    serde_json::from_str(value).unwrap_or_default()
}

async fn insert_item(conn: &Connection, item_type_id: i64) -> Result<i64> {
    let now = Utc::now().to_rfc3339();
    for _ in 0..16 {
        let key = new_item_key();
        let mut rows = conn
            .query(
                "SELECT 1 FROM items WHERE libraryID = ?1 AND key = ?2",
                turso::params![DEFAULT_LIBRARY_ID, key.clone()],
            )
            .await?;
        let key_exists = rows.next().await?.is_some();
        drop(rows);
        if key_exists {
            continue;
        }

        conn.execute(
            "INSERT INTO items \
                (itemTypeID, dateAdded, dateModified, clientDateModified, libraryID, key) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            turso::params![
                item_type_id,
                now.clone(),
                now.clone(),
                now,
                DEFAULT_LIBRARY_ID,
                key
            ],
        )
        .await?;
        return last_insert_id(conn).await;
    }
    Err(crate::error::Error::Unknown(
        "could not allocate a unique 8-character item key".into(),
    ))
}

async fn last_insert_id(conn: &Connection) -> Result<i64> {
    let mut rows = conn
        .query("SELECT last_insert_rowid()", turso::params![])
        .await?;
    let row = rows.next().await?.ok_or_else(|| {
        crate::error::Error::Unknown("last_insert_rowid() returned no row".into())
    })?;
    Ok(row.get::<i64>(0)?)
}

async fn item_key(conn: &Connection, item_id: i64) -> Result<String> {
    let mut rows = conn
        .query(
            "SELECT key FROM items WHERE itemID = ?1",
            turso::params![item_id],
        )
        .await?;
    let row = rows.next().await?.ok_or_else(|| {
        crate::error::Error::Unknown(format!("item {item_id} disappeared while creating key"))
    })?;
    Ok(row.get::<String>(0)?)
}

fn new_item_key() -> String {
    uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .to_uppercase()
        .chars()
        .take(8)
        .collect()
}

fn filename_from_vfs_uri(uri: &str) -> String {
    let raw = uri.rsplit('/').next().unwrap_or("attachment");
    decode_component(raw.split_once('-').map_or(raw, |(_, filename)| filename))
}

fn decode_component(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn content_type(format: FileFormat) -> &'static str {
    match format {
        FileFormat::Pdf => "application/pdf",
        FileFormat::Html => "text/html",
        FileFormat::Txt => "text/plain",
    }
}

fn parse_file_format(value: &str) -> FileFormat {
    match value {
        "pdf" => FileFormat::Pdf,
        "html" => FileFormat::Html,
        _ => FileFormat::Txt,
    }
}

fn char_count(text: &str) -> i64 {
    text.chars().count() as i64
}

fn opt_string(value: turso::Value) -> Option<String> {
    match value {
        turso::Value::Text(value) => Some(value),
        _ => None,
    }
}

fn opt_int(value: turso::Value) -> Option<i64> {
    match value {
        turso::Value::Integer(value) => Some(value),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BibBase;
    use bib_types::{Article, FullText, FullTextSource};

    #[tokio::test]
    async fn migrates_article_and_fulltext_to_zotero_items() {
        let base = BibBase::open_in_memory().await.unwrap();
        base.upsert_article(&Article::new("doi:10.1/a", "Aligned paper"))
            .await
            .unwrap();
        base.upsert_fulltext(&FullText {
            article_id: "doi:10.1/a".into(),
            file_path: "vfs:///literature/doi%3A10.1%2Fa/abc-paper.pdf".into(),
            file_format: FileFormat::Pdf,
            text_content: Some("full text".into()),
            source: FullTextSource::UserUpload,
            file_hash: Some("abc".into()),
            file_size: Some(9),
            uploaded_at: Some(chrono::Utc::now()),
            extract_status: Some(ExtractStatus::Done),
            text_format: Some(TextFormat::Plain),
            extracted_by: Some("test".into()),
            extract_error: None,
        })
        .await
        .unwrap();

        let attachment = base
            .zotero_attachment("doi:10.1/a")
            .await
            .unwrap()
            .expect("attachment");
        assert_eq!(attachment.link_mode, LINK_MODE_IMPORTED_FILE);
        assert_eq!(attachment.path.as_deref(), Some("storage:paper.pdf"));
        assert_eq!(
            attachment.vfs_path.as_deref(),
            Some("vfs:///literature/doi%3A10.1%2Fa/abc-paper.pdf")
        );

        let state = base
            .zotero_fulltext_state(attachment.item_id)
            .await
            .unwrap()
            .expect("fulltext state");
        assert_eq!(state.indexed_chars, Some(9));
        assert!(state.synced);
        assert_eq!(state.extraction_status, Some(ExtractStatus::Done));
    }

    #[tokio::test]
    async fn creates_and_lists_multiple_attachments() {
        let core = crate::bib_base::BibCore::open(":memory:").await.unwrap();
        core.upsert_article(&Article::new("doi:10.1/multi", "Multi"))
            .await
            .unwrap();

        let make_input = |filename: &str| NewAttachment {
            article_id: "doi:10.1/multi".into(),
            title: Some(filename.into()),
            filename: Some(filename.into()),
            content_type: Some("text/plain".into()),
            file_format: FileFormat::Txt,
            link_mode: LINK_MODE_IMPORTED_FILE,
            path: None,
            source_url: None,
            vfs_path: None,
            storage_hash: None,
            file_size: Some(4),
        };
        let conn = core.conn();
        let first = create_attachment(&conn, &make_input("a.txt"), true)
            .await
            .unwrap();
        let second = create_attachment(&conn, &make_input("b.txt"), true)
            .await
            .unwrap();
        assert_ne!(first.key, second.key);

        let attachments = list_attachments_for_article(&conn, "doi:10.1/multi")
            .await
            .unwrap();
        assert_eq!(attachments.len(), 2);
        assert!(
            attachments
                .iter()
                .all(|attachment| attachment.file_size == Some(4))
        );
    }

    #[tokio::test]
    async fn actor_queries_see_attachment_size_on_both_connections() {
        let base = BibBase::open_in_memory().await.unwrap();
        base.upsert_article(&Article::new("doi:10.1/actor", "Actor"))
            .await
            .unwrap();
        let input = NewAttachment {
            article_id: "doi:10.1/actor".into(),
            title: Some("a.txt".into()),
            filename: Some("a.txt".into()),
            content_type: Some("text/plain".into()),
            file_format: FileFormat::Txt,
            link_mode: LINK_MODE_IMPORTED_FILE,
            path: None,
            source_url: None,
            vfs_path: None,
            storage_hash: None,
            file_size: Some(4),
        };
        base.create_zotero_attachment(input.clone(), true)
            .await
            .unwrap();
        assert!(
            base.zotero_attachment("doi:10.1/actor")
                .await
                .unwrap()
                .is_some()
        );
        base.create_zotero_attachment(input, true).await.unwrap();
        assert_eq!(
            base.zotero_attachments("doi:10.1/actor")
                .await
                .unwrap()
                .len(),
            2
        );
        assert!(
            base.zotero_attachment("doi:10.1/actor")
                .await
                .unwrap()
                .is_some_and(|attachment| attachment.file_size == Some(4))
        );
    }
}
