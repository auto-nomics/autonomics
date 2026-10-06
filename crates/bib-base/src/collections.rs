//! Collection management — CRUD for [`Collection`] and the enriched
//! [`CollectionArticle`] association that links articles to collections.
//!
//! All methods are on [`BibBase`] and share the same Turso connection.

use chrono::Utc;
use turso::{
    Value,
    transaction::{Transaction, TransactionBehavior},
};

use crate::bib_base::BibBase;
use crate::error::{Error, Result};
use bib_types::{
    AddedBy, Article, ArticleRole, Collection, CollectionArticle, CollectionStatus, FetchStatus,
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

impl BibBase {
    /// Insert or update a collection. Child rows (`collection_articles`)
    /// are **not** touched — use [`Self::add_to_collection`] for that.
    pub async fn upsert_collection(&self, collection: &Collection) -> Result<()> {
        let _write = self.write_gate.lock().await;
        let conn = self.write_conn();
        conn.execute(
            "INSERT INTO collections \
             (id, name, description, tags, status, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
             ON CONFLICT(id) DO UPDATE SET \
                name = excluded.name, \
                description = excluded.description, \
                tags = excluded.tags, \
                status = excluded.status, \
                created_at = COALESCE(collections.created_at, excluded.created_at), \
                updated_at = excluded.updated_at",
            turso::params![
                collection.id.clone(),
                collection.name.clone(),
                collection.description.clone(),
                serde_json::to_string(&collection.tags)?,
                collection.status.as_str(),
                collection.created_at.map(|t| t.to_rfc3339()),
                collection.updated_at.map(|t| t.to_rfc3339()),
            ],
        )
        .await?;
        Ok(())
    }

    /// Look up a single collection by ID, with its article IDs hydrated
    /// in order.
    pub async fn get_collection(&self, id: &str) -> Result<Option<Collection>> {
        let conn = self.conn();
        let mut rows = conn
            .query(
                "SELECT id, name, description, tags, status, created_at, updated_at \
                 FROM collections WHERE id = ?1",
                turso::params![id],
            )
            .await?;

        let row = match rows.next().await? {
            Some(row) => row,
            None => return Ok(None),
        };

        let mut col = Collection::new(row.get::<String>(0)?, row.get::<String>(1)?);
        col.description = opt_string(row.get_value(2)?);
        col.tags = parse_json_col(&opt_string(row.get_value(3)?));
        col.status = CollectionStatus::from_str(&opt_string(row.get_value(4)?).unwrap_or_default());
        col.created_at = opt_string(row.get_value(5)?).as_deref().and_then(parse_dt);
        col.updated_at = opt_string(row.get_value(6)?).as_deref().and_then(parse_dt);

        // Hydrate article_ids in position order.
        col.article_ids = self.collection_article_ids(id).await?;

        Ok(Some(col))
    }

    /// List collections, optionally filtered by status.
    ///
    /// Each collection's `article_ids` are batch-hydrated in position order
    /// via a single extra query, so `article_ids.len()` gives the correct
    /// article count without an N+1 pattern.
    pub async fn list_collections(
        &self,
        status: Option<CollectionStatus>,
    ) -> Result<Vec<Collection>> {
        let conn = self.conn();
        let sql = match status {
            Some(_) => {
                "SELECT id, name, description, tags, status, created_at, updated_at \
                        FROM collections WHERE status = ?1 ORDER BY updated_at DESC"
            }
            None => {
                "SELECT id, name, description, tags, status, created_at, updated_at \
                     FROM collections ORDER BY updated_at DESC"
            }
        };

        let mut rows = if let Some(s) = status {
            conn.query(sql, turso::params![s.as_str()]).await?
        } else {
            conn.query(sql, turso::params![]).await?
        };

        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            let mut col = Collection::new(row.get::<String>(0)?, row.get::<String>(1)?);
            col.description = opt_string(row.get_value(2)?);
            col.tags = parse_json_col(&opt_string(row.get_value(3)?));
            col.status =
                CollectionStatus::from_str(&opt_string(row.get_value(4)?).unwrap_or_default());
            col.created_at = opt_string(row.get_value(5)?).as_deref().and_then(parse_dt);
            col.updated_at = opt_string(row.get_value(6)?).as_deref().and_then(parse_dt);
            out.push(col);
        }
        drop(rows);

        // Batch-hydrate article_ids for all collections in one query,
        // grouped in Rust. Without this, callers that display article
        // counts (e.g. `bib_list_collection`'s list-all path) would see 0
        // for every collection — the data is in the DB but never reaches
        // the user, which looks like a persistence bug on restart.
        if !out.is_empty() {
            let ids: Vec<&str> = out.iter().map(|c| c.id.as_str()).collect();
            let placeholders = (0..ids.len())
                .map(|i| format!("?{}", i + 1))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "SELECT collection_id, article_id FROM collection_articles \
                 WHERE collection_id IN ({placeholders}) ORDER BY collection_id, position"
            );
            let params: Vec<Value> = ids.into_iter().map(|s| Value::Text(s.to_owned())).collect();
            let mut ca_rows = conn.query(sql, turso::params_from_iter(params)).await?;
            // Index into `out` by collection id for O(1) assignment.
            // Owns the id strings so the map doesn't borrow `out`.
            let mut idx: std::collections::HashMap<String, usize> =
                std::collections::HashMap::with_capacity(out.len());
            for (i, c) in out.iter().enumerate() {
                idx.insert(c.id.clone(), i);
            }
            while let Some(row) = ca_rows.next().await? {
                let cid = row.get::<String>(0)?;
                let aid = row.get::<String>(1)?;
                if let Some(&i) = idx.get(&cid) {
                    out[i].article_ids.push(aid);
                }
            }
        }

        Ok(out)
    }

    /// Update the status of a collection.
    pub async fn update_collection_status(&self, id: &str, status: CollectionStatus) -> Result<()> {
        let _write = self.write_gate.lock().await;
        let conn = self.write_conn();
        conn.execute(
            "UPDATE collections SET status = ?1, updated_at = ?2 WHERE id = ?3",
            turso::params![status.as_str(), Utc::now().to_rfc3339(), id],
        )
        .await?;
        Ok(())
    }

    /// Delete a collection (FK CASCADE removes `collection_articles` rows).
    pub async fn delete_collection(&self, id: &str) -> Result<()> {
        let _write = self.write_gate.lock().await;
        self.write_conn()
            .execute("DELETE FROM collections WHERE id = ?1", turso::params![id])
            .await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Collection ↔ Article association
    // -----------------------------------------------------------------------

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
        let _write = self.write_gate.lock().await;
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
        let _write = self.write_gate.lock().await;
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;

        async fn run(
            tx: &Transaction<'_>,
            articles: &[Article],
            collection_id: &str,
            assignments: &[CollectionAssignment],
        ) -> Result<Vec<CollectionAssignmentOutcome>> {
            for article in articles {
                BibBase::upsert_article_in_tx(tx, article).await?;
                crate::bib_base::sync_search_index(tx, &article.id).await?;
            }

            let mut outcomes = Vec::with_capacity(assignments.len());
            for assignment in assignments {
                let outcome = BibBase::add_to_collection_in_tx(
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
        let _write = self.write_gate.lock().await;
        self.write_conn()
            .execute(
                "DELETE FROM collection_articles \
                 WHERE collection_id = ?1 AND article_id = ?2",
                turso::params![collection_id, article_id],
            )
            .await?;
        Ok(())
    }

    /// Update the role of an article within a collection.
    pub async fn update_article_role(
        &self,
        collection_id: &str,
        article_id: &str,
        role: ArticleRole,
    ) -> Result<()> {
        let _write = self.write_gate.lock().await;
        self.write_conn()
            .execute(
                "UPDATE collection_articles SET role = ?1 \
                 WHERE collection_id = ?2 AND article_id = ?3",
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
        let _write = self.write_gate.lock().await;
        self.write_conn()
            .execute(
                "UPDATE collection_articles SET fetch_status = ?1 \
                 WHERE collection_id = ?2 AND article_id = ?3",
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

                conn.execute(
                    "INSERT INTO collection_articles \
                     (collection_id, article_id, position, role, fetch_status, added_by, note, added_at) \
                     VALUES (?1, ?2, ?3, ?4, 'metadata_only', ?5, ?6, ?7)",
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

                Ok(CollectionAddOutcome::Inserted)
            }
            Some((prev_role, prev_note)) => {
                // Preserve note when caller passes None; overwrite only
                // with an explicit new value.
                let note_changed = note.is_some() && note != prev_note.as_deref();
                let role_changed = prev_role != role;

                conn.execute(
                    "UPDATE collection_articles \
                     SET role = ?1, added_by = ?2, note = COALESCE(?3, note) \
                     WHERE collection_id = ?4 AND article_id = ?5",
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
