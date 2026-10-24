//! Turso-backed persistent memory repository.
mod knowledge;
mod memory_store;
mod queue;

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
pub struct TursoKmsStorage {
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
        entry_type: text(row, 1)?,
        title: text(row, 2)?,
        body_md: text(row, 3)?,
        status: text(row, 4)?,
        confidence: real(row, 5)?,
        created_at: int(row, 6)?,
        updated_at: int(row, 7)?,
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
        schema_version: text(row, 0)?,
        summary_md: text(row, 1)?,
        source_hash: text(row, 2)?,
        generated_at: int(row, 3)?,
    })
}

fn row_to_note(row: &turso::Row) -> Result<MemoryNote> {
    Ok(MemoryNote {
        id: parse_uuid(text(row, 0)?, "note_id")?,
        slug: text(row, 1)?,
        content: text(row, 2)?,
        status: text(row, 3)?,
        created_at: int(row, 4)?,
    })
}

fn row_to_observation(row: &turso::Row) -> Result<SemanticObservation> {
    Ok(SemanticObservation {
        id: parse_uuid(text(row, 0)?, "observation_id")?,
        subject: text(row, 1)?,
        predicate: text(row, 2)?,
        object: text(row, 3)?,
        content: text(row, 4)?,
        status: text(row, 5)?,
        confidence: real(row, 6)?,
        source_hash: text(row, 7)?,
        created_at: int(row, 8)?,
        last_error: opt_text(row, 9)?.filter(|value| !value.is_empty()),
    })
}

impl TursoKmsStorage {
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
                    PRIMARY KEY (session_id)
                );
                CREATE INDEX IF NOT EXISTS idx_memory_stage1_generated
                    ON memory_stage1(generated_at DESC);
                CREATE TABLE IF NOT EXISTS memory_jobs (
                    singleton INTEGER PRIMARY KEY DEFAULT 0,
                    status TEXT NOT NULL,
                    source_hash TEXT NOT NULL,
                    lease_until INTEGER NOT NULL,
                    attempts INTEGER NOT NULL DEFAULT 0,
                    last_error TEXT,
                    updated_at INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS knowledge_pipeline_jobs (
                    id TEXT PRIMARY KEY,
                    job_json TEXT NOT NULL,
                    dedup_key TEXT NOT NULL,
                    status TEXT NOT NULL,
                    run_after INTEGER NOT NULL,
                    lease_until INTEGER NOT NULL DEFAULT 0,
                    attempts INTEGER NOT NULL DEFAULT 0,
                    max_attempts INTEGER NOT NULL DEFAULT 5,
                    last_error TEXT,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    UNIQUE(dedup_key)
                );
                CREATE INDEX IF NOT EXISTS idx_knowledge_jobs_due
                    ON knowledge_pipeline_jobs(status, run_after, updated_at);
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
                    singleton INTEGER PRIMARY KEY DEFAULT 0,
                    schema_version TEXT NOT NULL,
                    summary_md TEXT NOT NULL,
                    source_hash TEXT NOT NULL,
                    generated_at INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS memory_notes (
                    id TEXT PRIMARY KEY,
                    slug TEXT NOT NULL,
                    content TEXT NOT NULL,
                    status TEXT NOT NULL,
                    created_at INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_memory_notes_status
                    ON memory_notes(status, created_at);
                CREATE TABLE IF NOT EXISTS memory_semantic_observations (
                    id TEXT PRIMARY KEY,
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
                CREATE INDEX IF NOT EXISTS idx_memory_semantic_status
                    ON memory_semantic_observations(status, created_at DESC);
                CREATE TABLE IF NOT EXISTS memory_storage_metadata (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );
                "#,
            )
            .await
            .map_err(db_err)?;
        self.migrate_knowledge_nodes_to_global_schema().await?;
        self.migrate_memory_tables_to_global_schema().await?;
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

    async fn table_exists(&self, table: &str) -> Result<bool> {
        let mut rows = self
            .conn
            .query(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                params_from_iter([Value::Text(table.to_string())]),
            )
            .await
            .map_err(db_err)?;
        Ok(rows.next().await.map_err(db_err)?.is_some())
    }

    async fn table_has_scope_column(&self, table: &str) -> Result<bool> {
        let mut rows = self
            .conn
            .query(
                &format!("SELECT 1 FROM pragma_table_info('{table}') WHERE name = 'scope_id'"),
                (),
            )
            .await
            .map_err(db_err)?;
        Ok(rows.next().await.map_err(db_err)?.is_some())
    }

    /// Collapse the transitional scoped memory tables into global tables.
    ///
    /// All production scopes were nil; rejecting multiple scopes here prevents
    /// silent data loss if a non-global database is encountered.
    async fn migrate_memory_tables_to_global_schema(&self) -> Result<()> {
        let migrations: &[(&str, &str, &str, &str)] = &[
            (
                "memory_stage1",
                "CREATE TABLE memory_stage1 (
                    session_id TEXT NOT NULL PRIMARY KEY,
                    source_hash TEXT NOT NULL,
                    raw_memory TEXT NOT NULL,
                    rollout_summary TEXT NOT NULL,
                    rollout_slug TEXT,
                    status TEXT NOT NULL,
                    generated_at INTEGER NOT NULL,
                    lease_until INTEGER NOT NULL DEFAULT 0,
                    attempts INTEGER NOT NULL DEFAULT 0,
                    last_error TEXT,
                    updated_at INTEGER NOT NULL
                )",
                "session_id, source_hash, raw_memory, rollout_summary, rollout_slug,
                 status, generated_at, lease_until, attempts, updated_at",
                "session_id, source_hash, raw_memory, rollout_summary, rollout_slug,
                 status, generated_at, lease_until, attempts, updated_at",
            ),
            (
                "memory_jobs",
                "CREATE TABLE memory_jobs (
                    singleton INTEGER PRIMARY KEY DEFAULT 0,
                    status TEXT NOT NULL,
                    source_hash TEXT NOT NULL,
                    lease_until INTEGER NOT NULL,
                    attempts INTEGER NOT NULL DEFAULT 0,
                    last_error TEXT,
                    updated_at INTEGER NOT NULL
                )",
                "status, source_hash, lease_until, attempts, updated_at",
                "status, source_hash, lease_until, attempts, updated_at",
            ),
            (
                "memory_summaries",
                "CREATE TABLE memory_summaries (
                    singleton INTEGER PRIMARY KEY DEFAULT 0,
                    schema_version TEXT NOT NULL,
                    summary_md TEXT NOT NULL,
                    source_hash TEXT NOT NULL,
                    generated_at INTEGER NOT NULL
                )",
                "schema_version, summary_md, source_hash, generated_at",
                "schema_version, summary_md, source_hash, generated_at",
            ),
            (
                "memory_notes",
                "CREATE TABLE memory_notes (
                    id TEXT PRIMARY KEY,
                    slug TEXT NOT NULL,
                    content TEXT NOT NULL,
                    status TEXT NOT NULL,
                    created_at INTEGER NOT NULL
                )",
                "id, slug, content, status, created_at",
                "id, slug, content, status, created_at",
            ),
            (
                "memory_semantic_observations",
                "CREATE TABLE memory_semantic_observations (
                    id TEXT PRIMARY KEY,
                    subject TEXT NOT NULL,
                    predicate TEXT NOT NULL,
                    object TEXT NOT NULL,
                    content TEXT NOT NULL,
                    status TEXT NOT NULL,
                    confidence REAL NOT NULL,
                    source_hash TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    last_error TEXT
                )",
                "id, subject, predicate, object, content, status, confidence,
                 source_hash, created_at, last_error",
                "id, subject, predicate, object, content, status, confidence,
                 source_hash, created_at, last_error",
            ),
        ];

        for (table, create_table, insert_columns, select_columns) in migrations {
            if !self.table_exists(table).await? || !self.table_has_scope_column(table).await? {
                continue;
            }
            let mut scopes = self
                .conn
                .query(&format!("SELECT COUNT(DISTINCT scope_id) FROM {table}"), ())
                .await
                .map_err(db_err)?;
            let scope_count = match scopes.next().await.map_err(db_err)? {
                Some(row) => int(&row, 0)?,
                None => 0,
            };
            if scope_count > 1 {
                return Err(Error::Storage(format!(
                    "cannot collapse {scope_count} scopes in {table} into global memory"
                )));
            }

            self.conn.execute("BEGIN", ()).await.map_err(db_err)?;
            let result = async {
                self.conn
                    .execute(&format!("DROP INDEX IF EXISTS idx_{table}_generated"), ())
                    .await
                    .map_err(db_err)?;
                self.conn
                    .execute(
                        &format!("DROP INDEX IF EXISTS idx_{table}_scope_status"),
                        (),
                    )
                    .await
                    .map_err(db_err)?;
                self.conn
                    .execute(&format!("ALTER TABLE {table} RENAME TO {table}_scoped"), ())
                    .await
                    .map_err(db_err)?;
                self.conn
                    .execute_batch(create_table)
                    .await
                    .map_err(db_err)?;
                self.conn
                    .execute(
                        &format!(
                            "INSERT INTO {table} ({insert_columns})
                             SELECT {select_columns} FROM {table}_scoped"
                        ),
                        (),
                    )
                    .await
                    .map_err(db_err)?;
                if *table == "memory_jobs" || *table == "memory_summaries" {
                    self.conn
                        .execute(&format!("UPDATE {table} SET singleton = 0"), ())
                        .await
                        .map_err(db_err)?;
                }
                self.conn
                    .execute(&format!("DROP TABLE {table}_scoped"), ())
                    .await
                    .map_err(db_err)?;
                Ok(())
            }
            .await;
            match result {
                Ok(()) => {
                    self.conn.execute("COMMIT", ()).await.map_err(db_err)?;
                }
                Err(error) => {
                    let _ = self.conn.execute("ROLLBACK", ()).await;
                    return Err(error);
                }
            }
        }

        self.conn
            .execute_batch(
                "CREATE INDEX IF NOT EXISTS idx_memory_stage1_generated
                    ON memory_stage1(generated_at DESC);
                CREATE INDEX IF NOT EXISTS idx_memory_notes_status
                    ON memory_notes(status, created_at);
                CREATE INDEX IF NOT EXISTS idx_memory_semantic_status
                    ON memory_semantic_observations(status, created_at DESC);",
            )
            .await
            .map_err(db_err)?;
        Ok(())
    }

    /// Convert the transitional scoped knowledge tree to the global schema.
    ///
    /// The tree is intentionally global: its namespace is `knowledge_path`.
    /// Convert any transitional scoped knowledge tree to the global tree.
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

        let entries = if self.table_exists("memory_entries_staging").await? {
            self.collect(
                "SELECT id, entry_type, title, body_md, status, confidence,
                        created_at, updated_at
                 FROM memory_entries_staging WHERE status = 'active'
                 ORDER BY updated_at DESC, id ASC",
                vec![],
                row_to_entry,
            )
            .await?
        } else if self.table_exists("memory_entries").await? {
            self.collect(
                "SELECT id, entry_type, title, body_md, status, confidence,
                        created_at, updated_at
                 FROM memory_entries WHERE status = 'active'
                 ORDER BY updated_at DESC, id ASC",
                vec![],
                row_to_entry,
            )
            .await?
        } else {
            Vec::new()
        };
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

    async fn drop_legacy_memory_entries(&self) -> Result<()> {
        self.conn
            .execute("DROP TABLE IF EXISTS memory_entries", ())
            .await
            .map_err(db_err)?;
        self.conn
            .execute("DROP TABLE IF EXISTS memory_entries_staging", ())
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
            self.drop_legacy_memory_entries().await?;
            return Ok(());
        }
        if !Path::new(&path).exists() {
            self.set_migration_marker().await?;
            self.migrate_legacy_entries_to_knowledge_nodes().await?;
            self.drop_legacy_memory_entries().await?;
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
            self.copy_rows(
                &source,
                "memory_stage1",
                "memory_stage1",
                &[
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
                &[
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
            self.copy_rows(
                &source,
                "memory_jobs",
                "memory_jobs",
                &[
                    "status",
                    "source_hash",
                    "lease_until",
                    "attempts",
                    "last_error",
                    "updated_at",
                ],
                &[
                    "status",
                    "source_hash",
                    "lease_until",
                    "attempts",
                    "last_error",
                    "updated_at",
                ],
            )
            .await?;
            self.conn
                .execute("UPDATE memory_jobs SET singleton = 0", ())
                .await
                .map_err(db_err)?;
            self.conn
                .execute(
                    "CREATE TABLE memory_entries_staging (
                        id TEXT PRIMARY KEY,
                        entry_type TEXT NOT NULL,
                        title TEXT NOT NULL,
                        body_md TEXT NOT NULL,
                        status TEXT NOT NULL,
                        confidence REAL NOT NULL,
                        created_at INTEGER NOT NULL,
                        updated_at INTEGER NOT NULL
                    )",
                    (),
                )
                .await
                .map_err(db_err)?;
            self.copy_rows(
                &source,
                "memory_entries",
                "memory_entries_staging",
                &[
                    "id",
                    "entry_type",
                    "title",
                    "body_md",
                    "status",
                    "confidence",
                    "created_at",
                    "updated_at",
                ],
                &[
                    "id",
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
            self.copy_rows(
                &source,
                "memory_summaries",
                "memory_summaries",
                &[
                    "schema_version",
                    "summary_md",
                    "source_hash",
                    "generated_at",
                ],
                &[
                    "schema_version",
                    "summary_md",
                    "source_hash",
                    "generated_at",
                ],
            )
            .await?;
            self.conn
                .execute("UPDATE memory_summaries SET singleton = 0", ())
                .await
                .map_err(db_err)?;
            self.copy_rows(
                &source,
                "memory_notes",
                "memory_notes",
                &["id", "slug", "content", "status", "created_at"],
                &["id", "slug", "content", "status", "created_at"],
            )
            .await?;
            self.copy_rows(
                &source,
                "memory_semantic_observations",
                "memory_semantic_observations",
                &[
                    "id",
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
                &[
                    "id",
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
                self.drop_legacy_memory_entries().await?;
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

    async fn copy_rows(
        &self,
        source: &turso::Connection,
        source_table: &str,
        destination_table: &str,
        source_columns: &[&str],
        destination_columns: &[&str],
    ) -> Result<()> {
        let mut exists = source
            .query(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                params_from_iter([Value::Text(source_table.into())]),
            )
            .await
            .map_err(db_err)?;
        if exists.next().await.map_err(db_err)?.is_none() {
            return Ok(());
        }

        let destination_column_list = destination_columns.join(", ");
        let mut rows = source
            .query(
                format!("SELECT {} FROM {source_table}", source_columns.join(", ")),
                (),
            )
            .await
            .map_err(db_err)?;
        let placeholders = (1..=source_columns.len())
            .map(|idx| format!("?{idx}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "INSERT INTO {destination_table} ({destination_column_list}) VALUES ({placeholders}) ON CONFLICT DO NOTHING"
        );

        while let Some(row) = rows.next().await.map_err(db_err)? {
            let values = (0..source_columns.len())
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
    use crate::pipeline::{KnowledgeJob, StageOutcome, StageStatus};
    use crate::store::KnowledgeQueue;
    use crate::store::MemoryStore;
    use crate::store::knowledge_store::KnowledgeStore;

    #[tokio::test]
    async fn opens_schema_and_stores_notes() {
        let store = TursoKmsStorage::open_in_memory().await.unwrap();
        let note = store.insert_note("test", "hello").await.unwrap();
        assert_eq!(note.content, "hello");
        assert_eq!(store.list_pending_notes().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn knowledge_queue_deduplicates_retries_and_leaves_continuations_to_caller() {
        let store = TursoKmsStorage::open_in_memory().await.unwrap();
        let agent_id = Uuid::new_v4();
        let job = KnowledgeJob::ExtractAgentSessions { agent_id };
        let first_id = store.enqueue(job.clone()).await.unwrap();
        let second_id = store.enqueue(job).await.unwrap();
        assert_eq!(first_id, second_id);

        let now = crate::now_ms();
        let claimed = store.claim_due(now, 10).await.unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].id, first_id);
        assert_eq!(claimed[0].attempts, 1);
        assert!(store.claim_due(now, 10).await.unwrap().is_empty());

        store
            .complete(
                first_id,
                StageOutcome {
                    status: StageStatus::RetryLater {
                        after_secs: 60,
                        reason: "model unavailable".to_string(),
                    },
                    next_jobs: Vec::new(),
                },
            )
            .await
            .unwrap();
        assert!(store.claim_due(now + 59_000, 10).await.unwrap().is_empty());
        let claimed = store.claim_due(now + 61_000, 10).await.unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].attempts, 2);

        store
            .complete(
                first_id,
                StageOutcome::completed(vec![KnowledgeJob::ConsolidateKnowledge {
                    trigger: "test".to_string(),
                }]),
            )
            .await
            .unwrap();
        assert!(
            store
                .claim_due(crate::now_ms(), 10)
                .await
                .unwrap()
                .is_empty()
        );

        let continuation_id = store
            .enqueue(KnowledgeJob::ConsolidateKnowledge {
                trigger: "test".to_string(),
            })
            .await
            .unwrap();
        assert_ne!(continuation_id, first_id);
        let claimed = store.claim_due(crate::now_ms(), 10).await.unwrap();
        assert_eq!(claimed.len(), 1);
        assert!(matches!(
            claimed[0].job,
            KnowledgeJob::ConsolidateKnowledge { .. }
        ));
    }

    #[tokio::test]
    async fn releasing_a_foreign_claim_does_not_consume_an_attempt() {
        let store = TursoKmsStorage::open_in_memory().await.unwrap();
        let phase2_id = store
            .enqueue(KnowledgeJob::ConsolidateKnowledge {
                trigger: "test".to_string(),
            })
            .await
            .unwrap();

        let claimed = store.claim_due(crate::now_ms(), 10).await.unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].attempts, 1);

        store.release(phase2_id).await.unwrap();
        let reclaimed = store.claim_due(crate::now_ms(), 10).await.unwrap();
        assert_eq!(reclaimed.len(), 1);
        assert_eq!(reclaimed[0].id, phase2_id);
        assert_eq!(reclaimed[0].attempts, 1);
    }

    #[tokio::test]
    async fn phase2_upserts_nodes_by_knowledge_path() {
        let store = TursoKmsStorage::open_in_memory().await.unwrap();
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
    async fn migrates_scoped_memory_tables_to_global_schema() {
        let store = TursoKmsStorage::open_in_memory().await.unwrap();
        store
            .conn
            .execute_batch(
                "DROP TABLE memory_jobs;
                CREATE TABLE memory_jobs (
                    scope_id TEXT PRIMARY KEY,
                    status TEXT NOT NULL,
                    source_hash TEXT NOT NULL,
                    lease_until INTEGER NOT NULL,
                    attempts INTEGER NOT NULL DEFAULT 0,
                    last_error TEXT,
                    updated_at INTEGER NOT NULL
                );
                INSERT INTO memory_jobs
                    (scope_id, status, source_hash, lease_until, attempts, updated_at)
                 VALUES (
                    '00000000-0000-0000-0000-000000000000',
                    'succeeded',
                    'source',
                    0,
                    1,
                    1
                 );
                DROP TABLE memory_notes;
                CREATE TABLE memory_notes (
                    id TEXT PRIMARY KEY,
                    scope_id TEXT NOT NULL,
                    slug TEXT NOT NULL,
                    content TEXT NOT NULL,
                    status TEXT NOT NULL,
                    created_at INTEGER NOT NULL
                );
                INSERT INTO memory_notes
                    (id, scope_id, slug, content, status, created_at)
                 VALUES (
                    '00000000-0000-0000-0000-000000000001',
                    '00000000-0000-0000-0000-000000000000',
                    'test',
                    'hello',
                    'pending',
                    1
                 );",
            )
            .await
            .unwrap();

        store
            .migrate_memory_tables_to_global_schema()
            .await
            .unwrap();

        let mut jobs = store
            .conn
            .query("SELECT singleton, status FROM memory_jobs", ())
            .await
            .unwrap();
        let job = jobs.next().await.unwrap().unwrap();
        assert_eq!(int(&job, 0).unwrap(), 0);
        assert_eq!(text(&job, 1).unwrap(), "succeeded");
        assert_eq!(store.list_pending_notes().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn migrates_active_legacy_entries_to_knowledge_nodes() {
        let destination = TursoKmsStorage::open_in_memory().await.unwrap();
        let entry_id = Uuid::new_v4();
        destination
            .conn
            .execute_batch(format!(
                "CREATE TABLE memory_entries (
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
            INSERT INTO memory_entries
                (id, scope_id, entry_type, title, body_md, status, confidence,
                 created_at, updated_at)
             VALUES (
                '{entry_id}',
                '00000000-0000-0000-0000-000000000000',
                'failure_shield',
                'Torn WAL Recovery',
                'Use sqlite3 instead of Turso for recovery.',
                'active',
                0.9,
                1,
                2
             );"
            ))
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
        let source = TursoKmsStorage::open_in_memory().await.unwrap();
        let destination = TursoKmsStorage::open_in_memory().await.unwrap();
        source.insert_note("legacy", "old memory").await.unwrap();
        destination
            .copy_rows(
                &*source.conn.0.lock().await,
                "memory_notes",
                "memory_notes",
                &["id", "slug", "content", "status", "created_at"],
                &["id", "slug", "content", "status", "created_at"],
            )
            .await
            .unwrap();
        assert_eq!(destination.list_pending_notes().await.unwrap().len(), 1);

        // A marker makes the second run a no-op even if source data changes.
        source.insert_note("second", "not migrated").await.unwrap();
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
        assert_eq!(destination.list_pending_notes().await.unwrap().len(), 1);
    }
}
