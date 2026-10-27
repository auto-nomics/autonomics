//! Collection management — CRUD for [`Collection`] and the enriched
//! [`CollectionArticle`] association that links articles to collections.
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
use bib_types::{
    AddedBy, Article, ArticleRole, Collection, CollectionArticle, CollectionStatus, FetchStatus,
    IdKind, Identifier,
};

// ---------------------------------------------------------------------------
// Outcome type
// ---------------------------------------------------------------------------

/// What happened when associating an article with a collection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectionAddOutcome {
    /// A new `collection_articles` row was inserted.
    Inserted,
    /// The pair already existed; the row was updated in place.
    Updated {
        /// The role before this call.
        previous_role: ArticleRole,
        /// The note before this call (may be `None`).
        previous_note: Option<String>,
        /// Whether this call actually changed the role.
        role_changed: bool,
        /// Whether this call actually changed the note.
        note_changed: bool,
    },
}

impl CollectionAddOutcome {
    /// `true` for [`Self::Inserted`].
    pub fn was_inserted(&self) -> bool {
        matches!(self, Self::Inserted)
    }

    /// `true` when nothing changed (existing row, same role, note
    /// preserved or identical).
    pub fn was_noop(&self) -> bool {
        match self {
            Self::Inserted => false,
            Self::Updated {
                role_changed,
                note_changed,
                ..
            } => !role_changed && !note_changed,
        }
    }
}

/// One requested collection association for atomic library ingest.
#[derive(Debug, Clone)]
pub struct CollectionAssignment {
    pub article_id: String,
    pub role: ArticleRole,
    pub added_by: AddedBy,
    pub note: Option<String>,
}

/// The association result for one article after an atomic ingest.
#[derive(Debug, Clone)]
pub struct CollectionAssignmentOutcome {
    pub article_id: String,
    pub outcome: CollectionAddOutcome,
}

// ---------------------------------------------------------------------------
// Collection CRUD
// ---------------------------------------------------------------------------

/// Structural operations are validated and committed in one actor turn.
#[derive(Debug, Clone)]
pub enum CollectionChange {
    Update {
        name: Option<String>,
        parent_id: Option<Option<String>>,
    },
    Trash,
    Restore,
    Erase,
}

const COLLECTION_SELECT: &str = "SELECT c.id,c.collectionName,c.description,c.tags,c.status,c.created_at,c.updated_at, c.collectionID,c.libraryID,c.key,p.id, c.version, CASE WHEN d.collectionID IS NULL THEN 0 ELSE 1 END FROM collections c LEFT JOIN collections p ON p.collectionID=c.parentCollectionID LEFT JOIN deletedCollections d ON d.collectionID=c.collectionID";

fn decode_collection(row: &turso::Row) -> Result<Collection> {
    let mut col = Collection::new(row.get::<String>(0)?, row.get::<String>(1)?);
    col.description = opt_string(row.get_value(2)?);
    col.tags = parse_json_col(&opt_string(row.get_value(3)?));
    col.status = CollectionStatus::from_str(&opt_string(row.get_value(4)?).unwrap_or_default());
    col.created_at = opt_string(row.get_value(5)?).as_deref().and_then(parse_dt);
    col.updated_at = opt_string(row.get_value(6)?).as_deref().and_then(parse_dt);
    col.collection_id = row.get(7)?;
    col.library_id = row.get(8)?;
    col.key = row.get(9)?;
    col.parent_id = opt_string(row.get_value(10)?);
    col.version = row.get(11)?;
    col.deleted = row.get::<i64>(12)? != 0;
    Ok(col)
}

async fn stored_collection(conn: &turso::Connection, id: &str) -> Result<Collection> {
    let mut rows = conn
        .query(
            format!("{COLLECTION_SELECT} WHERE c.id=?1"),
            turso::params![id],
        )
        .await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| Error::NotFound(format!("collection {id}")))?;
    decode_collection(&row)
}

async fn validate_parent(
    conn: &turso::Connection,
    id: &str,
    library_id: i64,
    parent_id: Option<&str>,
) -> Result<Option<i64>> {
    let mut cursor = parent_id.map(str::to_owned);
    let mut seen = std::collections::HashSet::new();
    let mut direct = None;
    while let Some(parent) = cursor {
        if parent == id || !seen.insert(parent.clone()) {
            return Err(Error::CollectionConflict(
                "cannot move a collection into itself or its descendants".into(),
            ));
        }
        let col = stored_collection(conn, &parent).await?;
        if col.deleted {
            return Err(Error::CollectionConflict(
                "parent collection is in trash".into(),
            ));
        }
        if col.library_id != library_id {
            return Err(Error::CollectionConflict(
                "parent belongs to another library".into(),
            ));
        }
        direct = direct.or(Some(col.collection_id));
        cursor = col.parent_id;
    }
    Ok(direct)
}

/// Turso 0.7 does not support recursive CTEs. Traverse a bounded snapshot
/// iteratively, retaining the same transaction and cycle protection.
pub(crate) async fn subtree_ids(conn: &turso::Connection, id: &str) -> Result<Vec<(String, i64)>> {
    let mut rows = conn
        .query(
            "SELECT id,collectionID,parentCollectionID FROM collections",
            (),
        )
        .await?;
    let mut children: std::collections::HashMap<i64, Vec<(String, i64)>> =
        std::collections::HashMap::new();
    let mut root = None;
    while let Some(row) = rows.next().await? {
        let public = row.get::<String>(0)?;
        let native = row.get::<i64>(1)?;
        if public == id {
            root = Some((public.clone(), native));
        }
        if let turso::Value::Integer(parent) = row.get_value(2)? {
            children.entry(parent).or_default().push((public, native));
        }
    }
    drop(rows);
    let root = root.ok_or_else(|| Error::NotFound(format!("collection {id}")))?;
    let mut queue = vec![root];
    let mut seen = std::collections::HashSet::new();
    let mut at = 0;
    while at < queue.len() {
        let native = queue[at].1;
        if !seen.insert(native) {
            return Err(Error::CollectionConflict("cycle in collection tree".into()));
        }
        if let Some(kids) = children.remove(&native) {
            queue.extend(kids);
        }
        at += 1;
    }
    Ok(queue)
}

#[derive(Debug, Clone)]
pub struct CollectionQuery {
    pub collection_id: Option<String>,
    pub include_descendants: bool,
    pub unfiled: bool,
    pub query: String,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CollectionPage {
    pub articles: Vec<Article>,
    pub associations: Vec<CollectionArticle>,
    pub total: i64,
}

async fn mark_membership_change(conn: &turso::Connection, article_id: &str) -> Result<()> {
    conn.execute("UPDATE items SET synced=0,clientVersion=clientVersion+1,dateModified=?1,clientDateModified=?1 WHERE itemID=(SELECT itemID FROM articleItems WHERE article_id=?2)",turso::params![Utc::now().to_rfc3339(),article_id]).await?;
    Ok(())
}

async fn require_editable(conn: &turso::Connection, library_id: i64) -> Result<()> {
    let mut rows = conn
        .query(
            "SELECT editable,archived FROM libraries WHERE libraryID=?1",
            turso::params![library_id],
        )
        .await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| Error::NotFound(format!("library {library_id}")))?;
    if row.get::<i64>(0)? == 0 || row.get::<i64>(1)? != 0 {
        return Err(Error::CollectionConflict("library is not editable".into()));
    }
    Ok(())
}

impl BibCore {
    /// Legacy public IDs remain stable; native Zotero keys are allocated once.
    /// Existing parentage is preserved by metadata upsert. Use change_collection
    /// for an explicit move (including a move to the library root).
    pub async fn upsert_collection(&self, collection: &Collection) -> Result<()> {
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result: Result<()> = async {
            if collection.name.trim().is_empty() { return Err(Error::InvalidCollection("name is required".into())); }
            require_editable(&tx,collection.library_id).await?;
            let existing=match stored_collection(&tx,&collection.id).await {
                Ok(c) => Some(c), Err(Error::NotFound(_)) => None, Err(e) => return Err(e),
            };
            if let Some(c)=&existing {
                if c.deleted { return Err(Error::CollectionConflict("collection is in trash".into())); }
                if c.library_id != collection.library_id { return Err(Error::CollectionConflict("cannot change collection library".into())); }
            }
            let parent=if let Some(c)=&existing { c.parent_id.as_deref() } else { collection.parent_id.as_deref() };
            let parent_native=validate_parent(&tx,&collection.id,collection.library_id,parent).await?;
            let key=existing.as_ref().map(|c|c.key.clone()).unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string()[..8].to_uppercase());
            tx.execute("INSERT INTO collections (id,collectionName,parentCollectionID,libraryID,key,description,tags,status,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(id) DO UPDATE SET collectionName=excluded.collectionName,description=excluded.description,tags=excluded.tags,status=excluded.status,updated_at=excluded.updated_at,clientDateModified=excluded.updated_at,synced=0,clientVersion=collections.clientVersion+1",turso::params![collection.id.clone(),collection.name.trim(),parent_native,collection.library_id,key,collection.description.clone(),serde_json::to_string(&collection.tags)?,collection.status.as_str(),collection.created_at.map(|t|t.to_rfc3339()),Utc::now().to_rfc3339()]).await?;
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

    pub async fn get_collection(&self, id: &str) -> Result<Option<Collection>> {
        let mut col = match stored_collection(&self.conn(), id).await {
            Ok(c) if !c.deleted => c,
            Ok(_) | Err(Error::NotFound(_)) => return Ok(None),
            Err(e) => return Err(e),
        };
        col.article_ids = self.collection_article_ids(id).await?;
        Ok(Some(col))
    }

    pub async fn list_collection_summaries(
        &self,
        include_deleted: bool,
    ) -> Result<Vec<serde_json::Value>> {
        let conn = self.conn();
        let mut rows = conn
            .query(
                format!("{COLLECTION_SELECT} ORDER BY c.collectionName COLLATE NOCASE,c.id"),
                (),
            )
            .await?;
        let mut collections = Vec::new();
        while let Some(row) = rows.next().await? {
            let c = decode_collection(&row)?;
            if include_deleted || !c.deleted {
                collections.push(c);
            }
        }
        drop(rows);
        let mut counts = std::collections::HashMap::new();
        let mut rows = conn
            .query(
                "SELECT collectionID,COUNT(*) FROM collectionItems GROUP BY collectionID",
                (),
            )
            .await?;
        while let Some(row) = rows.next().await? {
            counts.insert(row.get::<i64>(0)?, row.get::<i64>(1)?);
        }
        let mut children = std::collections::HashMap::new();
        for c in &collections {
            if let Some(pid) = &c.parent_id {
                *children.entry((pid.as_str(), c.deleted)).or_insert(0usize) += 1;
            }
        }
        let mut out = Vec::new();
        for c in &collections {
            let mut value = serde_json::to_value(c)?;
            value.as_object_mut().unwrap().remove("article_ids");
            value["direct_article_count"] =
                serde_json::json!(counts.get(&c.collection_id).copied().unwrap_or(0));
            value["child_count"] = serde_json::json!(
                children
                    .get(&(c.id.as_str(), c.deleted))
                    .copied()
                    .unwrap_or(0)
            );
            out.push(value);
        }
        Ok(out)
    }

    pub async fn list_collections_including_deleted(
        &self,
        status: Option<CollectionStatus>,
        include_deleted: bool,
    ) -> Result<Vec<Collection>> {
        let conn = self.conn();
        let mut rows = conn
            .query(
                format!("{COLLECTION_SELECT} ORDER BY c.collectionName COLLATE NOCASE,c.id"),
                (),
            )
            .await?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            let c = decode_collection(&row)?;
            if (include_deleted || !c.deleted) && status.is_none_or(|s| s == c.status) {
                out.push(c);
            }
        }
        drop(rows);
        let index: std::collections::HashMap<String, usize> = out
            .iter()
            .enumerate()
            .map(|(i, c)| (c.id.clone(), i))
            .collect();
        let mut rows=conn.query("SELECT c.id,a.article_id FROM collectionItems ci JOIN collections c USING(collectionID) JOIN articleItems a USING(itemID) ORDER BY c.id,ci.orderIndex",()).await?;
        while let Some(row) = rows.next().await? {
            if let Some(&i) = index.get(&row.get::<String>(0)?) {
                out[i].article_ids.push(row.get(1)?);
            }
        }
        Ok(out)
    }

    pub async fn update_collection_status(&self, id: &str, status: CollectionStatus) -> Result<()> {
        let mut col = self
            .get_collection(id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("collection {id}")))?;
        col.status = status;
        self.upsert_collection(&col).await
    }

    /// Move the complete subtree to trash, preserving membership for restore.
    pub async fn delete_collection(&self, id: &str) -> Result<()> {
        self.change_collection(id, CollectionChange::Trash)
            .await
            .map(|_| ())
    }

    pub async fn change_collection(
        &self,
        id: &str,
        change: CollectionChange,
    ) -> Result<Vec<String>> {
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result:Result<Vec<String>>=async {
            let col=stored_collection(&tx,id).await?;
            require_editable(&tx,col.library_id).await?;
            let mut subtree=subtree_ids(&tx,id).await?;
            match change {
                CollectionChange::Update{name,parent_id} => {
                    if col.deleted {return Err(Error::CollectionConflict("collection is in trash".into()));}
                    let name=name.unwrap_or_else(||col.name.clone());
                    if name.trim().is_empty() {return Err(Error::InvalidCollection("name is required".into()));}
                    let parent=parent_id.unwrap_or_else(||col.parent_id.clone());
                    let native=validate_parent(&tx,id,col.library_id,parent.as_deref()).await?;
                    if name.trim()==col.name && parent==col.parent_id {return Ok(vec![id.to_owned()]);}
                    tx.execute("UPDATE collections SET collectionName=?1,parentCollectionID=?2,updated_at=?3,clientDateModified=?3,synced=0,clientVersion=clientVersion+1 WHERE id=?4",turso::params![name.trim(),native,Utc::now().to_rfc3339(),id]).await?;
                    return Ok(vec![id.to_owned()]);
                }
                CollectionChange::Trash => {
                    for (_,native) in &subtree {
                        tx.execute("INSERT OR IGNORE INTO deletedCollections(collectionID) VALUES (?1)",turso::params![*native]).await?;
                    }
                }
                CollectionChange::Restore => {
                    // Restoring a subtree must also restore its ancestors, so
                    // no live collection remains below a trashed parent.
                    let mut parent=col.parent_id;
                    let mut seen=std::collections::HashSet::new();
                    while let Some(pid)=parent {
                        if !seen.insert(pid.clone()) {return Err(Error::CollectionConflict("invalid ancestry".into()));}
                        let ancestor=stored_collection(&tx,&pid).await?;
                        tx.execute("DELETE FROM deletedCollections WHERE collectionID=?1",turso::params![ancestor.collection_id]).await?;
                        if !subtree.iter().any(|(id,_)|id==&pid) {subtree.push((pid,ancestor.collection_id));}
                        parent=ancestor.parent_id;
                    }
                    for (_,native) in &subtree {tx.execute("DELETE FROM deletedCollections WHERE collectionID=?1",turso::params![*native]).await?;}
                }
                CollectionChange::Erase => {
                    if !col.deleted {return Err(Error::CollectionConflict("move collection to trash before permanently erasing it".into()));}
                    // Clear parent links before deleting to avoid recursion limits.
                    for (_,native) in &subtree {tx.execute("UPDATE collections SET parentCollectionID=NULL WHERE collectionID=?1",turso::params![*native]).await?;}
                    for (_,native) in &subtree {tx.execute("DELETE FROM collections WHERE collectionID=?1",turso::params![*native]).await?;}
                    return Ok(subtree.into_iter().map(|(id,_)|id).collect());
                }
            }
            for (_,native) in &subtree {tx.execute("UPDATE collections SET synced=0,clientVersion=clientVersion+1,clientDateModified=?1,updated_at=?1 WHERE collectionID=?2",turso::params![Utc::now().to_rfc3339(),*native]).await?;}
            Ok(subtree.into_iter().map(|(id,_)|id).collect())
        }.await;
        match result {
            Ok(ids) => {
                tx.commit().await?;
                Ok(ids)
            }
            Err(e) => {
                let _ = tx.rollback().await;
                Err(e)
            }
        }
    }

    // -----------------------------------------------------------------------
    // Collection ↔ Article association
    // -----------------------------------------------------------------------

    /// User drag/add without relationship edits is idempotent: it must not
    /// downgrade an existing Cited role or replace a relationship note.
    pub async fn ensure_collection_membership(
        &self,
        collection_id: &str,
        article_id: &str,
    ) -> Result<CollectionAddOutcome> {
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result:Result<CollectionAddOutcome>=async {
            let col=stored_collection(&tx,collection_id).await?;
            require_editable(&tx,col.library_id).await?;
            if col.deleted {return Err(Error::CollectionConflict("collection is in trash".into()));}
            let mut rows=tx.query("SELECT role,note FROM collection_articles WHERE collection_id=?1 AND article_id=?2",turso::params![collection_id,article_id]).await?;
            let existing=rows.next().await?.map(|row|Ok::<_,Error>((ArticleRole::from_str(&row.get::<String>(0)?),opt_string(row.get_value(1)?)))).transpose()?;
            drop(rows);
            if let Some((role,note))=existing {return Ok(CollectionAddOutcome::Updated{previous_role:role,previous_note:note,role_changed:false,note_changed:false});}
            Self::add_to_collection_in_tx(&tx,collection_id,article_id,ArticleRole::Referenced,AddedBy::User,None).await
        }.await;
        match result {
            Ok(outcome) => {
                tx.commit().await?;
                Ok(outcome)
            }
            Err(e) => {
                let _ = tx.rollback().await;
                Err(e)
            }
        }
    }

    /// Associate an article with a collection.
    ///
    /// - **New pair** → inserts a row at `position = MAX + 1`.
    /// - **Existing pair** → updates `role`/`added_by` and overwrites
    ///   `note` only when the caller passes a non-`None` value (`None`
    ///   preserves the existing note). `position` is never changed.
    ///
    /// Returns [`CollectionAddOutcome`] so callers (e.g. the
    /// `bib_add_to_collection` tool) can distinguish a fresh insert from
    /// a silent overwrite and surface that to the user.
    pub async fn add_to_collection(
        &self,
        collection_id: &str,
        article_id: &str,
        role: ArticleRole,
        added_by: AddedBy,
        note: Option<&str>,
    ) -> Result<CollectionAddOutcome> {
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let outcome =
            Self::add_to_collection_in_tx(&tx, collection_id, article_id, role, added_by, note)
                .await;

        if outcome.is_err() {
            let _ = tx.rollback().await;
            return outcome;
        }

        tx.commit().await?;
        Ok(outcome.unwrap())
    }

    /// Upsert articles and associate them with one collection atomically.
    ///
    /// This is the DAG `bib_save` ingest primitive: metadata writes and
    /// collection membership changes commit together or not at all.
    pub async fn upsert_articles_with_collection(
        &self,
        articles: &[Article],
        collection_id: &str,
        assignments: &[CollectionAssignment],
    ) -> Result<Vec<CollectionAssignmentOutcome>> {
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;

        async fn run(
            tx: &Transaction<'_>,
            articles: &[Article],
            collection_id: &str,
            assignments: &[CollectionAssignment],
        ) -> Result<Vec<CollectionAssignmentOutcome>> {
            for article in articles {
                BibCore::upsert_article_in_tx(tx, article).await?;
                crate::zotero_model::ensure_article_item(tx, &article.id).await?;
                crate::zotero_model::sync_article_item(tx, &article.id).await?;
                crate::bib_base::sync_search_index(tx, &article.id).await?;
            }

            let mut outcomes = Vec::with_capacity(assignments.len());
            for assignment in assignments {
                let outcome = BibCore::add_to_collection_in_tx(
                    tx,
                    collection_id,
                    assignment.article_id.as_str(),
                    assignment.role,
                    assignment.added_by,
                    assignment.note.as_deref(),
                )
                .await?;
                outcomes.push(CollectionAssignmentOutcome {
                    article_id: assignment.article_id.to_owned(),
                    outcome,
                });
            }
            Ok(outcomes)
        }

        match run(&tx, articles, collection_id, assignments).await {
            Ok(outcomes) => {
                tx.commit().await?;
                Ok(outcomes)
            }
            Err(error) => {
                let _ = tx.rollback().await;
                Err(error)
            }
        }
    }

    /// Remove an article from a collection.
    pub async fn remove_from_collection(
        &self,
        collection_id: &str,
        article_id: &str,
    ) -> Result<()> {
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result:Result<()>=async {
            let col=stored_collection(&tx,collection_id).await?;
            require_editable(&tx,col.library_id).await?;
            if col.deleted {return Err(Error::CollectionConflict("collection is in trash".into()));}
            let changed=tx.execute("DELETE FROM collectionItems WHERE collectionID=?1 AND itemID=(SELECT itemID FROM articleItems WHERE article_id=?2)",turso::params![col.collection_id,article_id]).await?;
            if changed>0 {mark_membership_change(&tx,article_id).await?;}
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

    /// Update the role of an article within a collection.
    pub async fn update_article_role(
        &self,
        collection_id: &str,
        article_id: &str,
        role: ArticleRole,
    ) -> Result<()> {
        self.write_conn()
            .execute(
                "UPDATE collectionItems SET role = ?1 \
                 WHERE collectionID = (SELECT collectionID FROM collections WHERE id=?2) AND itemID = (SELECT itemID FROM articleItems WHERE article_id=?3)",
                turso::params![role.as_str(), collection_id, article_id],
            )
            .await?;
        Ok(())
    }

    /// Update the full-text fetch status of an article within a collection.
    pub async fn update_fetch_status(
        &self,
        collection_id: &str,
        article_id: &str,
        status: FetchStatus,
    ) -> Result<()> {
        self.write_conn()
            .execute(
                "UPDATE collectionItems SET fetch_status = ?1 \
                 WHERE collectionID = (SELECT collectionID FROM collections WHERE id=?2) AND itemID = (SELECT itemID FROM articleItems WHERE article_id=?3)",
                turso::params![status.as_str(), collection_id, article_id],
            )
            .await?;
        Ok(())
    }
    pub(crate) async fn add_to_collection_in_tx(
        conn: &turso::Connection,
        collection_id: &str,
        article_id: &str,
        role: ArticleRole,
        added_by: AddedBy,
        note: Option<&str>,
    ) -> Result<CollectionAddOutcome> {
        let collection = stored_collection(conn, collection_id).await?;
        require_editable(conn, collection.library_id).await?;
        if collection.deleted {
            return Err(Error::CollectionConflict("collection is in trash".into()));
        }
        {
            let mut rows=conn.query("SELECT i.libraryID FROM articleItems a JOIN items i USING(itemID) WHERE a.article_id=?1",turso::params![article_id]).await?;
            let row = rows
                .next()
                .await?
                .ok_or_else(|| Error::NotFound(format!("article {article_id}")))?;
            if row.get::<i64>(0)? != collection.library_id {
                return Err(Error::CollectionConflict(
                    "item belongs to another library".into(),
                ));
            }
        }
        // Fetch the existing row (if any) so we can report what changed.
        let existing = {
            let mut rows = conn
                .query(
                    "SELECT role, note FROM collection_articles \
                     WHERE collection_id = ?1 AND article_id = ?2",
                    turso::params![collection_id, article_id],
                )
                .await?;
            if let Some(row) = rows.next().await? {
                Some((
                    ArticleRole::from_str(&row.get::<String>(0)?),
                    opt_string(row.get_value(1)?),
                ))
            } else {
                None
            }
        };

        match existing {
            None => {
                let mut pos_rows = conn
                    .query(
                        "SELECT COALESCE(MAX(position), -1) + 1 \
                         FROM collection_articles WHERE collection_id = ?1",
                        turso::params![collection_id],
                    )
                    .await?;
                let position = pos_rows
                    .next()
                    .await?
                    .ok_or_else(|| Error::Unknown("MAX(position) returned no rows".into()))?
                    .get::<i64>(0)? as i32;
                drop(pos_rows);

                conn.execute(
                    "INSERT INTO collectionItems \
                     (collectionID, itemID, orderIndex, role, fetch_status, added_by, note, added_at) \
                     VALUES ((SELECT collectionID FROM collections WHERE id=?1), (SELECT itemID FROM articleItems WHERE article_id=?2), ?3, ?4, 'metadata_only', ?5, ?6, ?7)",
                    turso::params![
                        collection_id,
                        article_id,
                        position as i64,
                        role.as_str(),
                        added_by.as_str(),
                        note.map(|s| s.to_owned()),
                        Utc::now().to_rfc3339(),
                    ],
                )
                .await?;

                mark_membership_change(conn, article_id).await?;
                Ok(CollectionAddOutcome::Inserted)
            }
            Some((prev_role, prev_note)) => {
                // Preserve note when caller passes None; overwrite only
                // with an explicit new value.
                let note_changed = note.is_some() && note != prev_note.as_deref();
                let role_changed = prev_role != role;

                conn.execute(
                    "UPDATE collectionItems \
                     SET role = ?1, added_by = ?2, note = COALESCE(?3, note) \
                     WHERE collectionID = (SELECT collectionID FROM collections WHERE id=?4) AND itemID = (SELECT itemID FROM articleItems WHERE article_id=?5)",
                    turso::params![
                        role.as_str(),
                        added_by.as_str(),
                        note.map(|s| s.to_owned()),
                        collection_id,
                        article_id,
                    ],
                )
                .await?;

                Ok(CollectionAddOutcome::Updated {
                    previous_role: prev_role,
                    previous_note: prev_note,
                    role_changed,
                    note_changed,
                })
            }
        }
    }

    /// List the enriched [`CollectionArticle`] associations for a collection,
    /// optionally filtering by role and/or fetch status.
    pub async fn list_collection_articles(
        &self,
        collection_id: &str,
        role_filter: Option<ArticleRole>,
        fetch_filter: Option<FetchStatus>,
    ) -> Result<Vec<CollectionArticle>> {
        let conn = self.conn();
        let mut sql = String::from(
            "SELECT collection_id, article_id, position, role, fetch_status, \
                    added_by, note, added_at \
             FROM collection_articles WHERE collection_id = ?1",
        );
        let mut params: Vec<Value> = vec![collection_id.into()];
        let mut param_idx = 2u8;

        if let Some(role) = role_filter {
            sql.push_str(&format!(" AND role = ?{param_idx}"));
            params.push(role.as_str().into());
            param_idx += 1;
        }
        if let Some(fetch) = fetch_filter {
            sql.push_str(&format!(" AND fetch_status = ?{param_idx}"));
            params.push(fetch.as_str().into());
        }
        sql.push_str(" ORDER BY position");

        let mut rows = conn.query(sql, turso::params_from_iter(params)).await?;

        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            out.push(CollectionArticle {
                collection_id: row.get::<String>(0)?,
                article_id: row.get::<String>(1)?,
                position: row.get::<i64>(2)? as i32,
                role: ArticleRole::from_str(&row.get::<String>(3)?),
                fetch_status: FetchStatus::from_str(&row.get::<String>(4)?),
                added_by: AddedBy::from_str(&row.get::<String>(5)?),
                note: opt_string(row.get_value(6)?),
                added_at: opt_string(row.get_value(7)?).as_deref().and_then(parse_dt),
            });
        }
        Ok(out)
    }

    /// Hydrate every article in a collection in batch.
    ///
    /// This intentionally avoids the N+1 `get_article` loop used by early
    /// clients: collection order, article headers, authors, and identifiers
    /// are loaded with four bounded queries regardless of collection size.
    pub async fn list_articles_by_collection(&self, collection_id: &str) -> Result<Vec<Article>> {
        let associations = self
            .list_collection_articles(collection_id, None, None)
            .await?;
        self.hydrate_associations(&associations).await
    }

    async fn hydrate_associations(
        &self,
        associations: &[CollectionArticle],
    ) -> Result<Vec<Article>> {
        if associations.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.conn();
        let mut articles_by_id = std::collections::HashMap::new();
        for ids in associations.chunks(400) {
            let placeholders = (1..=ids.len())
                .map(|index| format!("?{index}"))
                .collect::<Vec<_>>()
                .join(", ");
            let params: Vec<Value> = ids
                .iter()
                .map(|association| Value::Text(association.article_id.clone()))
                .collect();
            let mut rows = conn
                .query(
                    &format!(
                        "SELECT id, title, abstract, year, month, journal, volume, issue, pages, \
                         issn, essn, language, pub_types, keywords, source \
                         FROM articles WHERE id IN ({placeholders})"
                    ),
                    turso::params_from_iter(params),
                )
                .await?;
            while let Some(row) = rows.next().await? {
                let mut article = Article::new(row.get::<String>(0)?, row.get::<String>(1)?);
                article.abstract_text = crate::bib_base::opt_string(row.get_value(2)?);
                article.year = crate::bib_base::opt_int(row.get_value(3)?).map(|year| year as u16);
                article.month =
                    crate::bib_base::opt_int(row.get_value(4)?).map(|month| month as u8);
                article.journal = crate::bib_base::opt_string(row.get_value(5)?);
                article.volume = crate::bib_base::opt_string(row.get_value(6)?);
                article.issue = crate::bib_base::opt_string(row.get_value(7)?);
                article.pages = crate::bib_base::opt_string(row.get_value(8)?);
                article.issn = crate::bib_base::opt_string(row.get_value(9)?);
                article.essn = crate::bib_base::opt_string(row.get_value(10)?);
                article.language = crate::bib_base::opt_string(row.get_value(11)?);
                article.pub_types = crate::bib_base::parse_json_col(&crate::bib_base::opt_string(
                    row.get_value(12)?,
                ));
                article.keywords = crate::bib_base::parse_json_col(&crate::bib_base::opt_string(
                    row.get_value(13)?,
                ));
                article.source = crate::bib_base::opt_string(row.get_value(14)?)
                    .as_deref()
                    .and_then(|source| serde_json::from_str(source).ok())
                    .unwrap_or_default();
                articles_by_id.insert(article.id.clone(), article);
            }
        }

        let ids: Vec<String> = associations
            .iter()
            .map(|association| association.article_id.clone())
            .collect();
        for ids in ids.chunks(400) {
            let placeholders = (1..=ids.len())
                .map(|index| format!("?{index}"))
                .collect::<Vec<_>>()
                .join(", ");
            let params: Vec<Value> = ids.iter().map(|id| Value::Text(id.clone())).collect();

            let mut author_rows = conn
                .query(
                    &format!(
                        "SELECT article_id, last_name, fore_name, initials, affiliation, orcid, corresponding \
                         FROM authors WHERE article_id IN ({placeholders}) ORDER BY article_id, position"
                    ),
                    turso::params_from_iter(params.clone()),
                )
                .await?;
            while let Some(row) = author_rows.next().await? {
                let article_id = row.get::<String>(0)?;
                if let Some(article) = articles_by_id.get_mut(&article_id) {
                    article.authors.push(bib_types::Author {
                        last_name: row.get::<String>(1)?,
                        fore_name: crate::bib_base::opt_string(row.get_value(2)?),
                        initials: crate::bib_base::opt_string(row.get_value(3)?),
                        affiliation: crate::bib_base::opt_string(row.get_value(4)?),
                        orcid: crate::bib_base::opt_string(row.get_value(5)?),
                        corresponding: row.get::<i64>(6)? != 0,
                    });
                }
            }

            let mut identifier_rows = conn
                .query(
                    &format!(
                        "SELECT article_id, kind, value FROM identifiers \
                         WHERE article_id IN ({placeholders}) ORDER BY article_id"
                    ),
                    turso::params_from_iter(params),
                )
                .await?;
            while let Some(row) = identifier_rows.next().await? {
                let article_id = row.get::<String>(0)?;
                let kind = match row.get::<String>(1)?.as_str() {
                    "doi" => IdKind::Doi,
                    "pmid" => IdKind::Pmid,
                    "pmc" => IdKind::Pmc,
                    "arxiv" => IdKind::Arxiv,
                    "s2" => IdKind::S2,
                    "openalex" => IdKind::OpenAlex,
                    _ => IdKind::Other,
                };
                if let Some(article) = articles_by_id.get_mut(&article_id) {
                    article
                        .identifiers
                        .push(Identifier::new(kind, row.get::<String>(2)?));
                }
            }
        }

        Ok(associations
            .iter()
            .filter_map(|association| articles_by_id.remove(&association.article_id))
            .collect())
    }

    /// Apply collection scope and lexical full-text matching before count/paging.
    pub async fn query_collection_articles(
        &self,
        input: CollectionQuery,
    ) -> Result<CollectionPage> {
        let conn = self.conn();
        let scope = if let Some(id) = &input.collection_id {
            let col = stored_collection(&conn, id).await?;
            if col.deleted {
                return Err(Error::NotFound(format!("collection {id}")));
            }
            let ids = if input.include_descendants {
                subtree_ids(&conn, id).await?
            } else {
                vec![(id.clone(), col.collection_id)]
            };
            let ids = ids
                .iter()
                .map(|(_, native)| native.to_string())
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "EXISTS (SELECT 1 FROM collectionItems ci JOIN collections c USING(collectionID) JOIN articleItems ai USING(itemID) WHERE ai.article_id=a.id AND c.collectionID IN ({ids}) AND NOT EXISTS (SELECT 1 FROM deletedCollections d WHERE d.collectionID=c.collectionID))"
            )
        } else if input.unfiled {
            "NOT EXISTS (SELECT 1 FROM collection_articles ca WHERE ca.article_id=a.id)".to_owned()
        } else {
            "1=1".to_owned()
        };
        let (ids, total) = if input.query.trim().is_empty() {
            let mut rows = conn
                .query(format!("SELECT COUNT(*) FROM articles a WHERE {scope}"), ())
                .await?;
            let total = rows
                .next()
                .await?
                .ok_or_else(|| Error::Unknown("count returned no row".into()))?
                .get::<i64>(0)?;
            drop(rows);
            let mut rows = conn.query(format!("SELECT a.id FROM articles a WHERE {scope} ORDER BY a.title COLLATE NOCASE,a.id LIMIT ?1 OFFSET ?2"), turso::params![input.limit as i64,input.offset as i64]).await?;
            let mut ids = Vec::new();
            while let Some(row) = rows.next().await? {
                ids.push(row.get::<String>(0)?);
            }
            (ids, total)
        } else {
            let mut rows = conn
                .query(format!("SELECT a.id FROM articles a WHERE {scope}"), ())
                .await?;
            let mut allowed = std::collections::HashSet::new();
            while let Some(row) = rows.next().await? {
                allowed.insert(row.get::<String>(0)?);
            }
            drop(rows);
            let ranked = self.local_matches_legacy(&input.query).await?;
            let scoped = ranked
                .into_iter()
                .filter(|hit| allowed.contains(&hit.article_id))
                .collect::<Vec<_>>();
            let total = scoped.len() as i64;
            let ids = scoped
                .into_iter()
                .skip(input.offset)
                .take(input.limit)
                .map(|hit| hit.article_id)
                .collect::<Vec<_>>();
            (ids, total)
        };
        let mut associations = Vec::new();
        // Fetch all direct provenance for the page, including memberships in
        // other branches. Display deduplication never discards relationship data.
        for chunk in ids.chunks(400) {
            let placeholders = (1..=chunk.len())
                .map(|i| format!("?{i}"))
                .collect::<Vec<_>>()
                .join(",");
            let mut rows=conn.query(format!("SELECT collection_id,article_id,position,role,fetch_status,added_by,note,added_at FROM collection_articles WHERE article_id IN ({placeholders}) ORDER BY collection_id,position"),turso::params_from_iter(chunk.iter().cloned().map(Value::Text))).await?;
            while let Some(row) = rows.next().await? {
                associations.push(CollectionArticle {
                    collection_id: row.get(0)?,
                    article_id: row.get(1)?,
                    position: row.get::<i64>(2)? as i32,
                    role: ArticleRole::from_str(&row.get::<String>(3)?),
                    fetch_status: FetchStatus::from_str(&row.get::<String>(4)?),
                    added_by: AddedBy::from_str(&row.get::<String>(5)?),
                    note: opt_string(row.get_value(6)?),
                    added_at: opt_string(row.get_value(7)?).as_deref().and_then(parse_dt),
                });
            }
        }
        let hydration = ids
            .iter()
            .map(|id| CollectionArticle {
                collection_id: String::new(),
                article_id: id.clone(),
                position: 0,
                role: ArticleRole::Referenced,
                fetch_status: FetchStatus::MetadataOnly,
                added_by: AddedBy::User,
                note: None,
                added_at: None,
            })
            .collect::<Vec<_>>();
        let articles = self.hydrate_associations(&hydration).await?;
        Ok(CollectionPage {
            articles,
            associations,
            total,
        })
    }

    /// List all collection-article entries that are waiting for a
    /// full-text upload (`fetch_status = 'fulltext_requested'`).
    ///
    /// If `collection_id` is `None`, searches across all collections.
    pub async fn list_fulltext_requests(
        &self,
        collection_id: Option<&str>,
    ) -> Result<Vec<CollectionArticle>> {
        let conn = self.conn();
        let mut rows = match collection_id {
            Some(cid) => {
                conn.query(
                    "SELECT collection_id, article_id, position, role, fetch_status, \
                            added_by, note, added_at \
                     FROM collection_articles \
                     WHERE collection_id = ?1 AND fetch_status = 'fulltext_requested' \
                     ORDER BY added_at DESC",
                    turso::params![cid],
                )
                .await?
            }
            None => {
                conn.query(
                    "SELECT collection_id, article_id, position, role, fetch_status, \
                            added_by, note, added_at \
                     FROM collection_articles \
                     WHERE fetch_status = 'fulltext_requested' \
                     ORDER BY added_at DESC",
                    turso::params![],
                )
                .await?
            }
        };

        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            out.push(CollectionArticle {
                collection_id: row.get::<String>(0)?,
                article_id: row.get::<String>(1)?,
                position: row.get::<i64>(2)? as i32,
                role: ArticleRole::from_str(&row.get::<String>(3)?),
                fetch_status: FetchStatus::from_str(&row.get::<String>(4)?),
                added_by: AddedBy::from_str(&row.get::<String>(5)?),
                note: opt_string(row.get_value(6)?),
                added_at: opt_string(row.get_value(7)?).as_deref().and_then(parse_dt),
            });
        }
        Ok(out)
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Return the ordered list of article IDs in a collection.
    async fn collection_article_ids(&self, collection_id: &str) -> Result<Vec<String>> {
        let conn = self.conn();
        let mut rows = conn
            .query(
                "SELECT article_id FROM collection_articles \
                 WHERE collection_id = ?1 ORDER BY position",
                turso::params![collection_id],
            )
            .await?;
        let mut ids = Vec::new();
        while let Some(row) = rows.next().await? {
            ids.push(row.get::<String>(0)?);
        }
        Ok(ids)
    }
}

// ---------------------------------------------------------------------------
// BibBase — thin handle methods, one command round-trip each
// ---------------------------------------------------------------------------

impl BibBase {
    pub async fn ensure_collection_membership(
        &self,
        collection_id: &str,
        article_id: &str,
    ) -> Result<CollectionAddOutcome> {
        self.call(|reply| BibCommand::EnsureCollectionMembership {
            collection_id: collection_id.to_owned(),
            article_id: article_id.to_owned(),
            reply,
        })
        .await
    }

    pub async fn list_collection_summaries(
        &self,
        include_deleted: bool,
    ) -> Result<Vec<serde_json::Value>> {
        self.call(|reply| BibCommand::ListCollectionSummaries {
            include_deleted,
            reply,
        })
        .await
    }

    pub async fn query_collection_articles(
        &self,
        input: CollectionQuery,
    ) -> Result<CollectionPage> {
        self.call(|reply| BibCommand::QueryCollectionArticles { input, reply })
            .await
    }

    pub async fn change_collection(
        &self,
        id: &str,
        change: CollectionChange,
    ) -> Result<Vec<String>> {
        self.call(|reply| BibCommand::ChangeCollection {
            id: id.to_owned(),
            change,
            reply,
        })
        .await
    }
    pub async fn list_collections_including_deleted(
        &self,
        status: Option<CollectionStatus>,
        include_deleted: bool,
    ) -> Result<Vec<Collection>> {
        self.call(|reply| BibCommand::ListCollections {
            status,
            include_deleted,
            reply,
        })
        .await
    }
    /// Insert or update a collection.
    pub async fn upsert_collection(&self, collection: &Collection) -> Result<()> {
        self.call(|reply| BibCommand::UpsertCollection {
            collection: collection.clone(),
            reply,
        })
        .await
    }

    /// Look up a single collection by ID, with its article IDs hydrated
    /// in order.
    pub async fn get_collection(&self, id: &str) -> Result<Option<Collection>> {
        self.call(|reply| BibCommand::GetCollection {
            id: id.to_owned(),
            reply,
        })
        .await
    }

    /// List collections, optionally filtered by status.
    pub async fn list_collections(
        &self,
        status: Option<CollectionStatus>,
    ) -> Result<Vec<Collection>> {
        self.call(|reply| BibCommand::ListCollections {
            status,
            include_deleted: false,
            reply,
        })
        .await
    }

    /// Update the status of a collection.
    pub async fn update_collection_status(&self, id: &str, status: CollectionStatus) -> Result<()> {
        self.call(|reply| BibCommand::UpdateCollectionStatus {
            id: id.to_owned(),
            status,
            reply,
        })
        .await
    }

    /// Delete a collection.
    pub async fn delete_collection(&self, id: &str) -> Result<()> {
        self.call(|reply| BibCommand::DeleteCollection {
            id: id.to_owned(),
            reply,
        })
        .await
    }

    /// Associate an article with a collection.
    pub async fn add_to_collection(
        &self,
        collection_id: &str,
        article_id: &str,
        role: ArticleRole,
        added_by: AddedBy,
        note: Option<&str>,
    ) -> Result<CollectionAddOutcome> {
        self.call(|reply| BibCommand::AddToCollection {
            collection_id: collection_id.to_owned(),
            article_id: article_id.to_owned(),
            role,
            added_by,
            note: note.map(str::to_owned),
            reply,
        })
        .await
    }

    /// Upsert articles and associate them with one collection atomically.
    pub async fn upsert_articles_with_collection(
        &self,
        articles: &[Article],
        collection_id: &str,
        assignments: &[CollectionAssignment],
    ) -> Result<Vec<CollectionAssignmentOutcome>> {
        self.call(|reply| BibCommand::UpsertArticlesWithCollection {
            articles: articles.to_vec(),
            collection_id: collection_id.to_owned(),
            assignments: assignments.to_vec(),
            reply,
        })
        .await
    }

    /// Remove an article from a collection.
    pub async fn remove_from_collection(
        &self,
        collection_id: &str,
        article_id: &str,
    ) -> Result<()> {
        self.call(|reply| BibCommand::RemoveFromCollection {
            collection_id: collection_id.to_owned(),
            article_id: article_id.to_owned(),
            reply,
        })
        .await
    }

    /// Update the role of an article within a collection.
    pub async fn update_article_role(
        &self,
        collection_id: &str,
        article_id: &str,
        role: ArticleRole,
    ) -> Result<()> {
        self.call(|reply| BibCommand::UpdateArticleRole {
            collection_id: collection_id.to_owned(),
            article_id: article_id.to_owned(),
            role,
            reply,
        })
        .await
    }

    /// Update the full-text fetch status of an article within a collection.
    pub async fn update_fetch_status(
        &self,
        collection_id: &str,
        article_id: &str,
        status: FetchStatus,
    ) -> Result<()> {
        self.call(|reply| BibCommand::UpdateFetchStatus {
            collection_id: collection_id.to_owned(),
            article_id: article_id.to_owned(),
            status,
            reply,
        })
        .await
    }

    /// List the enriched [`CollectionArticle`] associations for a collection.
    pub async fn list_collection_articles(
        &self,
        collection_id: &str,
        role_filter: Option<ArticleRole>,
        fetch_filter: Option<FetchStatus>,
    ) -> Result<Vec<CollectionArticle>> {
        self.call(|reply| BibCommand::ListCollectionArticles {
            collection_id: collection_id.to_owned(),
            role_filter,
            fetch_filter,
            reply,
        })
        .await
    }

    /// Load all articles in a collection as one batch, preserving position.
    pub async fn list_articles_by_collection(&self, collection_id: &str) -> Result<Vec<Article>> {
        self.call(|reply| BibCommand::ListArticlesByCollection {
            collection_id: collection_id.to_owned(),
            reply,
        })
        .await
    }

    /// List all collection-article entries waiting for a full-text upload.
    pub async fn list_fulltext_requests(
        &self,
        collection_id: Option<&str>,
    ) -> Result<Vec<CollectionArticle>> {
        self.call(|reply| BibCommand::ListFulltextRequests {
            collection_id: collection_id.map(str::to_owned),
            reply,
        })
        .await
    }
}

// ---------------------------------------------------------------------------
// Value extraction helpers — shared with bib_base.rs but duplicated here
// to avoid visibility issues. (These are trivial functions.)
// ---------------------------------------------------------------------------

fn opt_string(v: Value) -> Option<String> {
    match v {
        Value::Text(s) => Some(s),
        Value::Null => None,
        _ => None,
    }
}

fn parse_json_col(raw: &Option<String>) -> Vec<String> {
    raw.as_ref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default()
}

fn parse_dt(s: &str) -> Option<chrono::DateTime<Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}
