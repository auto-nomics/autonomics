//! Transactional migration to Zotero collections and collectionItems.
//! The old association name is a read-only compatibility view, never a second
//! membership store. Autonomics' relationship metadata lives on collectionItems.
use crate::error::{Error, Result};
use turso::{
    Connection,
    transaction::{Transaction, TransactionBehavior},
};

pub(crate) async fn prepare_legacy(conn: &Connection) -> Result<()> {
    let mut rows = conn
        .query("SELECT 1 FROM sqlite_master WHERE name = 'collections'", ())
        .await?;
    let exists = rows.next().await?.is_some();
    drop(rows);
    if !exists {
        conn.execute_batch(
            r#"CREATE TABLE IF NOT EXISTS collections (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    description TEXT,
    tags        TEXT,
    status      TEXT NOT NULL DEFAULT 'active',
    created_at  TEXT,
    updated_at  TEXT
);

CREATE TABLE IF NOT EXISTS collection_articles (
    collection_id  TEXT NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    article_id     TEXT NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    position       INTEGER NOT NULL,
    role           TEXT NOT NULL DEFAULT 'referenced',
    fetch_status   TEXT NOT NULL DEFAULT 'metadata_only',
    added_by       TEXT NOT NULL DEFAULT 'agent',
    note           TEXT,
    added_at       TEXT,
    PRIMARY KEY (collection_id, article_id)
);

CREATE INDEX IF NOT EXISTS idx_collection_articles_fetch
    ON collection_articles(fetch_status) WHERE fetch_status = 'fulltext_requested';

"#,
        )
        .await?;
    }
    Ok(())
}

const NATIVE_SCHEMA: &str = r#"
CREATE INDEX IF NOT EXISTS collections_parent ON collections(parentCollectionID);
CREATE INDEX IF NOT EXISTS collections_synced ON collections(synced);
CREATE TABLE IF NOT EXISTS deletedCollections (
    collectionID INTEGER PRIMARY KEY REFERENCES collections(collectionID) ON DELETE CASCADE,
    dateDeleted TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS deletedCollections_dateDeleted ON deletedCollections(dateDeleted);
CREATE TABLE IF NOT EXISTS collectionRelations (
    collectionID INTEGER NOT NULL REFERENCES collections(collectionID) ON DELETE CASCADE,
    predicateID INTEGER NOT NULL,
    object TEXT NOT NULL,
    PRIMARY KEY(collectionID, predicateID, object)
);
CREATE INDEX IF NOT EXISTS collectionItems_itemID ON collectionItems(itemID);
CREATE INDEX IF NOT EXISTS collectionItems_fetch ON collectionItems(fetch_status);
CREATE VIEW IF NOT EXISTS collection_articles AS
    SELECT c.id AS collection_id, a.article_id, ci.orderIndex AS position,
           ci.role, ci.fetch_status, ci.added_by, ci.note, ci.added_at
    FROM collectionItems ci JOIN collections c USING(collectionID)
    JOIN articleItems a USING(itemID)
    WHERE NOT EXISTS (SELECT 1 FROM deletedCollections d WHERE d.collectionID=c.collectionID);
"#;

/// Keep a complete preimage of repaired relationships. No fuzzy matching or
/// lossy merging: ambiguous identifiers and conflicting semantics fail the transaction.
async fn repair_legacy_memberships(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS collectionMigrationAudit (collection_id TEXT NOT NULL,article_id TEXT NOT NULL,position INTEGER NOT NULL,role TEXT NOT NULL,fetch_status TEXT NOT NULL,added_by TEXT NOT NULL,note TEXT,added_at TEXT,resolved_article_id TEXT NOT NULL,recovery_reason TEXT NOT NULL,PRIMARY KEY(collection_id,article_id,recovery_reason));").await?;
    let mut rows=conn.query("SELECT DISTINCT ca.collection_id FROM collection_articles ca LEFT JOIN collections c ON c.id=ca.collection_id WHERE c.id IS NULL",()).await?;
    let mut missing = Vec::new();
    while let Some(row) = rows.next().await? {
        missing.push(row.get::<String>(0)?);
    }
    drop(rows);
    for id in missing {
        conn.execute("INSERT INTO collectionMigrationAudit SELECT collection_id,article_id,position,role,fetch_status,added_by,note,added_at,article_id,'missing_collection' FROM collection_articles WHERE collection_id=?1",turso::params![id.clone()]).await?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute("INSERT INTO collections(id,name,description,tags,status,created_at,updated_at) VALUES (?1,?2,?3,'[]','active',?4,?4)",turso::params![id.clone(),format!("Recovered collection {id}"),"Recovered during Zotero collection migration: existing memberships referenced a missing collection; its original name is unavailable.",now]).await?;
    }
    let mut rows=conn.query("SELECT ca.collection_id,ca.article_id FROM collection_articles ca LEFT JOIN articles a ON a.id=ca.article_id WHERE a.id IS NULL",()).await?;
    let mut aliases = Vec::new();
    while let Some(row) = rows.next().await? {
        aliases.push((row.get::<String>(0)?, row.get::<String>(1)?));
    }
    drop(rows);
    for (collection_id, original) in aliases {
        let raw = ["doi:", "pmid:", "pmc:", "arxiv:", "openalex:"]
            .iter()
            .find_map(|prefix| original.strip_prefix(prefix))
            .unwrap_or(original.as_str());
        let mut rows=conn.query("SELECT DISTINCT i.article_id FROM identifiers i JOIN articles a ON a.id=i.article_id WHERE LOWER(i.value)=LOWER(?1)",turso::params![raw]).await?;
        let mut candidates = Vec::new();
        while let Some(row) = rows.next().await? {
            candidates.push(row.get::<String>(0)?);
        }
        drop(rows);
        if candidates.len() != 1 {
            return Err(Error::InvalidCollection(format!(
                "cannot uniquely resolve legacy membership {collection_id}/{original}; no data was discarded"
            )));
        }
        let resolved = &candidates[0];
        let existing = legacy_membership(conn, &collection_id, resolved).await?;
        let original_data = legacy_membership(conn, &collection_id, &original)
            .await?
            .ok_or_else(|| Error::NotFound("legacy membership disappeared".into()))?;
        if let Some(existing) = &existing {
            if existing.role != original_data.role
                || existing.fetch_status != original_data.fetch_status
                || existing.added_by != original_data.added_by
            {
                return Err(Error::CollectionConflict(format!(
                    "legacy membership alias {original} has conflicting semantic metadata with {resolved} in {collection_id}; refusing a lossy merge"
                )));
            }
        }
        let reason = if existing.is_some() {
            "duplicate_identifier_alias"
        } else {
            "article_identifier_alias"
        };
        conn.execute("INSERT INTO collectionMigrationAudit SELECT collection_id,article_id,position,role,fetch_status,added_by,note,added_at,?3,?4 FROM collection_articles WHERE collection_id=?1 AND article_id=?2",turso::params![collection_id.clone(),original.clone(),resolved.clone(),reason]).await?;
        if let Some(existing) = existing {
            let note = match (existing.note, original_data.note) {
                (Some(left), Some(right)) if left != right => Some(format!(
                    "{left}\n\n[Recovered membership {original}]\n{right}"
                )),
                (Some(left), _) => Some(left),
                (_, right) => right,
            };
            let added_at = match (existing.added_at, original_data.added_at) {
                (Some(left), Some(right)) => Some(left.min(right)),
                (Some(left), _) => Some(left),
                (_, right) => right,
            };
            conn.execute("UPDATE collection_articles SET position=?3,note=?4,added_at=?5 WHERE collection_id=?1 AND article_id=?2",turso::params![collection_id.clone(),resolved.clone(),existing.position.min(original_data.position),note,added_at]).await?;
            conn.execute(
                "DELETE FROM collection_articles WHERE collection_id=?1 AND article_id=?2",
                turso::params![collection_id, original],
            )
            .await?;
        } else {
            conn.execute("UPDATE collection_articles SET article_id=?3 WHERE collection_id=?1 AND article_id=?2",turso::params![collection_id,original,resolved.clone()]).await?;
        }
    }
    Ok(())
}

struct LegacyMembership {
    position: i64,
    role: String,
    fetch_status: String,
    added_by: String,
    note: Option<String>,
    added_at: Option<String>,
}
async fn legacy_membership(
    conn: &Connection,
    collection_id: &str,
    article_id: &str,
) -> Result<Option<LegacyMembership>> {
    let mut rows=conn.query("SELECT position,role,fetch_status,added_by,note,added_at FROM collection_articles WHERE collection_id=?1 AND article_id=?2",turso::params![collection_id,article_id]).await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    let optional = |v: turso::Value| match v {
        turso::Value::Text(s) => Some(s),
        _ => None,
    };
    Ok(Some(LegacyMembership {
        position: row.get(0)?,
        role: row.get(1)?,
        fetch_status: row.get(2)?,
        added_by: row.get(3)?,
        note: optional(row.get_value(4)?),
        added_at: optional(row.get_value(5)?),
    }))
}

pub(crate) async fn migrate(conn: &Connection) -> Result<()> {
    let mut rows = conn.query("PRAGMA table_info(collections)", ()).await?;
    let mut native = false;
    while let Some(row) = rows.next().await? {
        native |= row.get::<String>(1)? == "collectionID";
    }
    drop(rows);
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate).await?;
    let result: Result<()> = async {
        if !native {
            repair_legacy_memberships(&tx).await?;
            // Every legacy article must have been mirrored before membership migration.
            let mut missing = tx.query("SELECT 1 FROM collection_articles ca LEFT JOIN articleItems a ON a.article_id=ca.article_id WHERE a.itemID IS NULL LIMIT 1", ()).await?;
            if missing.next().await?.is_some() { return Err(Error::Unknown("collection migration: unmapped article".into())); }
            drop(missing);
            tx.execute_batch(r#"
CREATE TABLE collections_native (
    collectionID INTEGER PRIMARY KEY AUTOINCREMENT,
    collectionName TEXT NOT NULL,
    parentCollectionID INTEGER REFERENCES collections_native(collectionID) ON DELETE CASCADE,
    clientDateModified TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    libraryID INTEGER NOT NULL DEFAULT 1 REFERENCES libraries(libraryID) ON DELETE CASCADE,
    key TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 0,
    clientVersion INTEGER NOT NULL DEFAULT 0,
    synced INTEGER NOT NULL DEFAULT 0,
    id TEXT NOT NULL UNIQUE,
    description TEXT, tags TEXT,
    status TEXT NOT NULL DEFAULT 'active', created_at TEXT, updated_at TEXT,
    UNIQUE(libraryID,key), CHECK(parentCollectionID IS NULL OR parentCollectionID != collectionID)
);
INSERT INTO collections_native(collectionName,key,id,description,tags,status,created_at,updated_at)
    SELECT name,upper(hex(randomblob(4))),id,description,tags,status,created_at,updated_at FROM collections;
CREATE TABLE collectionItems (
    collectionID INTEGER NOT NULL REFERENCES collections_native(collectionID) ON DELETE CASCADE,
    itemID INTEGER NOT NULL REFERENCES items(itemID) ON DELETE CASCADE,
    orderIndex INTEGER NOT NULL DEFAULT 0,
    role TEXT NOT NULL DEFAULT 'referenced',
    fetch_status TEXT NOT NULL DEFAULT 'metadata_only',
    added_by TEXT NOT NULL DEFAULT 'agent', note TEXT, added_at TEXT,
    PRIMARY KEY(collectionID,itemID)
);
INSERT INTO collectionItems(collectionID,itemID,orderIndex,role,fetch_status,added_by,note,added_at)
    SELECT c.collectionID,a.itemID,ca.position,ca.role,ca.fetch_status,ca.added_by,ca.note,ca.added_at
    FROM collection_articles ca JOIN collections_native c ON c.id=ca.collection_id
    JOIN articleItems a ON a.article_id=ca.article_id;
DROP TABLE collection_articles;
DROP TABLE collections;
ALTER TABLE collections_native RENAME TO collections;
"#).await.map_err(|e|Error::Unknown(format!("rebuild collections: {e}")))?;
        }
        tx.execute_batch(NATIVE_SCHEMA).await.map_err(|e|Error::Unknown(format!("collection views: {e}")))?;
        tx.execute("INSERT INTO bib_meta(key,value) VALUES ('collection_schema_version','1') ON CONFLICT(key) DO UPDATE SET value=excluded.value", ()).await?;
        Ok(())
    }.await;
    match result {
        Ok(()) => {
            tx.commit().await?;
            Ok(())
        }
        Err(e) => {
            let _ = tx.rollback().await;
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bib_base::BibCore;
    use bib_types::{AddedBy, Article, ArticleRole, Collection};

    #[tokio::test]
    async fn legacy_migration_preserves_native_identity_relationship_metadata_and_reopens() {
        let path =
            std::env::temp_dir().join(format!("collection-migration-{}.db", uuid::Uuid::new_v4()));
        let path_string = path.to_string_lossy().to_string();
        let core = BibCore::open(&path_string).await.unwrap();
        core.upsert_article(&Article::new("paper", "Old paper"))
            .await
            .unwrap();
        let conn = core.write_conn();
        // Recreate exactly the pre-migration collection tables around a real
        // item-backed article, rather than testing with SQLite's other engine.
        conn.execute_batch("DROP VIEW collection_articles; DROP TABLE collectionItems; DROP TABLE collectionRelations; DROP TABLE deletedCollections; DROP TABLE collections;").await.unwrap();
        prepare_legacy(&conn).await.unwrap();
        conn.execute("INSERT INTO collections(id,name,description,tags,status,created_at,updated_at) VALUES ('legacy','Legacy name','description','[\"tag\"]','completed','2020-01-01T00:00:00Z','2021-01-01T00:00:00Z')",()).await.unwrap();
        conn.execute("INSERT INTO collection_articles(collection_id,article_id,position,role,fetch_status,added_by,note,added_at) VALUES ('legacy','paper',7,'cited','fulltext_requested','user','original note','2020-01-02T00:00:00Z')",()).await.unwrap();
        migrate(&conn).await.unwrap();
        let loaded = core.get_collection("legacy").await.unwrap().unwrap();
        assert_eq!(loaded.name, "Legacy name");
        assert_eq!(loaded.description.as_deref(), Some("description"));
        assert_eq!(loaded.tags, vec!["tag"]);
        assert!(loaded.parent_id.is_none());
        assert_eq!(loaded.article_ids, vec!["paper"]);
        let membership = core
            .list_collection_articles("legacy", None, None)
            .await
            .unwrap()
            .remove(0);
        assert_eq!(membership.position, 7);
        assert_eq!(membership.role, ArticleRole::Cited);
        assert_eq!(membership.added_by, AddedBy::User);
        assert_eq!(membership.note.as_deref(), Some("original note"));
        assert_eq!(membership.fetch_status.as_str(), "fulltext_requested");
        let mut child = Collection::new("child", "Child");
        child.parent_id = Some("legacy".into());
        core.upsert_collection(&child).await.unwrap();
        core.migrate().await.unwrap();
        assert_eq!(
            core.get_collection("legacy").await.unwrap().unwrap().key,
            loaded.key
        );
        drop(conn);
        drop(core);
        let reopened = BibCore::open(&path_string).await.unwrap();
        assert_eq!(
            reopened
                .get_collection("child")
                .await
                .unwrap()
                .unwrap()
                .parent_id
                .as_deref(),
            Some("legacy")
        );
        assert_eq!(
            reopened
                .list_collection_articles("legacy", None, None)
                .await
                .unwrap()[0],
            membership
        );
        let conn = reopened.conn();
        let mut rows = conn.query("PRAGMA foreign_key_check", ()).await.unwrap();
        assert!(rows.next().await.unwrap().is_none());
        drop(rows);
        drop(conn);
        drop(reopened);
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    use crate::bib_base::BibCore;
    use bib_types::{Article, Identifier};

    async fn legacy_core() -> BibCore {
        let core = BibCore::open(":memory:").await.unwrap();
        let conn = core.write_conn();
        conn.execute_batch("DROP VIEW collection_articles;DROP TABLE collectionItems;DROP TABLE collectionRelations;DROP TABLE deletedCollections;DROP TABLE collections;").await.unwrap();
        prepare_legacy(&conn).await.unwrap();
        core
    }
    #[tokio::test]
    async fn resolves_unique_identifier_alias_and_recovers_missing_collection_without_losing_metadata()
     {
        let core = legacy_core().await;
        let mut article = Article::new("canonical", "Canonical title");
        article.identifiers.push(Identifier::doi("10.1000/alias"));
        core.upsert_article(&article).await.unwrap();
        let conn = core.write_conn();
        conn.pragma_update("foreign_keys", false).await.unwrap();
        conn.execute("INSERT INTO collection_articles(collection_id,article_id,position,role,fetch_status,added_by,note,added_at) VALUES ('orphan','10.1000/alias',9,'cited','fulltext_requested','agent','Keep this note','2020-01-01')",()).await.unwrap();
        conn.pragma_update("foreign_keys", true).await.unwrap();
        migrate(&conn).await.unwrap();
        let col = core.get_collection("orphan").await.unwrap().unwrap();
        assert_eq!(col.article_ids, vec!["canonical"]);
        assert_eq!(col.name, "Recovered collection orphan");
        let membership = core
            .list_collection_articles("orphan", None, None)
            .await
            .unwrap();
        assert_eq!(membership[0].position, 9);
        assert_eq!(membership[0].note.as_deref(), Some("Keep this note"));
        assert_eq!(membership[0].fetch_status.as_str(), "fulltext_requested");
        let mut rows = conn
            .query("SELECT COUNT(*) FROM collectionMigrationAudit", ())
            .await
            .unwrap();
        assert_eq!(
            rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
            2
        );
        drop(rows);
        let mut rows = conn.query("PRAGMA foreign_key_check", ()).await.unwrap();
        assert!(rows.next().await.unwrap().is_none());
    }
    #[tokio::test]
    async fn ambiguous_identifier_rolls_back_recovery_and_keeps_original_relationship() {
        let core = legacy_core().await;
        for id in ["a", "b"] {
            let mut article = Article::new(id, id);
            article
                .identifiers
                .push(Identifier::doi("10.1000/ambiguous"));
            core.upsert_article(&article).await.unwrap();
        }
        let conn = core.write_conn();
        conn.pragma_update("foreign_keys", false).await.unwrap();
        conn.execute("INSERT INTO collection_articles(collection_id,article_id,position,role,fetch_status,added_by,note,added_at) VALUES ('orphan','10.1000/ambiguous',0,'cited','metadata_only','agent','original',NULL)",()).await.unwrap();
        conn.pragma_update("foreign_keys", true).await.unwrap();
        assert!(migrate(&conn).await.is_err());
        let mut rows = conn
            .query("SELECT article_id,note FROM collection_articles", ())
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "10.1000/ambiguous");
        assert_eq!(row.get::<String>(1).unwrap(), "original");
        drop(rows);
        let mut rows = conn
            .query("SELECT COUNT(*) FROM collections", ())
            .await
            .unwrap();
        assert_eq!(
            rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
            0
        );
        drop(rows);
        let mut rows = conn
            .query("SELECT COUNT(*) FROM collectionMigrationAudit", ())
            .await
            .unwrap();
        assert_eq!(
            rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
            0
        );
    }
}

#[cfg(test)]
mod duplicate_recovery_tests {
    use super::*;
    use crate::bib_base::BibCore;
    use bib_types::{Article, Identifier};
    #[tokio::test]
    async fn duplicate_identifier_aliases_keep_both_notes_and_auditable_preimages() {
        let core = BibCore::open(":memory:").await.unwrap();
        let mut a = Article::new("canonical", "Paper");
        a.identifiers.push(Identifier::doi("10.1000/paper"));
        a.identifiers.push(Identifier::pmid("12345"));
        core.upsert_article(&a).await.unwrap();
        let conn = core.write_conn();
        conn.execute_batch("DROP VIEW collection_articles;DROP TABLE collectionItems;DROP TABLE collectionRelations;DROP TABLE deletedCollections;DROP TABLE collections;").await.unwrap();
        prepare_legacy(&conn).await.unwrap();
        conn.execute(
            "INSERT INTO collections(id,name) VALUES ('c','Collection')",
            (),
        )
        .await
        .unwrap();
        conn.pragma_update("foreign_keys", false).await.unwrap();
        conn.execute("INSERT INTO collection_articles(collection_id,article_id,position,role,fetch_status,added_by,note,added_at) VALUES ('c','10.1000/paper',24,'referenced','metadata_only','agent','First note','2020-01-01'),('c','12345',36,'referenced','metadata_only','agent','Second note','2021-01-01')",()).await.unwrap();
        conn.pragma_update("foreign_keys", true).await.unwrap();
        migrate(&conn).await.unwrap();
        let rows = core
            .list_collection_articles("c", None, None)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].article_id, "canonical");
        assert_eq!(rows[0].position, 24);
        let note = rows[0].note.as_deref().unwrap();
        assert!(note.contains("First note"));
        assert!(note.contains("Second note"));
        assert!(note.contains("\n\n[Recovered membership"));
        let mut audit = conn
            .query(
                "SELECT article_id,note FROM collectionMigrationAudit ORDER BY article_id",
                (),
            )
            .await
            .unwrap();
        let mut count = 0;
        while let Some(row) = audit.next().await.unwrap() {
            let id = row.get::<String>(0).unwrap();
            let note = row.get::<String>(1).unwrap();
            assert!(matches!(
                (id.as_str(), note.as_str()),
                ("10.1000/paper", "First note") | ("12345", "Second note")
            ));
            count += 1;
        }
        assert_eq!(count, 2);
    }
}
