//! Turso-backed persistent memory repository.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;
use turso::{Builder, IntoParams, Value, params_from_iter};
use uuid::Uuid;

use crate::error::{MemoryError, MemoryResult};
use crate::store::{
    MemoryEntry, MemoryNote, MemoryStage1Record, MemoryStore, MemorySummary, SemanticObservation,
};

const MIGRATION_MARKER: &str = "legacy_agent_db_memory_migrated_v1";

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

fn db_err(error: impl std::fmt::Display) -> MemoryError {
    MemoryError::Storage(error.to_string())
}

fn uuid_value(id: Uuid) -> Value {
    Value::Text(id.to_string())
}

fn text(row: &turso::Row, idx: usize) -> MemoryResult<String> {
    match row.get_value(idx).map_err(db_err)? {
        Value::Text(value) => Ok(value),
        Value::Null => Ok(String::new()),
        other => Err(MemoryError::Storage(format!(
            "expected TEXT at column {idx}, got {other:?}"
        ))),
    }
}

fn opt_text(row: &turso::Row, idx: usize) -> MemoryResult<Option<String>> {
    match row.get_value(idx).map_err(db_err)? {
        Value::Text(value) => Ok(Some(value)),
        _ => Ok(None),
    }
}

fn int(row: &turso::Row, idx: usize) -> MemoryResult<i64> {
    match row.get_value(idx).map_err(db_err)? {
        Value::Integer(value) => Ok(value),
        other => Err(MemoryError::Storage(format!(
            "expected INTEGER at column {idx}, got {other:?}"
        ))),
    }
}

fn real(row: &turso::Row, idx: usize) -> MemoryResult<f64> {
    match row.get_value(idx).map_err(db_err)? {
        Value::Real(value) => Ok(value),
        Value::Integer(value) => Ok(value as f64),
        other => Err(MemoryError::Storage(format!(
            "expected REAL at column {idx}, got {other:?}"
        ))),
    }
}

fn parse_uuid(value: String, field: &str) -> MemoryResult<Uuid> {
    Uuid::parse_str(&value).map_err(|e| MemoryError::Storage(format!("parse memory {field}: {e}")))
}

fn row_to_stage1(row: &turso::Row) -> MemoryResult<MemoryStage1Record> {
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

fn row_to_entry(row: &turso::Row) -> MemoryResult<MemoryEntry> {
    Ok(MemoryEntry {
        id: parse_uuid(text(row, 0)?, "entry_id")?,
        scope_id: parse_uuid(text(row, 1)?, "scope_id")?,
        entry_type: text(row, 2)?,
        title: text(row, 3)?,
        body_md: text(row, 4)?,
        status: text(row, 5)?,
        confidence: real(row, 6)?,
        created_at: int(row, 7)?,
        updated_at: int(row, 8)?,
    })
}

fn row_to_summary(row: &turso::Row) -> MemoryResult<MemorySummary> {
    Ok(MemorySummary {
        scope_id: parse_uuid(text(row, 0)?, "scope_id")?,
        schema_version: text(row, 1)?,
        summary_md: text(row, 2)?,
        source_hash: text(row, 3)?,
        generated_at: int(row, 4)?,
    })
}

fn row_to_note(row: &turso::Row) -> MemoryResult<MemoryNote> {
    Ok(MemoryNote {
        id: parse_uuid(text(row, 0)?, "note_id")?,
        scope_id: parse_uuid(text(row, 1)?, "scope_id")?,
        slug: text(row, 2)?,
        content: text(row, 3)?,
        status: text(row, 4)?,
        created_at: int(row, 5)?,
    })
}

fn row_to_observation(row: &turso::Row) -> MemoryResult<SemanticObservation> {
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
    pub async fn open(path: impl AsRef<Path>) -> MemoryResult<Self> {
        let path_ref = path.as_ref();
        let path = path_ref
            .to_str()
            .ok_or_else(|| MemoryError::Storage("memory db path is not valid UTF-8".into()))?;
        if let Some(parent) = path_ref.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| MemoryError::Storage(format!("create memory db parent: {e}")))?;
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

    pub async fn open_in_memory() -> MemoryResult<Self> {
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

    async fn init_schema(&self) -> MemoryResult<()> {
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
            .map_err(db_err)
    }

    /// Copy legacy in-agent.db memory tables into this knowledge database.
    ///
    /// The migration is row-by-row and marker-based, so repeated startup is a
    /// no-op and concurrent writers in the old database are not required to
    /// support cross-database transactions.
    pub async fn migrate_from_agent_db(&self, path: impl AsRef<Path>) -> MemoryResult<()> {
        let path = path
            .as_ref()
            .to_str()
            .ok_or_else(|| MemoryError::Storage("agent db path is not valid UTF-8".into()))?
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
        if matches!(marker.next().await.map_err(db_err)?, Some(_)) {
            return Ok(());
        }
        if !Path::new(&path).exists() {
            self.set_migration_marker().await?;
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
                Ok(())
            }
            Err(error) => {
                let _ = self.conn.execute("ROLLBACK", ()).await;
                Err(error)
            }
        }
    }

    async fn set_migration_marker(&self) -> MemoryResult<()> {
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
    ) -> MemoryResult<()> {
        let mut exists = source
            .query(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                params_from_iter([Value::Text(table.into())]),
            )
            .await
            .map_err(db_err)?;
        if matches!(exists.next().await.map_err(db_err)?, None) {
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
                .collect::<MemoryResult<Vec<_>>>()?;
            self.conn
                .execute(&sql, params_from_iter(values))
                .await
                .map_err(db_err)?;
        }
        Ok(())
    }

    async fn collect<T, F>(&self, sql: &str, params: Vec<Value>, map: F) -> MemoryResult<Vec<T>>
    where
        F: Fn(&turso::Row) -> MemoryResult<T>,
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

#[async_trait]
impl MemoryStore for TursoMemoryStorage {
    async fn get_stage1_output(
        &self,
        scope_id: Uuid,
        session_id: Uuid,
    ) -> MemoryResult<Option<MemoryStage1Record>> {
        let mut rows = self
            .conn
            .query(
                "SELECT session_id, source_hash, raw_memory, rollout_summary, rollout_slug,
                        status, generated_at, lease_until
                 FROM memory_stage1 WHERE scope_id = ?1 AND session_id = ?2",
                params_from_iter([uuid_value(scope_id), uuid_value(session_id)]),
            )
            .await
            .map_err(db_err)?;
        match rows.next().await.map_err(db_err)? {
            Some(row) => Ok(Some(row_to_stage1(&row)?)),
            None => Ok(None),
        }
    }

    async fn claim_stage1(
        &self,
        scope_id: Uuid,
        session_id: Uuid,
        source_hash: &str,
        lease_until: i64,
    ) -> MemoryResult<bool> {
        let updated_at = chrono::Utc::now().timestamp_millis();
        let changed = self
            .conn
            .execute(
                "INSERT INTO memory_stage1
                    (scope_id, session_id, source_hash, raw_memory, rollout_summary,
                     rollout_slug, status, generated_at, lease_until, attempts, updated_at)
                 VALUES (?1, ?2, ?3, '', '', NULL, 'running', 0, ?4, 1, ?5)
                 ON CONFLICT(scope_id, session_id) DO UPDATE SET
                    source_hash = excluded.source_hash,
                    status = 'running',
                    lease_until = excluded.lease_until,
                    attempts = memory_stage1.attempts + 1,
                    updated_at = excluded.updated_at
                 WHERE memory_stage1.source_hash <> excluded.source_hash
                    OR memory_stage1.status = 'failed'
                    OR (memory_stage1.status = 'running'
                        AND memory_stage1.lease_until <= excluded.updated_at)",
                params_from_iter([
                    uuid_value(scope_id),
                    uuid_value(session_id),
                    Value::Text(source_hash.into()),
                    Value::Integer(lease_until),
                    Value::Integer(updated_at),
                ]),
            )
            .await
            .map_err(db_err)?;
        Ok(changed > 0)
    }

    async fn complete_stage1(
        &self,
        scope_id: Uuid,
        output: MemoryStage1Record,
    ) -> MemoryResult<()> {
        let updated_at = chrono::Utc::now().timestamp_millis();
        self.conn
            .execute(
                "INSERT INTO memory_stage1
                    (scope_id, session_id, source_hash, raw_memory, rollout_summary,
                     rollout_slug, status, generated_at, lease_until, attempts, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'succeeded', ?7, 0, 1, ?8)
                 ON CONFLICT(scope_id, session_id) DO UPDATE SET
                    source_hash = excluded.source_hash,
                    raw_memory = excluded.raw_memory,
                    rollout_summary = excluded.rollout_summary,
                    rollout_slug = excluded.rollout_slug,
                    status = 'succeeded',
                    generated_at = excluded.generated_at,
                    lease_until = 0,
                    attempts = memory_stage1.attempts + 1,
                    last_error = NULL,
                    updated_at = excluded.updated_at",
                params_from_iter([
                    uuid_value(scope_id),
                    uuid_value(output.session_id),
                    Value::Text(output.source_hash),
                    Value::Text(output.raw_memory),
                    Value::Text(output.rollout_summary),
                    output.rollout_slug.map(Value::Text).unwrap_or(Value::Null),
                    Value::Integer(output.generated_at),
                    Value::Integer(updated_at),
                ]),
            )
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn fail_stage1(
        &self,
        scope_id: Uuid,
        session_id: Uuid,
        source_hash: &str,
        error: &str,
    ) -> MemoryResult<()> {
        let updated_at = chrono::Utc::now().timestamp_millis();
        self.conn
            .execute(
                "INSERT INTO memory_stage1
                    (scope_id, session_id, source_hash, raw_memory, rollout_summary,
                     rollout_slug, status, generated_at, lease_until, attempts,
                     last_error, updated_at)
                 VALUES (?1, ?2, ?3, '', '', NULL, 'failed', 0, 0, 1, ?4, ?5)
                 ON CONFLICT(scope_id, session_id) DO UPDATE SET
                    source_hash = excluded.source_hash,
                    status = 'failed',
                    lease_until = 0,
                    attempts = memory_stage1.attempts + 1,
                    last_error = excluded.last_error,
                    updated_at = excluded.updated_at",
                params_from_iter([
                    uuid_value(scope_id),
                    uuid_value(session_id),
                    Value::Text(source_hash.into()),
                    Value::Text(error.into()),
                    Value::Integer(updated_at),
                ]),
            )
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn list_stage1_outputs(
        &self,
        scope_id: Uuid,
        limit: usize,
    ) -> MemoryResult<Vec<MemoryStage1Record>> {
        self.collect(
            "SELECT session_id, source_hash, raw_memory, rollout_summary, rollout_slug,
                    status, generated_at, lease_until
             FROM memory_stage1
             WHERE scope_id = ?1 AND status = 'succeeded'
             ORDER BY generated_at DESC, session_id DESC LIMIT ?2",
            vec![uuid_value(scope_id), Value::Integer(limit as i64)],
            row_to_stage1,
        )
        .await
    }

    async fn claim_phase2(
        &self,
        scope_id: Uuid,
        source_hash: &str,
        lease_until: i64,
    ) -> MemoryResult<bool> {
        let updated_at = chrono::Utc::now().timestamp_millis();
        let changed = self
            .conn
            .execute(
                "INSERT INTO memory_jobs
                    (scope_id, status, source_hash, lease_until, attempts, updated_at)
                 VALUES (?1, 'running', ?2, ?3, 1, ?4)
                 ON CONFLICT(scope_id) DO UPDATE SET
                    status = 'running',
                    source_hash = excluded.source_hash,
                    lease_until = excluded.lease_until,
                    attempts = memory_jobs.attempts + 1,
                    last_error = NULL,
                    updated_at = excluded.updated_at
                 WHERE NOT (memory_jobs.status = 'succeeded'
                        AND memory_jobs.source_hash = excluded.source_hash)
                    AND (memory_jobs.status <> 'running'
                        OR memory_jobs.lease_until <= excluded.updated_at)",
                params_from_iter([
                    uuid_value(scope_id),
                    Value::Text(source_hash.into()),
                    Value::Integer(lease_until),
                    Value::Integer(updated_at),
                ]),
            )
            .await
            .map_err(db_err)?;
        Ok(changed > 0)
    }

    async fn complete_phase2(
        &self,
        scope_id: Uuid,
        source_hash: &str,
        entries: Vec<MemoryEntry>,
        summary_md: &str,
        observations: Vec<SemanticObservation>,
        consumed_note_ids: Vec<Uuid>,
    ) -> MemoryResult<()> {
        let now = crate::now_ms();
        self.conn.execute("BEGIN", ()).await.map_err(db_err)?;
        let result = async {
            self.conn
                .execute(
                    "UPDATE memory_entries SET status = 'superseded', updated_at = ?1
                     WHERE scope_id = ?2 AND status = 'active'",
                    params_from_iter([Value::Integer(now), uuid_value(scope_id)]),
                )
                .await
                .map_err(db_err)?;

            for entry in entries {
                self.conn
                    .execute(
                        "INSERT INTO memory_entries
                            (id, scope_id, entry_type, title, body_md, status,
                             confidence, created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, 'active', ?6, ?7, ?7)",
                        params_from_iter([
                            uuid_value(entry.id),
                            uuid_value(scope_id),
                            Value::Text(entry.entry_type),
                            Value::Text(entry.title),
                            Value::Text(entry.body_md),
                            Value::Real(entry.confidence),
                            Value::Integer(now),
                        ]),
                    )
                    .await
                    .map_err(db_err)?;
            }

            self.conn
                .execute(
                    "INSERT INTO memory_summaries
                        (scope_id, schema_version, summary_md, source_hash, generated_at)
                     VALUES (?1, 'v1', ?2, ?3, ?4)
                     ON CONFLICT(scope_id) DO UPDATE SET
                        schema_version = 'v1',
                        summary_md = excluded.summary_md,
                        source_hash = excluded.source_hash,
                        generated_at = excluded.generated_at",
                    params_from_iter([
                        uuid_value(scope_id),
                        Value::Text(summary_md.into()),
                        Value::Text(source_hash.into()),
                        Value::Integer(now),
                    ]),
                )
                .await
                .map_err(db_err)?;

            for observation in observations {
                let mut existing = self
                    .conn
                    .query(
                        "SELECT id FROM memory_semantic_observations
                         WHERE scope_id = ?1 AND subject = ?2 AND predicate = ?3 AND object = ?4",
                        params_from_iter([
                            uuid_value(scope_id),
                            Value::Text(observation.subject.clone()),
                            Value::Text(observation.predicate.clone()),
                            Value::Text(observation.object.clone()),
                        ]),
                    )
                    .await
                    .map_err(db_err)?;
                let existing_id = match existing.next().await.map_err(db_err)? {
                    Some(row) => parse_uuid(text(&row, 0)?, "observation_id")?,
                    None => Uuid::nil(),
                };
                if existing_id != Uuid::nil() {
                    self.conn
                        .execute(
                            "UPDATE memory_semantic_observations SET
                                content = ?1, status = 'candidate', confidence = ?2,
                                source_hash = ?3, created_at = ?4, last_error = NULL
                             WHERE id = ?5 AND scope_id = ?6",
                            params_from_iter([
                                Value::Text(observation.content),
                                Value::Real(observation.confidence),
                                Value::Text(observation.source_hash),
                                Value::Integer(observation.created_at),
                                uuid_value(existing_id),
                                uuid_value(scope_id),
                            ]),
                        )
                        .await
                        .map_err(db_err)?;
                    continue;
                }
                self.conn
                    .execute(
                        "INSERT INTO memory_semantic_observations
                            (id, scope_id, subject, predicate, object, content, status,
                             confidence, source_hash, created_at, last_error)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'candidate', ?7, ?8, ?9, NULL)",
                        params_from_iter([
                            uuid_value(observation.id),
                            uuid_value(scope_id),
                            Value::Text(observation.subject),
                            Value::Text(observation.predicate),
                            Value::Text(observation.object),
                            Value::Text(observation.content),
                            Value::Real(observation.confidence),
                            Value::Text(observation.source_hash),
                            Value::Integer(now),
                        ]),
                    )
                    .await
                    .map_err(db_err)?;
            }

            for note_id in consumed_note_ids {
                self.conn
                    .execute(
                        "UPDATE memory_notes SET status = 'consumed' WHERE id = ?1 AND scope_id = ?2",
                        params_from_iter([uuid_value(note_id), uuid_value(scope_id)]),
                    )
                    .await
                    .map_err(db_err)?;
            }

            self.conn
                .execute(
                    "INSERT INTO memory_jobs
                        (scope_id, status, source_hash, lease_until, attempts, updated_at)
                     VALUES (?1, 'succeeded', ?2, 0, 1, ?3)
                     ON CONFLICT(scope_id) DO UPDATE SET
                        status = 'succeeded', source_hash = excluded.source_hash,
                        lease_until = 0, last_error = NULL, updated_at = excluded.updated_at",
                    params_from_iter([
                        uuid_value(scope_id),
                        Value::Text(source_hash.into()),
                        Value::Integer(now),
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
                Ok(())
            }
            Err(error) => {
                let _ = self.conn.execute("ROLLBACK", ()).await;
                Err(error)
            }
        }
    }

    async fn fail_phase2(
        &self,
        scope_id: Uuid,
        source_hash: &str,
        error: &str,
    ) -> MemoryResult<()> {
        let updated_at = chrono::Utc::now().timestamp_millis();
        self.conn
            .execute(
                "INSERT INTO memory_jobs
                    (scope_id, status, source_hash, lease_until, attempts, last_error, updated_at)
                 VALUES (?1, 'failed', ?2, 0, 1, ?3, ?4)
                 ON CONFLICT(scope_id) DO UPDATE SET
                    status = 'failed', source_hash = excluded.source_hash,
                    lease_until = 0, attempts = memory_jobs.attempts + 1,
                    last_error = excluded.last_error, updated_at = excluded.updated_at",
                params_from_iter([
                    uuid_value(scope_id),
                    Value::Text(source_hash.into()),
                    Value::Text(error.into()),
                    Value::Integer(updated_at),
                ]),
            )
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn get_summary(&self, scope_id: Uuid) -> MemoryResult<Option<MemorySummary>> {
        let mut rows = self
            .conn
            .query(
                "SELECT scope_id, schema_version, summary_md, source_hash, generated_at
                 FROM memory_summaries WHERE scope_id = ?1",
                params_from_iter([uuid_value(scope_id)]),
            )
            .await
            .map_err(db_err)?;
        match rows.next().await.map_err(db_err)? {
            Some(row) => Ok(Some(row_to_summary(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_entries(&self, scope_id: Uuid, limit: usize) -> MemoryResult<Vec<MemoryEntry>> {
        self.collect(
            "SELECT id, scope_id, entry_type, title, body_md, status, confidence,
                    created_at, updated_at
             FROM memory_entries WHERE scope_id = ?1 AND status = 'active'
             ORDER BY updated_at DESC, title ASC LIMIT ?2",
            vec![uuid_value(scope_id), Value::Integer(limit as i64)],
            row_to_entry,
        )
        .await
    }

    async fn get_entry(&self, scope_id: Uuid, entry_id: Uuid) -> MemoryResult<Option<MemoryEntry>> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, scope_id, entry_type, title, body_md, status, confidence,
                        created_at, updated_at
                 FROM memory_entries WHERE scope_id = ?1 AND id = ?2",
                params_from_iter([uuid_value(scope_id), uuid_value(entry_id)]),
            )
            .await
            .map_err(db_err)?;
        match rows.next().await.map_err(db_err)? {
            Some(row) => Ok(Some(row_to_entry(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_pending_notes(&self, scope_id: Uuid) -> MemoryResult<Vec<MemoryNote>> {
        self.collect(
            "SELECT id, scope_id, slug, content, status, created_at
             FROM memory_notes WHERE scope_id = ?1 AND status = 'pending'
             ORDER BY created_at ASC, id ASC",
            vec![uuid_value(scope_id)],
            row_to_note,
        )
        .await
    }

    async fn insert_note(
        &self,
        scope_id: Uuid,
        slug: &str,
        content: &str,
    ) -> MemoryResult<MemoryNote> {
        let note = MemoryNote {
            id: Uuid::new_v4(),
            scope_id,
            slug: slug.into(),
            content: content.into(),
            status: "pending".into(),
            created_at: crate::now_ms(),
        };
        self.conn
            .execute(
                "INSERT INTO memory_notes
                    (id, scope_id, slug, content, status, created_at)
                 VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
                params_from_iter([
                    uuid_value(note.id),
                    uuid_value(note.scope_id),
                    Value::Text(note.slug.clone()),
                    Value::Text(note.content.clone()),
                    Value::Integer(note.created_at),
                ]),
            )
            .await
            .map_err(db_err)?;
        Ok(note)
    }

    async fn list_observations(
        &self,
        scope_id: Uuid,
        status: &str,
        limit: usize,
    ) -> MemoryResult<Vec<SemanticObservation>> {
        self.collect(
            "SELECT id, scope_id, subject, predicate, object, content, status,
                    confidence, source_hash, created_at, last_error
             FROM memory_semantic_observations
             WHERE scope_id = ?1 AND status = ?2
             ORDER BY created_at DESC, id DESC LIMIT ?3",
            vec![
                uuid_value(scope_id),
                Value::Text(status.into()),
                Value::Integer(limit as i64),
            ],
            row_to_observation,
        )
        .await
    }

    async fn set_observation_status(
        &self,
        scope_id: Uuid,
        observation_id: Uuid,
        status: &str,
        error: Option<&str>,
    ) -> MemoryResult<()> {
        let changed = self
            .conn
            .execute(
                "UPDATE memory_semantic_observations
                 SET status = ?1, last_error = ?2 WHERE scope_id = ?3 AND id = ?4",
                params_from_iter([
                    Value::Text(status.into()),
                    error
                        .map(|value| Value::Text(value.into()))
                        .unwrap_or(Value::Null),
                    uuid_value(scope_id),
                    uuid_value(observation_id),
                ]),
            )
            .await
            .map_err(db_err)?;
        if changed == 0 {
            return Err(MemoryError::Storage(format!(
                "semantic observation {observation_id} not found"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MEMORY_SCOPE_ID;

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
    async fn migrates_legacy_agent_memory_tables_once() {
        let dir = std::env::temp_dir().join(format!("agentik-memory-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let source_path = dir.join("agent.db");
        let destination_path = dir.join("knowledge.db");

        let source = TursoMemoryStorage::open(&source_path).await.unwrap();
        source
            .insert_note(MEMORY_SCOPE_ID, "legacy", "old memory")
            .await
            .unwrap();
        let destination = TursoMemoryStorage::open(&destination_path).await.unwrap();
        destination
            .migrate_from_agent_db(&source_path)
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
            .migrate_from_agent_db(&source_path)
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

        let _ = std::fs::remove_file(&source_path);
        let _ = std::fs::remove_file(&destination_path);
        let _ = std::fs::remove_dir(&dir);
    }
}
