//! Turso-backed persistent memory repository.
mod knowledge;
mod memory_store;

use std::path::Path;
use std::sync::Arc;

use tokio::sync::Mutex;
use turso::{Builder, IntoParams, Value, params_from_iter};
use uuid::Uuid;

use super::error::{Error, Result};
use crate::knowledge::{Knowledge, KnowledgePath};
use crate::store::{MemoryNote, MemoryStage1Record, MemorySummary, SemanticObservation};

const MIGRATION_MARKER: &str = "legacy_agent_db_memory_migrated_v1";
const KNOWLEDGE_TREE_MIGRATION_MARKER: &str = "memory_entries_to_knowledge_nodes_v1";

#[derive(Clone)]
pub struct TursoMemoryStorage {
    conn: LockedConn,
}

#[derive(Clone)]
struct LockedConn(Arc<Mutex<turso::Connection>>);

struct LockedRows {
    rows: std::vec::IntoIter<turso::Row>,
}

impl LockedRows {
    async fn next(&mut self) -> turso::Result<Option<turso::Row>> {
        Ok(self.rows.next())
    }
}

impl LockedConn {
    async fn execute(&self, sql: impl AsRef<str>, params: impl IntoParams) -> turso::Result<u64> {
        self.0.lock().await.execute(sql, params).await
    }

    async fn query(
        &self,
        sql: impl AsRef<str>,
        params: impl IntoParams,
    ) -> turso::Result<LockedRows> {
        let guard = self.0.lock().await;
        let mut rows = guard.query(sql, params).await?;
        let mut owned = Vec::new();
        while let Some(row) = rows.next().await? {
            owned.push(row);
        }
        Ok(LockedRows {
            rows: owned.into_iter(),
        })
    }

    async fn execute_batch(&self, sql: impl AsRef<str>) -> turso::Result<()> {
        self.0.lock().await.execute_batch(sql).await
    }
}

fn db_err(error: impl std::fmt::Display) -> Error {
    Error::Storage(error.to_string())
}

fn uuid_value(id: Uuid) -> Value {
    Value::Text(id.to_string())
}

fn text(row: &turso::Row, idx: usize) -> Result<String> {
    match row.get_value(idx).map_err(db_err)? {
        Value::Text(value) => Ok(value),
        Value::Null => Ok(String::new()),
        other => Err(Error::Storage(format!(
            "expected TEXT at column {idx}, got {other:?}"
        ))),
    }
}

fn opt_text(row: &turso::Row, idx: usize) -> Result<Option<String>> {
    match row.get_value(idx).map_err(db_err)? {
        Value::Text(value) => Ok(Some(value)),
        _ => Ok(None),
    }
}

fn int(row: &turso::Row, idx: usize) -> Result<i64> {
    match row.get_value(idx).map_err(db_err)? {
        Value::Integer(value) => Ok(value),
        other => Err(Error::Storage(format!(
            "expected INTEGER at column {idx}, got {other:?}"
        ))),
    }
}

fn real(row: &turso::Row, idx: usize) -> Result<f64> {
    match row.get_value(idx).map_err(db_err)? {
        Value::Real(value) => Ok(value),
        Value::Integer(value) => Ok(value as f64),
        other => Err(Error::Storage(format!(
            "expected REAL at column {idx}, got {other:?}"
        ))),
    }
}

fn parse_uuid(value: String, field: &str) -> Result<Uuid> {
    Uuid::parse_str(&value).map_err(|e| Error::Storage(format!("parse memory {field}: {e}")))
}

fn row_to_stage1(row: &turso::Row) -> Result<MemoryStage1Record> {
    Ok(MemoryStage1Record {
        session_id: parse_uuid(text(row, 0)?, "session_id")?,
        source_hash: text(row, 1)?,
        raw_memory: text(row, 2)?,
        rollout_summary: text(row, 3)?,
        rollout_slug: opt_text(row, 4)?.filter(|value| !value.is_empty()),
        status: text(row, 5)?,
        generated_at: int(row, 6)?,
        lease_until: int(row, 7)?,
    })
}

struct LegacyMemoryEntry {
    id: Uuid,
    entry_type: String,
    title: String,
    body_md: String,
    status: String,
    confidence: f64,
    created_at: i64,
    updated_at: i64,
}

fn row_to_entry(row: &turso::Row) -> Result<LegacyMemoryEntry> {
    Ok(LegacyMemoryEntry {
        id: parse_uuid(text(row, 0)?, "entry_id")?,
        entry_type: text(row, 2)?,
        title: text(row, 3)?,
        body_md: text(row, 4)?,
        status: text(row, 5)?,
        confidence: real(row, 6)?,
        created_at: int(row, 7)?,
        updated_at: int(row, 8)?,
    })
}

fn row_to_knowledge(row: &turso::Row) -> Result<Knowledge> {
    let id = text(row, 0)?;
    let segments = text(row, 2)?
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    Ok(Knowledge::new(
        parse_uuid(id.clone(), "knowledge_id")?,
        opt_text(row, 1)?
            .map(|value| parse_uuid(value, "parent_id"))
            .transpose()?,
        KnowledgePath::from_segments(segments)
            .ok_or_else(|| Error::Storage(format!("invalid knowledge path in row {id}")))?,
        text(row, 3)?,
        text(row, 4)?,
        text(row, 5)?,
        real(row, 6)?,
        text(row, 7)?,
        int(row, 8)? != 0,
        int(row, 9)?,
        int(row, 10)?,
    ))
}

fn row_to_summary(row: &turso::Row) -> Result<MemorySummary> {
    Ok(MemorySummary {
        scope_id: parse_uuid(text(row, 0)?, "scope_id")?,
        schema_version: text(row, 1)?,
        summary_md: text(row, 2)?,
        source_hash: text(row, 3)?,
        generated_at: int(row, 4)?,
    })
}

fn row_to_note(row: &turso::Row) -> Result<MemoryNote> {
    Ok(MemoryNote {
        id: parse_uuid(text(row, 0)?, "note_id")?,
        scope_id: parse_uuid(text(row, 1)?, "scope_id")?,
        slug: text(row, 2)?,
        content: text(row, 3)?,
        status: text(row, 4)?,
        created_at: int(row, 5)?,
    })
}

fn row_to_observation(row: &turso::Row) -> Result<SemanticObservation> {
    Ok(SemanticObservation {
        id: parse_uuid(text(row, 0)?, "observation_id")?,
        scope_id: parse_uuid(text(row, 1)?, "scope_id")?,
        subject: text(row, 2)?,
        predicate: text(row, 3)?,
        object: text(row, 4)?,
        content: text(row, 5)?,
        status: text(row, 6)?,
        confidence: real(row, 7)?,
        source_hash: text(row, 8)?,
        created_at: int(row, 9)?,
        last_error: opt_text(row, 10)?.filter(|value| !value.is_empty()),
    })
}

impl TursoMemoryStorage {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path_ref = path.as_ref();
        let path = path_ref
            .to_str()
            .ok_or_else(|| Error::Storage("memory db path is not valid UTF-8".into()))?;
        if let Some(parent) = path_ref.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::Storage(format!("create memory db parent: {e}")))?;
        }

        let db = Builder::new_local(path)
            .experimental_multiprocess_wal(true)
            .build()
            .await
            .map_err(db_err)?;
        let conn = db.connect().map_err(db_err)?;
        conn.pragma_update("busy_timeout", 5000)
            .await
            .map_err(db_err)?;
        let storage = Self {
            conn: LockedConn(Arc::new(Mutex::new(conn))),
        };
        storage.init_schema().await?;
        Ok(storage)
    }

    pub async fn open_in_memory() -> Result<Self> {
        let db = Builder::new_local(":memory:")
            .build()
            .await
            .map_err(db_err)?;
        let conn = db.connect().map_err(db_err)?;
        let storage = Self {
            conn: LockedConn(Arc::new(Mutex::new(conn))),
        };
        storage.init_schema().await?;
        Ok(storage)
    }

    async fn init_schema(&self) -> Result<()> {
        self.conn
            .execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS memory_stage1 (
                    scope_id TEXT NOT NULL,
                    session_id TEXT NOT NULL,
                    source_hash TEXT NOT NULL,
                    raw_memory TEXT NOT NULL,
                    rollout_summary TEXT NOT NULL,
                    rollout_slug TEXT,
                    status TEXT NOT NULL,
                    generated_at INTEGER NOT NULL,
                    lease_until INTEGER NOT NULL DEFAULT 0,
                    attempts INTEGER NOT NULL DEFAULT 0,
                    last_error TEXT,
                    updated_at INTEGER NOT NULL,
                    PRIMARY KEY (scope_id, session_id)
                );
                CREATE INDEX IF NOT EXISTS idx_memory_stage1_generated
                    ON memory_stage1(generated_at DESC);
                CREATE TABLE IF NOT EXISTS memory_jobs (
                    scope_id TEXT PRIMARY KEY,
                    status TEXT NOT NULL,
                    source_hash TEXT NOT NULL,
                    lease_until INTEGER NOT NULL,
                    attempts INTEGER NOT NULL DEFAULT 0,
                    last_error TEXT,
                    updated_at INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS memory_entries (
                    id TEXT PRIMARY KEY,
                    scope_id TEXT NOT NULL,
                    entry_type TEXT NOT NULL,
                    title TEXT NOT NULL,
                    body_md TEXT NOT NULL,
                    status TEXT NOT NULL,
                    confidence REAL NOT NULL,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_memory_entries_scope_status
                    ON memory_entries(scope_id, status, updated_at DESC);
                CREATE TABLE IF NOT EXISTS knowledge_nodes (
                    id TEXT PRIMARY KEY,
                    parent_id TEXT,
                    knowledge_path TEXT NOT NULL,
                    title TEXT NOT NULL,
                    body_md TEXT NOT NULL,
                    status TEXT NOT NULL,
                    confidence REAL NOT NULL,
                    source_hash TEXT NOT NULL,
                    structural INTEGER NOT NULL DEFAULT 0,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    UNIQUE(knowledge_path)
                );
                CREATE INDEX IF NOT EXISTS idx_knowledge_nodes_parent
                    ON knowledge_nodes(parent_id);
                CREATE TABLE IF NOT EXISTS memory_summaries (
                    scope_id TEXT PRIMARY KEY,
                    schema_version TEXT NOT NULL,
                    summary_md TEXT NOT NULL,
                    source_hash TEXT NOT NULL,
                    generated_at INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS memory_notes (
                    id TEXT PRIMARY KEY,
                    scope_id TEXT NOT NULL,
                    slug TEXT NOT NULL,
                    content TEXT NOT NULL,
                    status TEXT NOT NULL,
                    created_at INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_memory_notes_scope_status
                    ON memory_notes(scope_id, status, created_at);
                CREATE TABLE IF NOT EXISTS memory_semantic_observations (
                    id TEXT PRIMARY KEY,
                    scope_id TEXT NOT NULL,
                    subject TEXT NOT NULL,
                    predicate TEXT NOT NULL,
                    object TEXT NOT NULL,
                    content TEXT NOT NULL,
                    status TEXT NOT NULL,
                    confidence REAL NOT NULL,
                    source_hash TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    last_error TEXT
                );
                CREATE INDEX IF NOT EXISTS idx_memory_semantic_scope_status
                    ON memory_semantic_observations(scope_id, status, created_at DESC);
                CREATE TABLE IF NOT EXISTS memory_storage_metadata (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );
                "#,
            )
            .await
            .map_err(db_err)?;
        self.migrate_knowledge_nodes_to_global_schema().await?;
        self.conn
            .execute_batch(
                "CREATE INDEX IF NOT EXISTS idx_knowledge_nodes_status
                    ON knowledge_nodes(status, updated_at DESC);",
            )
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn knowledge_nodes_have_scope_column(&self) -> Result<bool> {
        let mut rows = self
            .conn
            .query(
                "SELECT 1 FROM pragma_table_info('knowledge_nodes') WHERE name = 'scope_id'",
                (),
            )
            .await
            .map_err(db_err)?;
        Ok(rows.next().await.map_err(db_err)?.is_some())
    }

    /// Convert the transitional scoped knowledge tree to the global schema.
    ///
    /// The tree is intentionally global: its namespace is `knowledge_path`.
    /// Legacy memory jobs and notes remain scoped until their pipeline is
    /// migrated, but knowledge-tree identity no longer carries a scope.
    async fn migrate_knowledge_nodes_to_global_schema(&self) -> Result<()> {
        if !self.knowledge_nodes_have_scope_column().await? {
            return Ok(());
        }

        let mut scopes = self
            .conn
            .query("SELECT COUNT(DISTINCT scope_id) FROM knowledge_nodes", ())
            .await
            .map_err(db_err)?;
        let scope_count = match scopes.next().await.map_err(db_err)? {
            Some(row) => int(&row, 0)?,
            None => 0,
        };
        if scope_count > 1 {
            return Err(Error::Storage(format!(
                "cannot collapse {scope_count} knowledge scopes into one global tree"
            )));
        }

        self.conn.execute("BEGIN", ()).await.map_err(db_err)?;
        let result = async {
            self.conn
                .execute("DROP INDEX IF EXISTS idx_knowledge_nodes_parent", ())
                .await
                .map_err(db_err)?;
            self.conn
                .execute(
                    "ALTER TABLE knowledge_nodes RENAME TO knowledge_nodes_scoped",
                    (),
                )
                .await
                .map_err(db_err)?;
            self.conn
                .execute_batch(
                    "CREATE TABLE knowledge_nodes (
                        id TEXT PRIMARY KEY,
                        parent_id TEXT,
                        knowledge_path TEXT NOT NULL,
                        title TEXT NOT NULL,
                        body_md TEXT NOT NULL,
                        status TEXT NOT NULL,
                        confidence REAL NOT NULL,
                        source_hash TEXT NOT NULL,
                        structural INTEGER NOT NULL DEFAULT 0,
                        created_at INTEGER NOT NULL,
                        updated_at INTEGER NOT NULL,
                        UNIQUE(knowledge_path)
                    );",
                )
                .await
                .map_err(db_err)?;
            self.conn
                .execute(
                    "INSERT INTO knowledge_nodes
                        (id, parent_id, knowledge_path, title, body_md, status, confidence,
                         source_hash, structural, created_at, updated_at)
                     SELECT id, parent_id, knowledge_path, title, body_md, status, confidence,
                            source_hash, structural, created_at, updated_at
                     FROM knowledge_nodes_scoped",
                    (),
                )
                .await
                .map_err(db_err)?;
            self.conn
                .execute("DROP TABLE knowledge_nodes_scoped", ())
                .await
                .map_err(db_err)?;
            Ok(())
        }
        .await;
        match result {
            Ok(()) => {
                self.conn.execute("COMMIT", ()).await.map_err(db_err)?;
                self.conn
                    .execute_batch(
                        "CREATE INDEX IF NOT EXISTS idx_knowledge_nodes_parent
                            ON knowledge_nodes(parent_id);",
                    )
                    .await
                    .map_err(db_err)?;
                Ok(())
            }
            Err(error) => {
                let _ = self.conn.execute("ROLLBACK", ()).await;
                Err(error)
            }
        }
    }

    pub(super) async fn ensure_knowledge_node(&self, node: &Knowledge) -> Result<Uuid> {
        let mut chain = vec![node.clone()];
        while let Some(parent_path) = chain
            .last()
            .and_then(|current| current.knowledge_path.parent())
        {
            let parent = Knowledge::structural(parent_path, node.updated_at);
            chain.push(parent);
        }
        chain.reverse();

        let mut parent_id = None;
        for current in chain {
            parent_id = Some(self.upsert_knowledge_node(&current, parent_id).await?);
        }
        parent_id.ok_or_else(|| Error::Storage("knowledge node chain was empty".into()))
    }

    async fn knowledge_node_id_by_path(&self, path: &KnowledgePath) -> Result<Option<Uuid>> {
        let mut existing = self
            .conn
            .query(
                "SELECT id FROM knowledge_nodes WHERE knowledge_path = ?1",
                params_from_iter([Value::Text(path.as_str())]),
            )
            .await
            .map_err(db_err)?;
        match existing.next().await.map_err(db_err)? {
            Some(row) => Ok(Some(parse_uuid(text(&row, 0)?, "knowledge_id")?)),
            None => Ok(None),
        }
    }

    async fn upsert_knowledge_node(
        &self,
        node: &Knowledge,
        parent_id: Option<Uuid>,
    ) -> Result<Uuid> {
        if let Some(id) = self.knowledge_node_id_by_path(&node.knowledge_path).await? {
            if node.structural {
                self.conn
                    .execute(
                        "UPDATE knowledge_nodes
                         SET title = ?1, status = 'active', updated_at = ?2
                         WHERE id = ?3",
                        params_from_iter([
                            Value::Text(node.title.clone()),
                            Value::Integer(node.updated_at),
                            uuid_value(id),
                        ]),
                    )
                    .await
                    .map_err(db_err)?;
            } else {
                self.conn
                    .execute(
                        "UPDATE knowledge_nodes
                         SET title = ?1, body_md = ?2, status = 'active', confidence = ?3,
                             source_hash = ?4, structural = 0, updated_at = ?5
                         WHERE id = ?6",
                        params_from_iter([
                            Value::Text(node.title.clone()),
                            Value::Text(node.body_md.clone()),
                            Value::Real(node.confidence),
                            Value::Text(node.source_hash.clone()),
                            Value::Integer(node.updated_at),
                            uuid_value(id),
                        ]),
                    )
                    .await
                    .map_err(db_err)?;
            }
            return Ok(id);
        }

        self.conn
            .execute(
                "INSERT INTO knowledge_nodes
                    (id, parent_id, knowledge_path, title, body_md, status,
                     confidence, source_hash, structural, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
                params_from_iter([
                    uuid_value(node.id),
                    parent_id.map(uuid_value).unwrap_or(Value::Null),
                    Value::Text(node.knowledge_path.as_str()),
                    Value::Text(node.title.clone()),
                    Value::Text(node.body_md.clone()),
                    Value::Text(node.status.clone()),
                    Value::Real(node.confidence),
                    Value::Text(node.source_hash.clone()),
                    Value::Integer(i64::from(node.structural)),
                    Value::Integer(node.updated_at),
                ]),
            )
            .await
            .map_err(db_err)?;
        Ok(node.id)
    }

    async fn migrate_legacy_entries_to_knowledge_nodes(&self) -> Result<()> {
        let mut marker = self
            .conn
            .query(
                "SELECT value FROM memory_storage_metadata WHERE key = ?1",
                params_from_iter([Value::Text(KNOWLEDGE_TREE_MIGRATION_MARKER.into())]),
            )
            .await
            .map_err(db_err)?;
        if marker.next().await.map_err(db_err)?.is_some() {
            return Ok(());
        }

        let entries = self
            .collect(
                "SELECT id, scope_id, entry_type, title, body_md, status, confidence,
                        created_at, updated_at
                 FROM memory_entries WHERE status = 'active'
                 ORDER BY updated_at DESC, id ASC",
                vec![],
                row_to_entry,
            )
            .await?;
        for entry in entries {
            let root = match entry.entry_type.as_str() {
                "preference" => "/agent/preferences",
                "failure_shield" => "/operations/failure-shields",
                "task_map" => "/architecture/task-maps",
                _ => "/engineering/workflows",
            };
            let segment = crate::artifacts::normalize_slug(Some(&entry.title), entry.id);
            let proposed = format!("{}/{}", root, segment);
            let path = KnowledgePath::normalize(&proposed)
                .or_else(|| KnowledgePath::normalize(&format!("{}/{}", root, entry.id)))
                .ok_or_else(|| Error::Storage("generate legacy knowledge path".into()))?;
            let node = Knowledge::new(
                entry.id,
                None,
                path,
                entry.title,
                entry.body_md,
                entry.status,
                entry.confidence,
                String::new(),
                false,
                entry.created_at,
                entry.updated_at,
            );
            self.ensure_knowledge_node(&node).await?;
        }

        self.conn
            .execute(
                "INSERT INTO memory_storage_metadata(key, value) VALUES (?1, 'migrated')
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params_from_iter([Value::Text(KNOWLEDGE_TREE_MIGRATION_MARKER.into())]),
            )
            .await
            .map_err(db_err)?;
        Ok(())
    }

    /// Copy legacy in-agent.db memory tables into this knowledge database.
    ///
    /// The migration is row-by-row and marker-based, so repeated startup is a
    /// no-op and concurrent writers in the old database are not required to
    /// support cross-database transactions.
    pub async fn migrate_from_agent_db(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path
            .as_ref()
            .to_str()
            .ok_or_else(|| Error::Storage("agent db path is not valid UTF-8".into()))?
            .to_string();

        let marker = self
            .conn
            .query(
                "SELECT value FROM memory_storage_metadata WHERE key = ?1",
                params_from_iter([Value::Text(MIGRATION_MARKER.into())]),
            )
            .await
            .map_err(db_err)?;
        let mut marker = marker;
        if marker.next().await.map_err(db_err)?.is_some() {
            self.migrate_legacy_entries_to_knowledge_nodes().await?;
            return Ok(());
        }
        if !Path::new(&path).exists() {
            self.set_migration_marker().await?;
            self.migrate_legacy_entries_to_knowledge_nodes().await?;
            return Ok(());
        }

        let source_db = Builder::new_local(&path)
            .experimental_multiprocess_wal(true)
            .build()
            .await
            .map_err(db_err)?;
        let source = source_db.connect().map_err(db_err)?;
        source
            .pragma_update("busy_timeout", 5000)
            .await
            .map_err(db_err)?;

        self.conn.execute("BEGIN", ()).await.map_err(db_err)?;
        let result = async {
            self.copy_table(
                &source,
                "memory_stage1",
                &[
                    "scope_id",
                    "session_id",
                    "source_hash",
                    "raw_memory",
                    "rollout_summary",
                    "rollout_slug",
                    "status",
                    "generated_at",
                    "lease_until",
                    "attempts",
                    "last_error",
                    "updated_at",
                ],
            )
            .await?;
            self.copy_table(
                &source,
                "memory_jobs",
                &[
                    "scope_id",
                    "status",
                    "source_hash",
                    "lease_until",
                    "attempts",
                    "last_error",
                    "updated_at",
                ],
            )
            .await?;
            self.copy_table(
                &source,
                "memory_entries",
                &[
                    "id",
                    "scope_id",
                    "entry_type",
                    "title",
                    "body_md",
                    "status",
                    "confidence",
                    "created_at",
                    "updated_at",
                ],
            )
            .await?;
            self.copy_table(
                &source,
                "memory_summaries",
                &[
                    "scope_id",
                    "schema_version",
                    "summary_md",
                    "source_hash",
                    "generated_at",
                ],
            )
            .await?;
            self.copy_table(
                &source,
                "memory_notes",
                &["id", "scope_id", "slug", "content", "status", "created_at"],
            )
            .await?;
            self.copy_table(
                &source,
                "memory_semantic_observations",
                &[
                    "id",
                    "scope_id",
                    "subject",
                    "predicate",
                    "object",
                    "content",
                    "status",
                    "confidence",
                    "source_hash",
                    "created_at",
                    "last_error",
                ],
            )
            .await?;
            self.conn
                .execute(
                    "INSERT INTO memory_storage_metadata(key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params_from_iter([
                        Value::Text(MIGRATION_MARKER.into()),
                        Value::Text(path.clone()),
                    ]),
                )
                .await
                .map_err(db_err)?;
            Ok(())
        }
        .await;

        match result {
            Ok(()) => {
                self.conn.execute("COMMIT", ()).await.map_err(db_err)?;
                self.migrate_legacy_entries_to_knowledge_nodes().await?;
                Ok(())
            }
            Err(error) => {
                let _ = self.conn.execute("ROLLBACK", ()).await;
                Err(error)
            }
        }
    }

    async fn set_migration_marker(&self) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO memory_storage_metadata(key, value) VALUES (?1, 'missing_source')
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params_from_iter([Value::Text(MIGRATION_MARKER.into())]),
            )
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn copy_table(
        &self,
        source: &turso::Connection,
        table: &str,
        columns: &[&str],
    ) -> Result<()> {
        let mut exists = source
            .query(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                params_from_iter([Value::Text(table.into())]),
            )
            .await
            .map_err(db_err)?;
        if exists.next().await.map_err(db_err)?.is_none() {
            return Ok(());
        }

        let column_list = columns.join(", ");
        let mut rows = source
            .query(format!("SELECT {column_list} FROM {table}"), ())
            .await
            .map_err(db_err)?;
        let placeholders = (1..=columns.len())
            .map(|idx| format!("?{idx}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "INSERT INTO {table} ({column_list}) VALUES ({placeholders}) ON CONFLICT DO NOTHING"
        );

        while let Some(row) = rows.next().await.map_err(db_err)? {
            let values = (0..columns.len())
                .map(|idx| row.get_value(idx).map_err(db_err))
                .collect::<Result<Vec<_>>>()?;
            self.conn
                .execute(&sql, params_from_iter(values))
                .await
                .map_err(db_err)?;
        }
        Ok(())
    }

    async fn collect<T, F>(&self, sql: &str, params: Vec<Value>, map: F) -> Result<Vec<T>>
    where
        F: Fn(&turso::Row) -> Result<T>,
    {
        let mut rows = self
            .conn
            .query(sql, params_from_iter(params))
            .await
            .map_err(db_err)?;
        let mut values = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_err)? {
            values.push(map(&row)?);
        }
        Ok(values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MEMORY_SCOPE_ID;
    use crate::store::MemoryStore;
    use crate::store::knowledge_store::KnowledgeStore;

    #[tokio::test]
    async fn opens_schema_and_stores_notes() {
        let store = TursoMemoryStorage::open_in_memory().await.unwrap();
        let note = store
            .insert_note(MEMORY_SCOPE_ID, "test", "hello")
            .await
            .unwrap();
        assert_eq!(note.content, "hello");
        assert_eq!(
            store
                .list_pending_notes(MEMORY_SCOPE_ID)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn phase2_upserts_nodes_by_knowledge_path() {
        let store = TursoMemoryStorage::open_in_memory().await.unwrap();
        let path = KnowledgePath::normalize("/engineering/testing/verification").unwrap();
        let first = Knowledge::leaf(
            path.clone(),
            "Verification workflow".into(),
            "Run targeted tests.".into(),
            0.8,
            "source-a",
            crate::now_ms(),
        );
        let first_id = first.id;
        store
            .complete_phase2(
                MEMORY_SCOPE_ID,
                "source-a",
                vec![first],
                "v1\n\nfirst",
                Vec::new(),
                Vec::new(),
            )
            .await
            .unwrap();

        let updated = Knowledge::leaf(
            path.clone(),
            "Verification workflow".into(),
            "Run targeted tests before check.".into(),
            0.9,
            "source-b",
            crate::now_ms(),
        );
        let branch = Knowledge::leaf(
            KnowledgePath::normalize("/engineering/testing/database-migrations").unwrap(),
            "Database migrations".into(),
            "Use idempotent migrations.".into(),
            0.9,
            "source-b",
            crate::now_ms(),
        );
        store
            .complete_phase2(
                MEMORY_SCOPE_ID,
                "source-b",
                vec![updated, branch],
                "v1\n\nsecond",
                Vec::new(),
                Vec::new(),
            )
            .await
            .unwrap();

        let nodes = store.list_knowledge(100).await.unwrap();
        let verification = nodes
            .iter()
            .find(|node| node.knowledge_path == path)
            .unwrap();
        assert_eq!(verification.id, first_id);
        assert!(verification.body_md.contains("before check"));
        assert_eq!(
            nodes.iter().filter(|node| node.structural).count(),
            2,
            "engineering and testing structure nodes are absent or wrong"
        );
    }

    #[tokio::test]
    async fn migrates_active_legacy_entries_to_knowledge_nodes() {
        let destination = TursoMemoryStorage::open_in_memory().await.unwrap();
        let entry_id = Uuid::new_v4();
        destination
            .conn
            .execute(
                "INSERT INTO memory_entries
                    (id, scope_id, entry_type, title, body_md, status, confidence,
                     created_at, updated_at)
                 VALUES (?1, ?2, 'failure_shield', 'Torn WAL Recovery', ?3, 'active', 0.9, 1, 2)",
                params_from_iter([
                    uuid_value(entry_id),
                    uuid_value(MEMORY_SCOPE_ID),
                    Value::Text("Use sqlite3 instead of Turso for recovery.".into()),
                ]),
            )
            .await
            .unwrap();
        destination
            .migrate_legacy_entries_to_knowledge_nodes()
            .await
            .unwrap();
        let nodes = destination.list_knowledge(100).await.unwrap();
        let entry = nodes.iter().find(|node| node.id == entry_id).unwrap();
        assert_eq!(
            entry.knowledge_path.as_str(),
            "/operations/failure-shields/torn-wal-recovery"
        );
        assert!(entry.body_md.contains("sqlite3"));
        assert!(nodes.iter().any(|node| node.structural));
    }

    #[tokio::test]
    async fn migrates_legacy_agent_memory_tables_once() {
        let source = TursoMemoryStorage::open_in_memory().await.unwrap();
        let destination = TursoMemoryStorage::open_in_memory().await.unwrap();
        source
            .insert_note(MEMORY_SCOPE_ID, "legacy", "old memory")
            .await
            .unwrap();
        destination
            .copy_table(
                &*source.conn.0.lock().await,
                "memory_notes",
                &["id", "scope_id", "slug", "content", "status", "created_at"],
            )
            .await
            .unwrap();
        assert_eq!(
            destination
                .list_pending_notes(MEMORY_SCOPE_ID)
                .await
                .unwrap()
                .len(),
            1
        );

        // A marker makes the second run a no-op even if source data changes.
        source
            .insert_note(MEMORY_SCOPE_ID, "second", "not migrated")
            .await
            .unwrap();
        destination
            .conn
            .execute(
                "INSERT INTO memory_storage_metadata(key, value)
                 VALUES ('legacy_agent_db_memory_migrated_v1', 'test')",
                (),
            )
            .await
            .unwrap();
        destination
            .migrate_from_agent_db("/definitely/missing/agentik/source.db")
            .await
            .unwrap();
        assert_eq!(
            destination
                .list_pending_notes(MEMORY_SCOPE_ID)
                .await
                .unwrap()
                .len(),
            1
        );
    }
}
