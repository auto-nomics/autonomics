//! Turso-backed agent storage — the sole storage backend for agent persistence.
//!
//! Uses [`turso::Connection`] (async, SQLite-compatible) to store:
//! - **Snapshots** — full `Memory` checkpoints at session boundaries.
//! - **Agent registry** — `AgentRecord` + `AgentRelation` tables.
//! - **Session log (WAL)** — incremental message append for fast recovery.
//!
//! # Schema
//!
//! ```sql
//! CREATE TABLE snapshots (
//!     snapshot_id TEXT PRIMARY KEY,
//!     agent_id    TEXT NOT NULL,
//!     ts          INTEGER NOT NULL,
//!     status      TEXT NOT NULL,
//!     memory      TEXT NOT NULL
//! );
//! CREATE INDEX idx_snapshots_agent_ts ON snapshots(agent_id, ts DESC);
//!
//! CREATE TABLE agents (
//!     id          TEXT PRIMARY KEY,
//!     name        TEXT NOT NULL,
//!     config_json TEXT NOT NULL,
//!     created_at  INTEGER NOT NULL,
//!     last_active INTEGER NOT NULL
//! );
//!
//! CREATE TABLE agent_relations (
//!     parent_id TEXT NOT NULL,
//!     child_id  TEXT NOT NULL,
//!     kind      TEXT NOT NULL,
//!     PRIMARY KEY (parent_id, child_id)
//! );
//! CREATE INDEX idx_relations_child ON agent_relations(child_id);
//!
//! CREATE TABLE sessions (
//!     id         TEXT PRIMARY KEY,
//!     agent_id   TEXT NOT NULL,
//!     started_at INTEGER NOT NULL,
//!     ended_at   INTEGER
//! );
//! CREATE INDEX idx_sessions_agent ON sessions(agent_id);
//!
//! CREATE TABLE messages (
//!     id           INTEGER PRIMARY KEY AUTOINCREMENT,
//!     session_id   TEXT NOT NULL,
//!     seq          INTEGER NOT NULL,
//!     message_json TEXT NOT NULL,
//!     ts           INTEGER NOT NULL
//! );
//! CREATE INDEX idx_messages_session ON messages(session_id, seq);
//! CREATE INDEX idx_messages_agent_ts ON messages(session_id, ts);
//! ```

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use turso::{Value, params_from_iter};
use uuid::Uuid;

use agentik_sdk::types::messages::Message;
use agentik_types::AgentPlan;

use crate::storage::{
    AgentProfile, AgentProfileRegistry, AgentRecord, AgentRelation, AgentSnapshot, AgentStorage,
    PersistedAgentGraph, RelationKind, StorageError,
};

/// Turso-backed implementation of [`AgentStorage`].
#[derive(Clone)]
pub struct TursoAgentStorage {
    conn: turso::Connection,
}

impl TursoAgentStorage {
    /// Open (or create) an on-disk agent database at `path`.
    ///
    /// If the on-disk files are in a torn-WAL state (e.g. the process was
    /// SIGKILL'd mid-transaction and the WAL index points past EOF), the
    /// matching `-wal` / `-shm` / `-twal` / `-tshm` sidecars are
    /// quarantined and the open is retried once. The main `.db` file is
    /// never touched — checkpointed pages in it remain durable.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let path_ref = path.as_ref();
        let path_str = path_ref
            .to_str()
            .ok_or_else(|| StorageError::Other("agent db path is not valid UTF-8".into()))?;

        if let Some(parent) = path_ref.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| StorageError::Other(format!("create db parent dir: {e}").into()))?;
        }

        match Self::try_open_local(path_str).await {
            Ok(s) => Ok(s),
            Err(e) if is_torn_wal_error(&e) => {
                tracing::warn!(
                    db = %path_str,
                    error = %e,
                    "WAL torn on open — quarantining -wal/-shm sidecars and retrying once"
                );
                quarantine_wal_sidecars(path_ref);
                Self::try_open_local(path_str).await
            }
            Err(e) => Err(e),
        }
    }

    /// Internal: actually open the database without recovery. Split out so
    /// the recovery wrapper can call it twice.
    async fn try_open_local(path_str: &str) -> Result<Self, StorageError> {
        let db = turso::Builder::new_local(path_str)
            .experimental_multiprocess_wal(true)
            .build()
            .await
            .map_err(|e| StorageError::Other(format!("open agent database: {e}").into()))?;

        let conn = db
            .connect()
            .map_err(|e| StorageError::Other(format!("connect agent database: {e}").into()))?;

        conn.pragma_update("busy_timeout", 5000)
            .await
            .map_err(|e| StorageError::Other(format!("set busy_timeout: {e}").into()))?;

        let storage = Self { conn };
        storage.init_schema().await?;
        tracing::info!(db = path_str, "turso agent storage opened");
        Ok(storage)
    }

    /// Create an in-memory database (useful for tests).
    pub async fn open_in_memory() -> Result<Self, StorageError> {
        let db = turso::Builder::new_local(":memory:")
            .build()
            .await
            .map_err(|e| StorageError::Other(format!("create in-memory db: {e}").into()))?;

        let conn = db
            .connect()
            .map_err(|e| StorageError::Other(format!("connect in-memory db: {e}").into()))?;

        let storage = Self { conn };
        storage.init_schema().await?;
        Ok(storage)
    }

    async fn init_schema(&self) -> Result<(), StorageError> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS snapshots (
                    snapshot_id TEXT PRIMARY KEY,
                    agent_id    TEXT NOT NULL,
                    ts          INTEGER NOT NULL,
                    status      TEXT NOT NULL,
                    memory      TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_snapshots_agent_ts
                    ON snapshots(agent_id, ts DESC);

                CREATE TABLE IF NOT EXISTS agents (
                    id          TEXT PRIMARY KEY,
                    name        TEXT NOT NULL,
                    config_json TEXT NOT NULL,
                    created_at  INTEGER NOT NULL,
                    last_active INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS agent_relations (
                    parent_id TEXT NOT NULL,
                    child_id  TEXT NOT NULL,
                    kind      TEXT NOT NULL,
                    PRIMARY KEY (parent_id, child_id)
                );
                CREATE INDEX IF NOT EXISTS idx_relations_child
                    ON agent_relations(child_id);

                CREATE TABLE IF NOT EXISTS sessions (
                    id         TEXT PRIMARY KEY,
                    agent_id   TEXT NOT NULL,
                    started_at INTEGER NOT NULL,
                    ended_at   INTEGER,
                    title      TEXT
                );
                CREATE INDEX IF NOT EXISTS idx_sessions_agent
                    ON sessions(agent_id);

                CREATE TABLE IF NOT EXISTS messages (
                    id           INTEGER PRIMARY KEY AUTOINCREMENT,
                    session_id   TEXT NOT NULL,
                    seq          INTEGER NOT NULL,
                    message_json TEXT NOT NULL,
                    ts           INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_messages_session
                    ON messages(session_id, seq);
                CREATE INDEX IF NOT EXISTS idx_messages_agent_ts
                    ON messages(session_id, ts);

                CREATE TABLE IF NOT EXISTS agent_profiles (
                    id              TEXT PRIMARY KEY,
                    name            TEXT NOT NULL UNIQUE,
                    description     TEXT NOT NULL DEFAULT '',
                    config_json     TEXT NOT NULL,
                    created_at      INTEGER NOT NULL,
                    updated_at      INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS agent_plans (
                    agent_id   TEXT PRIMARY KEY,
                    plan_json  TEXT NOT NULL,
                    revision   INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS agent_graph (
                    path         TEXT PRIMARY KEY,
                    parent_path  TEXT,
                    profile_path TEXT NOT NULL,
                    agent_id     TEXT NOT NULL,
                    status_json  TEXT NOT NULL,
                    last_event   TEXT,
                    created_at   INTEGER NOT NULL,
                    updated_at   INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_agent_graph_parent
                    ON agent_graph(parent_path);
                CREATE INDEX IF NOT EXISTS idx_agent_graph_updated
                    ON agent_graph(updated_at DESC);
                ",
            )
            .await
            .map_err(|e| StorageError::Other(format!("schema init failed: {e}").into()))?;

        // ── Migrations for existing databases ──
        // Add `title` column to sessions if missing (idempotent).
        let _ = self
            .conn
            .execute("ALTER TABLE sessions ADD COLUMN title TEXT", ())
            .await;
        // Add `session_id` column to snapshots if missing (idempotent).
        let _ = self
            .conn
            .execute("ALTER TABLE snapshots ADD COLUMN session_id TEXT", ())
            .await;
        // Create per-session snapshot index (safe now that column exists).
        let _ = self
            .conn
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_snapshots_session_ts ON snapshots(session_id, ts DESC)",
                (),
            )
            .await;

        // ── Migration: agents.name short-name → full AgentPath ──
        //
        // Before the AgentPath refactor, `agents.name` stored only the
        // short name (e.g. `"researcher"`). The tree-based resume picker
        // requires the full hierarchical path (e.g. `/root/researcher`).
        // This one-shot UPDATE prefixes `/root/` to every row whose name
        // does not already start with `/root`. Idempotent — rows that are
        // already in the new format are left untouched.
        let _ = self
            .conn
            .execute(
                "UPDATE agents SET name = '/root/' || name
                 WHERE name NOT LIKE '/root%'",
                (),
            )
            .await;

        Ok(())
    }
}

// ── Row helpers ─────────────────────────────────────────────────

fn text_col(row: &turso::Row, idx: usize) -> Result<String, turso::Error> {
    match row.get_value(idx)? {
        Value::Text(s) => Ok(s),
        Value::Null => Ok(String::new()),
        other => Err(turso::Error::ToSqlConversionFailure(
            format!("expected TEXT at column {idx}, got {other:?}").into(),
        )),
    }
}

fn int_col(row: &turso::Row, idx: usize) -> Result<i64, turso::Error> {
    match row.get_value(idx)? {
        Value::Integer(v) => Ok(v),
        other => Err(turso::Error::ToSqlConversionFailure(
            format!("expected INTEGER at column {idx}, got {other:?}").into(),
        )),
    }
}

fn row_to_snapshot(row: &turso::Row) -> Result<AgentSnapshot, StorageError> {
    let snapshot_id_str = text_col(row, 0)?;
    let agent_id_str = text_col(row, 1)?;
    let ts = int_col(row, 2)?;
    let status_json = text_col(row, 3)?;
    let memory_json = text_col(row, 4)?;
    // Column 5 is session_id (may be NULL for old snapshots).
    let session_id = match row.get_value(5) {
        Ok(Value::Text(s)) if !s.is_empty() => Uuid::parse_str(&s).ok(),
        _ => None,
    };

    Ok(AgentSnapshot {
        snapshot_id: Uuid::parse_str(&snapshot_id_str)
            .map_err(|e| StorageError::Other(format!("parse snapshot_id: {e}").into()))?,
        agent_id: Uuid::parse_str(&agent_id_str)
            .map_err(|e| StorageError::Other(format!("parse agent_id: {e}").into()))?,
        ts,
        agent_status: serde_json::from_str(&status_json)?,
        memory: serde_json::from_str(&memory_json)?,
        session_id,
    })
}

fn row_to_record(row: &turso::Row) -> Result<AgentRecord, StorageError> {
    let id_str = text_col(row, 0)?;
    let name = text_col(row, 1)?;
    let config_str = text_col(row, 2)?;
    let created_at = int_col(row, 3)?;
    let last_active = int_col(row, 4)?;

    Ok(AgentRecord {
        id: Uuid::parse_str(&id_str)
            .map_err(|e| StorageError::Other(format!("parse agent id: {e}").into()))?,
        name,
        config_json: serde_json::from_str(&config_str)?,
        created_at,
        last_active,
    })
}

fn parse_relation(row: &turso::Row) -> Result<AgentRelation, StorageError> {
    let parent_id =
        Uuid::parse_str(&text_col(row, 0)?).map_err(|e| StorageError::Other(e.into()))?;
    let child_id =
        Uuid::parse_str(&text_col(row, 1)?).map_err(|e| StorageError::Other(e.into()))?;
    let kind_str = text_col(row, 2)?;
    Ok(AgentRelation {
        parent_id,
        child_id,
        kind: RelationKind::from_str(&kind_str).ok_or_else(|| {
            StorageError::Other(format!("unknown relation kind: {kind_str}").into())
        })?,
    })
}

// ── Unified AgentStorage impl ───────────────────────────────────

#[async_trait]
impl AgentStorage for TursoAgentStorage {
    // ── Snapshot ─────────────────────────────────────────────

    async fn create_snapshot(&self, snapshot: AgentSnapshot) -> Result<(), StorageError> {
        let memory_json = serde_json::to_string(&snapshot.memory)?;
        let status_json = serde_json::to_string(&snapshot.agent_status)?;
        let session_id_val = snapshot
            .session_id
            .map(|id| Value::Text(id.to_string()))
            .unwrap_or(Value::Null);
        self.conn
            .execute(
                "INSERT INTO snapshots
                    (snapshot_id, agent_id, ts, status, memory, session_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params_from_iter([
                    Value::Text(snapshot.snapshot_id.to_string()),
                    Value::Text(snapshot.agent_id.to_string()),
                    Value::Integer(snapshot.ts),
                    Value::Text(status_json),
                    Value::Text(memory_json),
                    session_id_val,
                ]),
            )
            .await?;
        Ok(())
    }

    async fn get_snapshot(&self, snapshot_id: Uuid) -> Result<AgentSnapshot, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT snapshot_id, agent_id, ts, status, memory, session_id
                 FROM snapshots WHERE snapshot_id = ?1",
                params_from_iter([Value::Text(snapshot_id.to_string())]),
            )
            .await?;

        match rows.next().await {
            Ok(Some(row)) => Ok(row_to_snapshot(&row)?),
            Ok(None) => Err(StorageError::NotFound(format!("snapshot {snapshot_id}"))),
            Err(e) => Err(e.into()),
        }
    }

    async fn get_agent_snapshots(
        &self,
        agent_id: Uuid,
    ) -> Result<Vec<AgentSnapshot>, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT snapshot_id, agent_id, ts, status, memory, session_id
                 FROM snapshots WHERE agent_id = ?1 ORDER BY ts DESC",
                params_from_iter([Value::Text(agent_id.to_string())]),
            )
            .await?;

        collect_rows(&mut rows, row_to_snapshot).await
    }

    async fn get_latest_snapshot(
        &self,
        agent_id: Uuid,
    ) -> Result<Option<AgentSnapshot>, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT snapshot_id, agent_id, ts, status, memory, session_id
                 FROM snapshots WHERE agent_id = ?1 ORDER BY ts DESC LIMIT 1",
                params_from_iter([Value::Text(agent_id.to_string())]),
            )
            .await?;

        match rows.next().await {
            Ok(Some(row)) => Ok(Some(row_to_snapshot(&row)?)),
            Ok(None) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    async fn list_all_agent_ids(&self) -> Result<Vec<Uuid>, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT DISTINCT agent_id FROM snapshots ORDER BY agent_id",
                params_from_iter([] as [Value; 0]),
            )
            .await?;

        let mut ids = Vec::new();
        loop {
            match rows.next().await {
                Ok(Some(row)) => {
                    ids.push(
                        Uuid::parse_str(&text_col(&row, 0)?)
                            .map_err(|e| StorageError::Other(e.into()))?,
                    );
                }
                Ok(None) => break,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(ids)
    }

    async fn delete_agent_snapshots(&self, agent_id: Uuid) -> Result<usize, StorageError> {
        self.conn
            .execute(
                "DELETE FROM snapshots WHERE agent_id = ?1",
                params_from_iter([Value::Text(agent_id.to_string())]),
            )
            .await?;
        Ok(0)
    }

    // ── Registry ─────────────────────────────────────────────

    async fn upsert_agent(&self, record: AgentRecord) -> Result<(), StorageError> {
        let config_json = serde_json::to_string(&record.config_json)?;
        self.conn
            .execute(
                "INSERT INTO agents (id, name, config_json, created_at, last_active)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(id) DO UPDATE SET
                     name = ?2,
                     config_json = ?3,
                     last_active = ?5",
                params_from_iter([
                    Value::Text(record.id.to_string()),
                    Value::Text(record.name),
                    Value::Text(config_json),
                    Value::Integer(record.created_at),
                    Value::Integer(record.last_active),
                ]),
            )
            .await?;
        Ok(())
    }

    async fn get_agent(&self, agent_id: Uuid) -> Result<Option<AgentRecord>, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, name, config_json, created_at, last_active
                 FROM agents WHERE id = ?1",
                params_from_iter([Value::Text(agent_id.to_string())]),
            )
            .await?;

        match rows.next().await {
            Ok(Some(row)) => Ok(Some(row_to_record(&row)?)),
            Ok(None) => Ok(None),
            Err(e) => Err(StorageError::Other(e.into())),
        }
    }

    async fn get_agent_by_name(&self, name: &str) -> Result<Option<AgentRecord>, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, name, config_json, created_at, last_active
                 FROM agents WHERE name = ?1 ORDER BY created_at DESC LIMIT 1",
                params_from_iter([Value::Text(name.to_string())]),
            )
            .await?;

        match rows.next().await {
            Ok(Some(row)) => Ok(Some(row_to_record(&row)?)),
            Ok(None) => Ok(None),
            Err(e) => Err(StorageError::Other(e.into())),
        }
    }

    async fn list_agents(&self) -> Result<Vec<AgentRecord>, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, name, config_json, created_at, last_active
                 FROM agents ORDER BY created_at ASC",
                params_from_iter([] as [Value; 0]),
            )
            .await?;

        collect_rows(&mut rows, row_to_record).await
    }

    async fn delete_agent(&self, agent_id: Uuid) -> Result<(), StorageError> {
        let id_str = agent_id.to_string();
        self.conn
            .execute(
                "DELETE FROM snapshots WHERE agent_id = ?1",
                params_from_iter([Value::Text(id_str.clone())]),
            )
            .await?;
        self.conn
            .execute(
                "DELETE FROM agent_relations WHERE parent_id = ?1 OR child_id = ?1",
                params_from_iter([Value::Text(id_str.clone())]),
            )
            .await?;
        // Cascade to sessions + messages
        self.conn
            .execute(
                "DELETE FROM messages WHERE session_id IN
                    (SELECT id FROM sessions WHERE agent_id = ?1)",
                params_from_iter([Value::Text(id_str.clone())]),
            )
            .await?;
        self.conn
            .execute(
                "DELETE FROM sessions WHERE agent_id = ?1",
                params_from_iter([Value::Text(id_str.clone())]),
            )
            .await?;
        self.conn
            .execute(
                "DELETE FROM agents WHERE id = ?1",
                params_from_iter([Value::Text(id_str)]),
            )
            .await?;
        Ok(())
    }

    async fn add_relation(&self, relation: AgentRelation) -> Result<(), StorageError> {
        self.conn
            .execute(
                "INSERT INTO agent_relations (parent_id, child_id, kind)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(parent_id, child_id) DO UPDATE SET kind = ?3",
                params_from_iter([
                    Value::Text(relation.parent_id.to_string()),
                    Value::Text(relation.child_id.to_string()),
                    Value::Text(relation.kind.as_str().to_string()),
                ]),
            )
            .await?;
        Ok(())
    }

    async fn list_children(&self, agent_id: Uuid) -> Result<Vec<AgentRelation>, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT parent_id, child_id, kind FROM agent_relations
                 WHERE parent_id = ?1",
                params_from_iter([Value::Text(agent_id.to_string())]),
            )
            .await?;
        collect_rows(&mut rows, parse_relation).await
    }

    async fn list_parents(&self, agent_id: Uuid) -> Result<Vec<AgentRelation>, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT parent_id, child_id, kind FROM agent_relations
                 WHERE child_id = ?1",
                params_from_iter([Value::Text(agent_id.to_string())]),
            )
            .await?;
        collect_rows(&mut rows, parse_relation).await
    }

    async fn touch_agent(&self, agent_id: Uuid) -> Result<(), StorageError> {
        let now = chrono::Utc::now().timestamp_millis();
        self.conn
            .execute(
                "UPDATE agents SET last_active = ?1 WHERE id = ?2",
                params_from_iter([Value::Integer(now), Value::Text(agent_id.to_string())]),
            )
            .await?;
        Ok(())
    }

    // ── Session log (WAL) ────────────────────────────────────

    async fn start_session(&self, agent_id: Uuid, session_id: Uuid) -> Result<(), StorageError> {
        let now = chrono::Utc::now().timestamp_millis();
        self.conn
            .execute(
                "INSERT INTO sessions (id, agent_id, started_at, ended_at)
                 VALUES (?1, ?2, ?3, NULL)
                 ON CONFLICT(id) DO UPDATE SET ended_at = NULL",
                params_from_iter([
                    Value::Text(session_id.to_string()),
                    Value::Text(agent_id.to_string()),
                    Value::Integer(now),
                ]),
            )
            .await?;
        Ok(())
    }

    async fn append_message(
        &self,
        session_id: Uuid,
        message: &Message,
    ) -> Result<(), StorageError> {
        let message_json = serde_json::to_string(message)?;
        let now = chrono::Utc::now().timestamp_millis();

        // seq = current max seq for this session + 1 (atomic via subquery)
        self.conn
            .execute(
                "INSERT INTO messages (session_id, seq, message_json, ts)
                 VALUES (?1, COALESCE(
                     (SELECT MAX(seq) FROM messages WHERE session_id = ?1), 0
                 ) + 1, ?2, ?3)",
                params_from_iter([
                    Value::Text(session_id.to_string()),
                    Value::Text(message_json),
                    Value::Integer(now),
                ]),
            )
            .await?;
        Ok(())
    }

    async fn end_session(&self, session_id: Uuid) -> Result<(), StorageError> {
        let now = chrono::Utc::now().timestamp_millis();
        self.conn
            .execute(
                "UPDATE sessions SET ended_at = ?1 WHERE id = ?2",
                params_from_iter([Value::Integer(now), Value::Text(session_id.to_string())]),
            )
            .await?;
        Ok(())
    }

    async fn delete_session(&self, session_id: Uuid) -> Result<(), StorageError> {
        let sid = session_id.to_string();
        // Delete messages belonging to this session.
        self.conn
            .execute(
                "DELETE FROM messages WHERE session_id = ?1",
                params_from_iter([Value::Text(sid.clone())]),
            )
            .await?;
        // Delete snapshots belonging to this session.
        self.conn
            .execute(
                "DELETE FROM snapshots WHERE session_id = ?1",
                params_from_iter([Value::Text(sid.clone())]),
            )
            .await?;
        // Delete the session row itself.
        self.conn
            .execute(
                "DELETE FROM sessions WHERE id = ?1",
                params_from_iter([Value::Text(sid)]),
            )
            .await?;
        Ok(())
    }

    async fn get_messages_since(
        &self,
        agent_id: Uuid,
        ts: i64,
    ) -> Result<Vec<Message>, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT m.message_json
                 FROM messages m
                 JOIN sessions s ON m.session_id = s.id
                 WHERE s.agent_id = ?1 AND m.ts > ?2
                 ORDER BY m.ts ASC, m.seq ASC",
                params_from_iter([Value::Text(agent_id.to_string()), Value::Integer(ts)]),
            )
            .await?;

        let mut messages = Vec::new();
        loop {
            match rows.next().await {
                Ok(Some(row)) => {
                    let json_str = text_col(&row, 0)?;
                    messages.push(serde_json::from_str(&json_str)?);
                }
                Ok(None) => break,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(messages)
    }

    async fn update_session_title(
        &self,
        session_id: Uuid,
        title: &str,
    ) -> Result<(), StorageError> {
        self.conn
            .execute(
                "UPDATE sessions SET title = ?1 WHERE id = ?2",
                params_from_iter([
                    Value::Text(title.to_string()),
                    Value::Text(session_id.to_string()),
                ]),
            )
            .await?;
        Ok(())
    }

    async fn list_session_records(
        &self,
        agent_id: Uuid,
    ) -> Result<Vec<crate::storage::SessionRecord>, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, title, started_at FROM sessions
                 WHERE agent_id = ?1
                 ORDER BY started_at ASC",
                params_from_iter([Value::Text(agent_id.to_string())]),
            )
            .await?;

        let mut records = Vec::new();
        loop {
            match rows.next().await {
                Ok(Some(row)) => {
                    let id_str = text_col(&row, 0)?;
                    let title = match row.get_value(1)? {
                        Value::Text(s) => Some(s),
                        _ => None,
                    };
                    let started_at = match row.get_value(2)? {
                        Value::Integer(n) => n,
                        _ => 0,
                    };
                    let session_id = Uuid::parse_str(&id_str).map_err(|e| {
                        StorageError::Other(format!("invalid session UUID '{id_str}': {e}").into())
                    })?;
                    records.push(crate::storage::SessionRecord {
                        session_id,
                        title,
                        started_at,
                    });
                }
                Ok(None) => break,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(records)
    }

    async fn get_latest_snapshot_for_session(
        &self,
        _agent_id: Uuid,
        session_id: Uuid,
    ) -> Result<Option<AgentSnapshot>, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT snapshot_id, agent_id, ts, status, memory, session_id
                 FROM snapshots
                 WHERE session_id = ?1
                 ORDER BY ts DESC LIMIT 1",
                params_from_iter([Value::Text(session_id.to_string())]),
            )
            .await?;
        match rows.next().await {
            Ok(Some(row)) => Ok(Some(row_to_snapshot(&row)?)),
            _ => Ok(None),
        }
    }

    async fn get_messages_since_for_session(
        &self,
        session_id: Uuid,
        ts: i64,
    ) -> Result<Vec<Message>, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT message_json
                 FROM messages
                 WHERE session_id = ?1 AND ts > ?2
                 ORDER BY ts ASC, seq ASC",
                params_from_iter([Value::Text(session_id.to_string()), Value::Integer(ts)]),
            )
            .await?;
        let mut messages = Vec::new();
        loop {
            match rows.next().await {
                Ok(Some(row)) => {
                    let json_str = text_col(&row, 0)?;
                    messages.push(serde_json::from_str(&json_str)?);
                }
                Ok(None) => break,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(messages)
    }

    // ── Persisted agent graph (Phase 4) ─────────────────────
    //
    // Keyed by hierarchical agent path so the dashboard can reconstruct
    // the multi-agent topology across process restarts. Mirrors codex's
    // `AgentGraphStore` (`codex-rs/agent-graph-store/src/store.rs:17-60`)
    // but persists only the topology + latest status — conversation
    // history is stored separately in `snapshots` + `messages`.

    async fn upsert_agent_graph_entry(
        &self,
        entry: PersistedAgentGraph,
    ) -> Result<(), StorageError> {
        // ON CONFLICT(path) DO UPDATE refreshes every mutable field
        // EXCEPT created_at (we preserve the original timestamp on
        // re-registration, matching codex's semantics).
        self.conn
            .execute(
                "INSERT INTO agent_graph
                    (path, parent_path, profile_path, agent_id,
                     status_json, last_event, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(path) DO UPDATE SET
                     parent_path  = excluded.parent_path,
                     profile_path = excluded.profile_path,
                     agent_id     = excluded.agent_id,
                     status_json  = excluded.status_json,
                     last_event   = excluded.last_event,
                     updated_at   = excluded.updated_at",
                params_from_iter([
                    Value::Text(entry.path),
                    entry.parent_path.map(Value::Text).unwrap_or(Value::Null),
                    Value::Text(entry.profile_path),
                    Value::Text(entry.agent_id.to_string()),
                    Value::Text(entry.status_json),
                    entry.last_event.map(Value::Text).unwrap_or(Value::Null),
                    Value::Integer(entry.created_at),
                    Value::Integer(entry.updated_at),
                ]),
            )
            .await?;
        Ok(())
    }

    async fn update_agent_graph_status(
        &self,
        path: &str,
        status_json: &str,
        last_event: Option<&str>,
    ) -> Result<(), StorageError> {
        let now = chrono::Utc::now().timestamp_millis();
        // Fire-and-log semantics: if the row was deleted between the
        // status observation and the write (e.g. concurrent shutdown),
        // affected_rows == 0 and we silently no-op — the host's removal
        // call already removed the entry.
        self.conn
            .execute(
                "UPDATE agent_graph
                 SET status_json = ?1, last_event = ?2, updated_at = ?3
                 WHERE path = ?4",
                params_from_iter([
                    Value::Text(status_json.to_string()),
                    last_event
                        .map(|s| Value::Text(s.to_string()))
                        .unwrap_or(Value::Null),
                    Value::Integer(now),
                    Value::Text(path.to_string()),
                ]),
            )
            .await?;
        Ok(())
    }

    async fn remove_agent_graph_entry(&self, path: &str) -> Result<(), StorageError> {
        // Idempotent: deleting a non-existent path is not an error.
        self.conn
            .execute(
                "DELETE FROM agent_graph WHERE path = ?1",
                params_from_iter([Value::Text(path.to_string())]),
            )
            .await?;
        Ok(())
    }

    async fn list_persisted_agents(&self) -> Result<Vec<PersistedAgentGraph>, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT path, parent_path, profile_path, agent_id,
                        status_json, last_event, created_at, updated_at
                 FROM agent_graph
                 ORDER BY updated_at DESC",
                (),
            )
            .await?;

        let mut out = Vec::new();
        loop {
            match rows.next().await {
                Ok(Some(row)) => {
                    let path = text_col(&row, 0)?;
                    let parent_path = match row.get_value(1)? {
                        Value::Text(s) => Some(s),
                        Value::Null => None,
                        other => {
                            return Err(turso::Error::ToSqlConversionFailure(
                                format!("expected TEXT or NULL at column 1, got {other:?}").into(),
                            )
                            .into());
                        }
                    };
                    let profile_path = text_col(&row, 2)?;
                    let agent_id_str = text_col(&row, 3)?;
                    let agent_id = Uuid::parse_str(&agent_id_str).map_err(|e| {
                        StorageError::Other(
                            format!("invalid agent_id uuid in agent_graph: {e}").into(),
                        )
                    })?;
                    let status_json = text_col(&row, 4)?;
                    let last_event = match row.get_value(5)? {
                        Value::Text(s) => Some(s),
                        Value::Null => None,
                        other => {
                            return Err(turso::Error::ToSqlConversionFailure(
                                format!("expected TEXT or NULL at column 5, got {other:?}").into(),
                            )
                            .into());
                        }
                    };
                    let created_at = int_col(&row, 6)?;
                    let updated_at = int_col(&row, 7)?;

                    out.push(PersistedAgentGraph {
                        path,
                        parent_path,
                        profile_path,
                        agent_id,
                        status_json,
                        last_event,
                        created_at,
                        updated_at,
                    });
                }
                Ok(None) => break,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(out)
    }

    async fn save_plan(&self, agent_id: Uuid, plan: &AgentPlan) -> Result<(), StorageError> {
        let json = serde_json::to_string(plan)?;
        let now = chrono::Utc::now().timestamp_millis();
        self.conn
            .execute(
                "INSERT INTO agent_plans (agent_id, plan_json, revision, updated_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(agent_id) DO UPDATE SET
                     plan_json = excluded.plan_json,
                     revision   = excluded.revision,
                     updated_at = excluded.updated_at",
                params_from_iter([
                    Value::Text(agent_id.to_string()),
                    Value::Text(json),
                    Value::Integer(plan.revision as i64),
                    Value::Integer(now),
                ]),
            )
            .await?;
        Ok(())
    }

    async fn load_plan(&self, agent_id: Uuid) -> Result<Option<AgentPlan>, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT plan_json FROM agent_plans WHERE agent_id = ?1",
                params_from_iter([Value::Text(agent_id.to_string())]),
            )
            .await?;
        match rows.next().await {
            Ok(Some(row)) => {
                let json_str = text_col(&row, 0)?;
                let plan = serde_json::from_str(&json_str)?;
                Ok(Some(plan))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

async fn collect_rows<T, F>(rows: &mut turso::Rows, mut f: F) -> Result<Vec<T>, StorageError>
where
    F: FnMut(&turso::Row) -> Result<T, StorageError>,
{
    let mut items = Vec::new();
    loop {
        match rows.next().await {
            Ok(Some(row)) => items.push(f(&row)?),
            Ok(None) => break,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(items)
}

// ── AgentProfileRegistry impl ───────────────────────────────────

fn row_to_profile(row: &turso::Row) -> Result<AgentProfile, StorageError> {
    let id_str = text_col(row, 0)?;
    let path = text_col(row, 1)?;
    let description = text_col(row, 2)?;
    let config_str = text_col(row, 3)?;
    let created_at = int_col(row, 4)?;
    let updated_at = int_col(row, 5)?;

    // The config_json stores everything except id/path/timestamps.
    let config: serde_json::Value = serde_json::from_str(&config_str)?;

    Ok(AgentProfile {
        id: Uuid::parse_str(&id_str)
            .map_err(|e| StorageError::Other(format!("parse profile id: {e}").into()))?,
        path,
        description,
        agent_identity: config
            .get("agent_identity")
            .and_then(|v| v.as_str())
            .unwrap_or("You are a helpful assistant.")
            .to_string(),
        system_prompt: config
            .get("system_prompt")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        enable_bibliography: config
            .get("enable_bibliography")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        enable_writing: config
            .get("enable_writing")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        enable_opengwas: config
            .get("enable_opengwas")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        enable_opentargets: config
            .get("enable_opentargets")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        enable_gwascatalog: config
            .get("enable_gwascatalog")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        enable_iceberg: config
            .get("enable_iceberg")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        enable_dag_history: config
            .get("enable_dag_history")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        preferred_model: config
            .get("preferred_model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        created_at,
        updated_at,
    })
}

/// Serialize the variable fields of a profile into a JSON value for storage
/// in `config_json` (everything except `id`, `name`, `description`, and
/// timestamps, which have their own columns).
fn profile_to_config_json(profile: &AgentProfile) -> serde_json::Value {
    serde_json::json!({
        "agent_identity": profile.agent_identity,
        "system_prompt": profile.system_prompt,
        "enable_bibliography": profile.enable_bibliography,
        "enable_writing": profile.enable_writing,
        "enable_opengwas": profile.enable_opengwas,
        "enable_opentargets": profile.enable_opentargets,
        "enable_gwascatalog": profile.enable_gwascatalog,
        "enable_iceberg": profile.enable_iceberg,
        "enable_dag_history": profile.enable_dag_history,
        "preferred_model": profile.preferred_model,
    })
}

#[async_trait]
impl AgentProfileRegistry for TursoAgentStorage {
    async fn create_profile(&self, profile: AgentProfile) -> Result<(), StorageError> {
        let config_json = serde_json::to_string(&profile_to_config_json(&profile))?;
        self.conn
            .execute(
                "INSERT INTO agent_profiles
                    (id, name, description, config_json, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params_from_iter([
                    Value::Text(profile.id.to_string()),
                    Value::Text(profile.path),
                    Value::Text(profile.description),
                    Value::Text(config_json),
                    Value::Integer(profile.created_at),
                    Value::Integer(profile.updated_at),
                ]),
            )
            .await?;
        Ok(())
    }

    async fn get_profile(&self, id: Uuid) -> Result<Option<AgentProfile>, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, name, description, config_json, created_at, updated_at
                 FROM agent_profiles WHERE id = ?1",
                params_from_iter([Value::Text(id.to_string())]),
            )
            .await?;

        match rows.next().await {
            Ok(Some(row)) => Ok(Some(row_to_profile(&row)?)),
            Ok(None) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    async fn get_profile_by_path(&self, path: &str) -> Result<Option<AgentProfile>, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, name, description, config_json, created_at, updated_at
                 FROM agent_profiles WHERE name = ?1 LIMIT 1",
                params_from_iter([Value::Text(path.to_string())]),
            )
            .await?;

        match rows.next().await {
            Ok(Some(row)) => Ok(Some(row_to_profile(&row)?)),
            Ok(None) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    async fn list_profiles(&self) -> Result<Vec<AgentProfile>, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, name, description, config_json, created_at, updated_at
                 FROM agent_profiles ORDER BY created_at ASC",
                params_from_iter([] as [Value; 0]),
            )
            .await?;

        collect_rows(&mut rows, row_to_profile).await
    }

    async fn list_child_profiles(
        &self,
        parent_path: &str,
    ) -> Result<Vec<AgentProfile>, StorageError> {
        let prefix = format!("{parent_path}/%");
        let mut rows = self
            .conn
            .query(
                "SELECT id, name, description, config_json, created_at, updated_at
                 FROM agent_profiles WHERE name LIKE ?1
                 ORDER BY created_at ASC",
                params_from_iter([Value::Text(prefix)]),
            )
            .await?;

        collect_rows(&mut rows, row_to_profile).await
    }

    async fn update_profile(&self, profile: AgentProfile) -> Result<(), StorageError> {
        let config_json = serde_json::to_string(&profile_to_config_json(&profile))?;
        self.conn
            .execute(
                "UPDATE agent_profiles
                 SET name = ?2,
                     description = ?3,
                     config_json = ?4,
                     updated_at = ?5
                 WHERE id = ?1",
                params_from_iter([
                    Value::Text(profile.id.to_string()),
                    Value::Text(profile.path),
                    Value::Text(profile.description),
                    Value::Text(config_json),
                    Value::Integer(profile.updated_at),
                ]),
            )
            .await?;
        Ok(())
    }

    async fn delete_profile(&self, id: Uuid) -> Result<(), StorageError> {
        self.conn
            .execute(
                "DELETE FROM agent_profiles WHERE id = ?1",
                params_from_iter([Value::Text(id.to_string())]),
            )
            .await?;
        Ok(())
    }
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle::AgentLifecycleStatus;
    use crate::memory::{Memory, MemoryItem};
    use crate::message_ext::AgentMessageExt;

    fn now_ms() -> i64 {
        chrono::Utc::now().timestamp_millis()
    }

    fn sample_record(name: &str) -> AgentRecord {
        AgentRecord {
            id: Uuid::new_v4(),
            name: name.to_string(),
            config_json: serde_json::json!({"name": name}),
            created_at: now_ms(),
            last_active: now_ms(),
        }
    }

    fn sample_snapshot(agent_id: Uuid, ts: i64) -> AgentSnapshot {
        AgentSnapshot {
            snapshot_id: Uuid::new_v4(),
            ts,
            agent_id,
            agent_status: AgentLifecycleStatus::Idle,
            memory: Memory::new(),
            session_id: None,
        }
    }

    // ── Registry ─────────────────────────────────────────────

    #[tokio::test]
    async fn test_upsert_and_get_agent() {
        let store = TursoAgentStorage::open_in_memory().await.unwrap();
        let record = sample_record("test-agent");
        store.upsert_agent(record.clone()).await.unwrap();
        let fetched = store.get_agent(record.id).await.unwrap().unwrap();
        assert_eq!(fetched.id, record.id);
        assert_eq!(fetched.name, "test-agent");
    }

    #[tokio::test]
    async fn test_upsert_replaces() {
        let store = TursoAgentStorage::open_in_memory().await.unwrap();
        let mut record = sample_record("a1");
        store.upsert_agent(record.clone()).await.unwrap();

        record.name = "a1-renamed".into();
        record.last_active = now_ms();
        store.upsert_agent(record.clone()).await.unwrap();

        let fetched = store.get_agent(record.id).await.unwrap().unwrap();
        assert_eq!(fetched.name, "a1-renamed");
        assert_eq!(fetched.created_at, record.created_at);
    }

    #[tokio::test]
    async fn test_list_agents_ordered() {
        let store = TursoAgentStorage::open_in_memory().await.unwrap();
        let r1 = sample_record("first");
        let mut r2 = sample_record("second");
        r2.created_at = r1.created_at + 1000;

        store.upsert_agent(r2).await.unwrap();
        store.upsert_agent(r1.clone()).await.unwrap();

        let agents = store.list_agents().await.unwrap();
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].name, "first");
    }

    #[tokio::test]
    async fn test_delete_agent_cascades() {
        let store = TursoAgentStorage::open_in_memory().await.unwrap();
        let rec = sample_record("doomed");
        let agent_id = rec.id;
        let sess_id = Uuid::new_v4();

        store.upsert_agent(rec).await.unwrap();
        store
            .create_snapshot(sample_snapshot(agent_id, 1000))
            .await
            .unwrap();
        store.start_session(agent_id, sess_id).await.unwrap();
        store
            .append_message(sess_id, &Message::user("hi"))
            .await
            .unwrap();

        store.delete_agent(agent_id).await.unwrap();
        assert!(store.get_agent(agent_id).await.unwrap().is_none());
        assert!(
            store
                .get_agent_snapshots(agent_id)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .get_messages_since(agent_id, 0)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn test_touch_agent() {
        let store = TursoAgentStorage::open_in_memory().await.unwrap();
        let rec = sample_record("touchy");
        store.upsert_agent(rec.clone()).await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        store.touch_agent(rec.id).await.unwrap();

        let fetched = store.get_agent(rec.id).await.unwrap().unwrap();
        assert!(fetched.last_active > rec.last_active);
    }

    // ── Relations ────────────────────────────────────────────

    #[tokio::test]
    async fn test_relations() {
        let store = TursoAgentStorage::open_in_memory().await.unwrap();
        let parent = sample_record("parent");
        let child = sample_record("child");

        store.upsert_agent(parent.clone()).await.unwrap();
        store.upsert_agent(child.clone()).await.unwrap();
        store
            .add_relation(AgentRelation {
                parent_id: parent.id,
                child_id: child.id,
                kind: RelationKind::Spawned,
            })
            .await
            .unwrap();

        assert_eq!(store.list_children(parent.id).await.unwrap().len(), 1);
        assert_eq!(store.list_parents(child.id).await.unwrap().len(), 1);
    }

    // ── Snapshot ─────────────────────────────────────────────

    #[tokio::test]
    async fn test_snapshot_crud() {
        let store = TursoAgentStorage::open_in_memory().await.unwrap();
        let agent_id = Uuid::new_v4();
        let snap = sample_snapshot(agent_id, 1000);

        store.create_snapshot(snap.clone()).await.unwrap();
        let fetched = store.get_snapshot(snap.snapshot_id).await.unwrap();
        assert_eq!(fetched.agent_id, agent_id);

        let latest = store.get_latest_snapshot(agent_id).await.unwrap().unwrap();
        assert_eq!(latest.ts, 1000);
    }

    #[tokio::test]
    async fn test_snapshot_latest_none() {
        let store = TursoAgentStorage::open_in_memory().await.unwrap();
        assert!(
            store
                .get_latest_snapshot(Uuid::new_v4())
                .await
                .unwrap()
                .is_none()
        );
    }

    // ── Session + WAL ────────────────────────────────────────

    #[tokio::test]
    async fn test_session_message_lifecycle() {
        let store = TursoAgentStorage::open_in_memory().await.unwrap();
        let agent_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();

        store.start_session(agent_id, session_id).await.unwrap();
        store
            .append_message(session_id, &Message::user("hello"))
            .await
            .unwrap();
        store
            .append_message(session_id, &Message::user("world"))
            .await
            .unwrap();
        store.end_session(session_id).await.unwrap();

        let msgs = store.get_messages_since(agent_id, 0).await.unwrap();
        assert_eq!(msgs.len(), 2);
    }

    #[tokio::test]
    async fn test_get_messages_since_watermark() {
        let store = TursoAgentStorage::open_in_memory().await.unwrap();
        let agent_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();

        store.start_session(agent_id, session_id).await.unwrap();
        store
            .append_message(session_id, &Message::user("old"))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let watermark = chrono::Utc::now().timestamp_millis();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        store
            .append_message(session_id, &Message::user("new"))
            .await
            .unwrap();

        let msgs = store.get_messages_since(agent_id, watermark).await.unwrap();
        assert_eq!(
            msgs.len(),
            1,
            "only messages after watermark should be returned"
        );
    }

    #[tokio::test]
    async fn test_list_session_records_includes_paused() {
        // Sessions that have been "ended" (paused) must still appear in
        // list_session_records. The agent calls end_session on all sessions
        // during shutdown; if those sessions were excluded from listing, they
        // would disappear on restart.
        let store = TursoAgentStorage::open_in_memory().await.unwrap();
        let agent_id = Uuid::new_v4();
        let s1 = Uuid::new_v4();
        let s2 = Uuid::new_v4();
        let s3 = Uuid::new_v4();

        // s1: active (no end)
        store.start_session(agent_id, s1).await.unwrap();
        // s2: ended (paused by shutdown)
        store.start_session(agent_id, s2).await.unwrap();
        store.end_session(s2).await.unwrap();
        // s3: active (no end)
        store.start_session(agent_id, s3).await.unwrap();

        let records = store.list_session_records(agent_id).await.unwrap();
        // All three must be returned, including the paused one.
        assert_eq!(records.len(), 3, "paused sessions must be included");
        let ids: Vec<Uuid> = records.iter().map(|r| r.session_id).collect();
        assert!(ids.contains(&s1));
        assert!(ids.contains(&s2));
        assert!(ids.contains(&s3));
    }

    #[tokio::test]
    async fn test_delete_session_removes_messages_and_row() {
        let store = TursoAgentStorage::open_in_memory().await.unwrap();
        let agent_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();

        store.start_session(agent_id, session_id).await.unwrap();
        store
            .append_message(session_id, &Message::user("hello"))
            .await
            .unwrap();

        // Verify session + messages exist.
        let msgs = store
            .get_messages_since_for_session(session_id, 0)
            .await
            .unwrap();
        assert_eq!(msgs.len(), 1);

        // Delete the session.
        store.delete_session(session_id).await.unwrap();

        // Session row should be gone.
        let records = store.list_session_records(agent_id).await.unwrap();
        assert!(
            records.iter().all(|r| r.session_id != session_id),
            "session row should be deleted"
        );

        // Messages should be gone.
        let msgs = store
            .get_messages_since_for_session(session_id, 0)
            .await
            .unwrap();
        assert!(msgs.is_empty(), "messages should be deleted");
    }

    #[tokio::test]
    async fn test_start_session_idempotent() {
        // Repeatedly calling start_session with the same ID should not
        // create duplicate rows. This models the fixed run_session() path
        // where the same session ID is reused across conversation turns.
        let store = TursoAgentStorage::open_in_memory().await.unwrap();
        let agent_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();

        // Call start_session 5 times — should still be 1 row.
        for _ in 0..5 {
            store.start_session(agent_id, session_id).await.unwrap();
        }

        let records = store.list_session_records(agent_id).await.unwrap();
        assert_eq!(
            records.len(),
            1,
            "repeated start_session must be idempotent"
        );
        assert_eq!(records[0].session_id, session_id);
    }

    // ── Restore ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_restore_memory() {
        use crate::storage::restore_memory;
        let store = TursoAgentStorage::open_in_memory().await.unwrap();
        let agent_id = Uuid::new_v4();

        // Snapshot with an initial memory state
        let mut snap = sample_snapshot(agent_id, 1000);
        snap.memory = Memory {
            items: vec![MemoryItem {
                messages: vec![Message::user("snapshotted")],
                summary: None,
            }],
            ..Default::default()
        };
        store.create_snapshot(snap).await.unwrap();

        // Messages after the snapshot
        let session_id = Uuid::new_v4();
        store.start_session(agent_id, session_id).await.unwrap();
        // Wait so messages have ts > 1000
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
        store
            .append_message(session_id, &Message::user("after-snapshot"))
            .await
            .unwrap();

        let memory = restore_memory(&store, agent_id).await.unwrap();
        // Should contain both the snapshotted message and the new one
        let all_msgs: Vec<_> = memory
            .items
            .iter()
            .flat_map(|i| i.messages.iter())
            .collect();
        assert!(
            all_msgs.len() >= 2,
            "expected at least 2 messages after restore"
        );
    }

    // ── RelationKind ─────────────────────────────────────────

    #[test]
    fn test_relation_kind_roundtrip() {
        for kind in [
            RelationKind::Spawned,
            RelationKind::Delegated,
            RelationKind::Parallel,
        ] {
            assert_eq!(RelationKind::from_str(kind.as_str()), Some(kind));
        }
        assert_eq!(RelationKind::from_str("unknown"), None);
    }

    // ── End-to-end: register → WAL → snapshot → restore ─────

    #[tokio::test]
    async fn test_e2e_persistence_cycle() {
        use crate::storage::restore_memory;

        let store = TursoAgentStorage::open_in_memory().await.unwrap();

        // 1. Register an agent.
        let agent_id = Uuid::new_v4();
        let now = now_ms();
        store
            .upsert_agent(AgentRecord {
                id: agent_id,
                name: "e2e-agent".into(),
                config_json: serde_json::json!({"key": "value"}),
                created_at: now,
                last_active: now,
            })
            .await
            .unwrap();

        // 2. Session 1: append some messages, snapshot.
        let sess1 = Uuid::new_v4();
        store.start_session(agent_id, sess1).await.unwrap();
        store
            .append_message(sess1, &Message::user("message-1"))
            .await
            .unwrap();
        store
            .append_message(sess1, &Message::user("message-2"))
            .await
            .unwrap();
        store.end_session(sess1).await.unwrap();

        // Snapshot after session 1.
        let snap1 = sample_snapshot(agent_id, now_ms());
        store.create_snapshot(snap1).await.unwrap();

        // 3. Session 2: more messages after the snapshot.
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        let sess2 = Uuid::new_v4();
        store.start_session(agent_id, sess2).await.unwrap();
        store
            .append_message(sess2, &Message::user("message-3"))
            .await
            .unwrap();
        store.end_session(sess2).await.unwrap();

        // 4. Restore.
        let memory = restore_memory(&store, agent_id).await.unwrap();
        let all_msgs: Vec<_> = memory
            .items
            .iter()
            .flat_map(|i| i.messages.iter())
            .collect();
        // Snapshot had empty memory, so all 3 messages should be replayed.
        assert!(
            !all_msgs.is_empty(),
            "restored memory should contain replayed messages"
        );

        // 5. Verify agent record is still there.
        let fetched = store.get_agent(agent_id).await.unwrap().unwrap();
        assert_eq!(fetched.name, "e2e-agent");

        // 6. Verify get_agent_by_name.
        let by_name = store.get_agent_by_name("e2e-agent").await.unwrap().unwrap();
        assert_eq!(by_name.id, agent_id);

        // 7. Delete cascades to everything.
        store.delete_agent(agent_id).await.unwrap();
        assert!(store.get_agent(agent_id).await.unwrap().is_none());
        assert!(
            store
                .get_messages_since(agent_id, 0)
                .await
                .unwrap()
                .is_empty()
        );
    }

    // ── AgentProfileRegistry ─────────────────────────────────

    fn sample_profile(path: &str) -> AgentProfile {
        AgentProfile {
            id: Uuid::new_v4(),
            path: path.to_string(),
            description: format!("Test profile: {path}"),
            agent_identity: "You are a test agent.".into(),
            system_prompt: Some("Custom prompt.".into()),
            enable_bibliography: true,
            enable_writing: true,
            enable_opengwas: false,
            enable_opentargets: true,
            enable_gwascatalog: false,
            enable_iceberg: true,
            enable_dag_history: false,
            preferred_model: Some("anthropic:claude-sonnet-5".into()),
            created_at: now_ms(),
            updated_at: now_ms(),
        }
    }

    #[tokio::test]
    async fn test_profile_create_and_get() {
        let store = TursoAgentStorage::open_in_memory().await.unwrap();
        let profile = sample_profile("test-profile");

        store.create_profile(profile.clone()).await.unwrap();

        let fetched = store.get_profile(profile.id).await.unwrap().unwrap();
        assert_eq!(fetched.id, profile.id);
        assert_eq!(fetched.path, "test-profile");
        assert_eq!(fetched.agent_identity, "You are a test agent.");
        assert_eq!(fetched.system_prompt.as_deref(), Some("Custom prompt."));
        assert!(fetched.enable_bibliography);
        assert!(!fetched.enable_opengwas);
        assert_eq!(
            fetched.preferred_model.as_deref(),
            Some("anthropic:claude-sonnet-5")
        );
    }

    #[tokio::test]
    async fn test_profile_get_by_path() {
        let store = TursoAgentStorage::open_in_memory().await.unwrap();
        let profile = sample_profile("by-path");

        store.create_profile(profile).await.unwrap();

        let fetched = store.get_profile_by_path("by-path").await.unwrap().unwrap();
        assert_eq!(fetched.path, "by-path");
    }

    #[tokio::test]
    async fn test_profile_list_ordered() {
        let store = TursoAgentStorage::open_in_memory().await.unwrap();
        let p1 = sample_profile("alpha");
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let p2 = sample_profile("beta");

        // Insert in reverse order.
        store.create_profile(p2).await.unwrap();
        store.create_profile(p1.clone()).await.unwrap();

        let profiles = store.list_profiles().await.unwrap();
        assert_eq!(profiles.len(), 2);
        // Ordered by created_at ASC.
        assert_eq!(profiles[0].path, "alpha");
        assert_eq!(profiles[1].path, "beta");
    }

    #[tokio::test]
    async fn test_profile_update() {
        let store = TursoAgentStorage::open_in_memory().await.unwrap();
        let mut profile = sample_profile("updatable");
        store.create_profile(profile.clone()).await.unwrap();

        // Mutate fields.
        profile.description = "Updated description.".into();
        profile.enable_opengwas = true;
        profile.agent_identity = "New identity.".into();
        profile.updated_at = now_ms();
        store.update_profile(profile.clone()).await.unwrap();

        let fetched = store.get_profile(profile.id).await.unwrap().unwrap();
        assert_eq!(fetched.description, "Updated description.");
        assert!(fetched.enable_opengwas, "flag should be updated");
        assert_eq!(fetched.agent_identity, "New identity.");
    }

    #[tokio::test]
    async fn test_profile_delete() {
        let store = TursoAgentStorage::open_in_memory().await.unwrap();
        let profile = sample_profile("deletable");
        let id = profile.id;

        store.create_profile(profile).await.unwrap();
        assert!(store.get_profile(id).await.unwrap().is_some());

        store.delete_profile(id).await.unwrap();
        assert!(store.get_profile(id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_seed_defaults_if_empty() {
        let store = TursoAgentStorage::open_in_memory().await.unwrap();

        // Empty → should seed.
        let seeded = store.seed_defaults_if_empty().await.unwrap();
        assert!(seeded, "should seed on empty table");

        let profiles = store.list_profiles().await.unwrap();
        assert_eq!(profiles.len(), 4, "should have 4 default profiles");
        assert!(profiles.iter().any(|p| p.path == "researcher"));
        assert!(profiles.iter().any(|p| p.path == "literature"));
        assert!(profiles.iter().any(|p| p.path == "gwas-analysis"));

        // Non-empty → should NOT seed again.
        let seeded_again = store.seed_defaults_if_empty().await.unwrap();
        assert!(!seeded_again, "should not seed when table has data");

        let profiles2 = store.list_profiles().await.unwrap();
        assert_eq!(profiles2.len(), 4, "should still have 4 profiles");
    }

    #[tokio::test]
    async fn test_seed_migrates_legacy_default() {
        let store = TursoAgentStorage::open_in_memory().await.unwrap();

        // Simulate a legacy DB: only a "default" profile exists.
        let legacy = AgentProfile {
            path: "default".into(),
            description: "legacy".into(),
            agent_identity: "legacy identity".into(),
            ..AgentProfile::new("default")
        };
        store.create_profile(legacy).await.unwrap();

        // Seed should rename default → researcher AND add missing defaults.
        let changed = store.seed_defaults_if_empty().await.unwrap();
        assert!(changed, "migration should make a change");

        let profiles = store.list_profiles().await.unwrap();
        // researcher (migrated from default) + literature + gwas-analysis + writer.
        assert_eq!(profiles.len(), 4);
        assert!(
            profiles.iter().any(|p| p.path == "researcher"),
            "legacy 'default' should be renamed to 'researcher'"
        );
        assert!(
            profiles.iter().all(|p| p.path != "default"),
            "no profile should retain the legacy 'default' path"
        );
        // The migrated researcher should preserve the legacy identity.
        let researcher = profiles.iter().find(|p| p.path == "researcher").unwrap();
        assert_eq!(researcher.agent_identity, "legacy identity");
    }

    #[tokio::test]
    async fn test_list_child_profiles() {
        let store = TursoAgentStorage::open_in_memory().await.unwrap();

        // Create a parent and two children.
        let parent = sample_profile("researcher");
        store.create_profile(parent.clone()).await.unwrap();

        let child1 = parent
            .derive_child("genomics", ProfileOverrides::default())
            .unwrap();
        store.create_profile(child1.clone()).await.unwrap();

        let child2 = parent
            .derive_child("proteomics", ProfileOverrides::default())
            .unwrap();
        store.create_profile(child2.clone()).await.unwrap();

        // Unrelated root profile.
        store
            .create_profile(sample_profile("writer"))
            .await
            .unwrap();

        let children = store.list_child_profiles("researcher").await.unwrap();
        assert_eq!(children.len(), 2);
        let paths: Vec<&str> = children.iter().map(|p| p.path.as_str()).collect();
        assert!(paths.contains(&"researcher/genomics"));
        assert!(paths.contains(&"researcher/proteomics"));
    }

    #[test]
    fn test_profile_name_and_parent_path() {
        let root = AgentProfile::new("researcher");
        assert_eq!(root.name(), "researcher");
        assert_eq!(root.parent_path(), None);
        assert_eq!(root.depth(), 0);

        let child = root
            .derive_child("genomics", ProfileOverrides::default())
            .unwrap();
        assert_eq!(child.name(), "genomics");
        assert_eq!(child.parent_path(), Some("researcher"));
        assert_eq!(child.depth(), 1);

        let grandchild = child
            .derive_child("mr_analysis", ProfileOverrides::default())
            .unwrap();
        assert_eq!(grandchild.name(), "mr_analysis");
        assert_eq!(grandchild.parent_path(), Some("researcher/genomics"));
        assert_eq!(grandchild.depth(), 2);
    }

    #[test]
    fn test_derive_child_inheritance_and_overrides() {
        let parent = AgentProfile {
            path: "researcher".into(),
            description: "Parent desc".into(),
            agent_identity: "Parent identity".into(),
            system_prompt: None,
            enable_bibliography: true,
            enable_writing: false,
            enable_opengwas: true,
            enable_opentargets: true,
            enable_gwascatalog: true,
            enable_iceberg: true,
            enable_dag_history: true,
            preferred_model: None,
            ..AgentProfile::new("researcher")
        };

        // Partial override.
        let overrides = ProfileOverrides {
            description: Some("Genomics specialist".into()),
            agent_identity: Some("You are a genomics expert.".into()),
            enable_writing: Some(true),
            enable_dag_history: Some(false),
            ..Default::default()
        };

        let child = parent.derive_child("genomics", overrides).unwrap();
        assert_eq!(child.path, "researcher/genomics");
        assert_eq!(child.description, "Genomics specialist");
        assert_eq!(child.agent_identity, "You are a genomics expert.");
        // Inherited.
        assert!(child.enable_bibliography);
        assert!(child.enable_opengwas);
        // Overridden.
        assert!(child.enable_writing);
        assert!(!child.enable_dag_history);
    }

    #[test]
    fn test_derive_child_rejects_invalid_segment() {
        let parent = AgentProfile::new("researcher");
        let result = parent.derive_child("Bad Name", ProfileOverrides::default());
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_plan_save_and_load() {
        let store = TursoAgentStorage::open_in_memory().await.unwrap();
        let agent_id = Uuid::new_v4();

        // No plan yet.
        assert!(store.load_plan(agent_id).await.unwrap().is_none());

        // Save a plan.
        let mut plan = AgentPlan::new();
        plan.replace(agentik_types::PlanUpdate {
            explanation: Some("test plan".into()),
            plan: vec![
                agentik_types::PlanStep {
                    step: "First".into(),
                    status: agentik_types::StepStatus::Completed,
                },
                agentik_types::PlanStep {
                    step: "Second".into(),
                    status: agentik_types::StepStatus::InProgress,
                },
            ],
        });
        store.save_plan(agent_id, &plan).await.unwrap();

        // Load it back.
        let loaded = store.load_plan(agent_id).await.unwrap().unwrap();
        assert_eq!(loaded, plan);
        assert_eq!(loaded.update.plan.len(), 2);
        assert_eq!(loaded.update.completed_count(), 1);

        // Overwrite with a new plan (upsert).
        let mut plan2 = AgentPlan::new();
        plan2.replace(agentik_types::PlanUpdate {
            explanation: None,
            plan: vec![agentik_types::PlanStep {
                step: "Only".into(),
                status: agentik_types::StepStatus::Pending,
            }],
        });
        store.save_plan(agent_id, &plan2).await.unwrap();
        let loaded2 = store.load_plan(agent_id).await.unwrap().unwrap();
        assert_eq!(loaded2.update.plan.len(), 1);
        assert_eq!(loaded2.revision, 1);
    }

    // ── Phase 4: agent graph persistence tests ──

    #[tokio::test]
    async fn test_agent_graph_upsert_and_list() {
        let store = TursoAgentStorage::open_in_memory().await.unwrap();
        let agent_id = Uuid::new_v4();
        let now = chrono::Utc::now().timestamp_millis();

        // Initial list is empty.
        assert!(store.list_persisted_agents().await.unwrap().is_empty());

        // Upsert a root agent entry.
        let entry = PersistedAgentGraph {
            path: "/root/researcher".into(),
            parent_path: None,
            profile_path: "root/researcher".into(),
            agent_id,
            status_json: r#"{"Idle":null}"#.into(),
            last_event: None,
            created_at: now,
            updated_at: now,
        };
        store.upsert_agent_graph_entry(entry).await.unwrap();

        // List contains the entry.
        let rows = store.list_persisted_agents().await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].path, "/root/researcher");
        assert_eq!(rows[0].agent_id, agent_id);
        assert_eq!(rows[0].parent_path, None);
        assert_eq!(rows[0].status_json, r#"{"Idle":null}"#);
        assert_eq!(rows[0].last_event, None);

        // Upsert a child agent with a parent.
        let child_id = Uuid::new_v4();
        let child_entry = PersistedAgentGraph {
            path: "/root/researcher/worker".into(),
            parent_path: Some("/root/researcher".into()),
            profile_path: "root/researcher/worker".into(),
            agent_id: child_id,
            status_json: r#"{"Running":null}"#.into(),
            last_event: Some("web_search".into()),
            created_at: now + 1,
            updated_at: now + 1,
        };
        store.upsert_agent_graph_entry(child_entry).await.unwrap();

        let rows = store.list_persisted_agents().await.unwrap();
        assert_eq!(rows.len(), 2);
        // Ordered by updated_at DESC — child comes first.
        assert_eq!(rows[0].path, "/root/researcher/worker");
        assert_eq!(rows[0].parent_path.as_deref(), Some("/root/researcher"));
        assert_eq!(rows[0].last_event.as_deref(), Some("web_search"));
        assert_eq!(rows[1].path, "/root/researcher");
    }

    #[tokio::test]
    async fn test_agent_graph_upsert_preserves_created_at() {
        let store = TursoAgentStorage::open_in_memory().await.unwrap();
        let agent_id = Uuid::new_v4();
        let original_created = 1_000_000_i64;

        let entry = PersistedAgentGraph {
            path: "/root/x".into(),
            parent_path: None,
            profile_path: "root/x".into(),
            agent_id,
            status_json: r#"{"Idle":null}"#.into(),
            last_event: None,
            created_at: original_created,
            updated_at: original_created,
        };
        store.upsert_agent_graph_entry(entry).await.unwrap();

        // Re-upsert with a later updated_at but a "fake" created_at.
        // The ON CONFLICT path should preserve the original created_at
        // (we never overwrite it from excluded.created_at).
        let entry2 = PersistedAgentGraph {
            path: "/root/x".into(),
            parent_path: None,
            profile_path: "root/x".into(),
            agent_id,
            status_json: r#"{"Running":null}"#.into(),
            last_event: Some("tool_x".into()),
            created_at: 9_999_999, // should be ignored on conflict
            updated_at: 2_000_000,
        };
        store.upsert_agent_graph_entry(entry2).await.unwrap();

        let rows = store.list_persisted_agents().await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].created_at, original_created);
        assert_eq!(rows[0].updated_at, 2_000_000);
        assert_eq!(rows[0].status_json, r#"{"Running":null}"#);
        assert_eq!(rows[0].last_event.as_deref(), Some("tool_x"));
    }

    #[tokio::test]
    async fn test_agent_graph_update_status_only() {
        let store = TursoAgentStorage::open_in_memory().await.unwrap();
        let agent_id = Uuid::new_v4();
        let now = chrono::Utc::now().timestamp_millis();

        store
            .upsert_agent_graph_entry(PersistedAgentGraph {
                path: "/root/p".into(),
                parent_path: None,
                profile_path: "root/p".into(),
                agent_id,
                status_json: r#"{"Idle":null}"#.into(),
                last_event: None,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();

        // Update only status + last_event.
        store
            .update_agent_graph_status(
                "/root/p",
                r#"{"AwaitingTool":{"tool":"web_search"}}"#,
                Some("web_search"),
            )
            .await
            .unwrap();

        let rows = store.list_persisted_agents().await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].status_json,
            r#"{"AwaitingTool":{"tool":"web_search"}}"#
        );
        assert_eq!(rows[0].last_event.as_deref(), Some("web_search"));
        // updated_at should be >= the original `now` (millisecond precision
        // means a same-ms update is indistinguishable from a never-updated
        // row, which is acceptable).
        assert!(rows[0].updated_at >= now, "updated_at should not regress");

        // Update with None last_event (clears it).
        store
            .update_agent_graph_status("/root/p", r#"{"Completed":null}"#, None)
            .await
            .unwrap();
        let rows = store.list_persisted_agents().await.unwrap();
        assert_eq!(rows[0].status_json, r#"{"Completed":null}"#);
        assert_eq!(rows[0].last_event, None);

        // Update on non-existent path is a silent no-op (no error).
        store
            .update_agent_graph_status("/root/nonexistent", r#"{"Idle":null}"#, None)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_agent_graph_remove_idempotent() {
        let store = TursoAgentStorage::open_in_memory().await.unwrap();
        let agent_id = Uuid::new_v4();
        let now = chrono::Utc::now().timestamp_millis();

        store
            .upsert_agent_graph_entry(PersistedAgentGraph {
                path: "/root/q".into(),
                parent_path: None,
                profile_path: "root/q".into(),
                agent_id,
                status_json: r#"{"Idle":null}"#.into(),
                last_event: None,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();

        assert_eq!(store.list_persisted_agents().await.unwrap().len(), 1);

        // Remove the entry.
        store.remove_agent_graph_entry("/root/q").await.unwrap();
        assert!(store.list_persisted_agents().await.unwrap().is_empty());

        // Removing again is idempotent (no error).
        store.remove_agent_graph_entry("/root/q").await.unwrap();
        store
            .remove_agent_graph_entry("/root/never_existed")
            .await
            .unwrap();
        assert!(store.list_persisted_agents().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_agent_graph_invalid_uuid_is_rejected() {
        // The `agent_id` column is stored as TEXT; if a row ever ends up
        // with a malformed UUID (data corruption, manual DB edit, schema
        // migration bug), `list_persisted_agents` should surface a clear
        // error rather than panic.
        let store = TursoAgentStorage::open_in_memory().await.unwrap();

        // Bypass the typed API to inject a row with an invalid UUID.
        store
            .conn
            .execute(
                "INSERT INTO agent_graph
                    (path, parent_path, profile_path, agent_id,
                     status_json, last_event, created_at, updated_at)
                 VALUES (?1, NULL, ?2, ?3, ?4, NULL, ?5, ?5)",
                params_from_iter([
                    Value::Text("/root/broken".into()),
                    Value::Text("root/broken".into()),
                    Value::Text("not-a-uuid".into()),
                    Value::Text(r#"{"Idle":null}"#.into()),
                    Value::Integer(0),
                ]),
            )
            .await
            .unwrap();

        let result = store.list_persisted_agents().await;
        assert!(
            result.is_err(),
            "expected StorageError for malformed agent_id UUID"
        );
    }

    #[tokio::test]
    async fn test_full_rename_restart_cycle_with_messages() {
        // Comprehensive regression test: simulate the full agent lifecycle
        // that triggers the "rename → restart → session disappears" bug.
        //
        // 1. Agent starts, session is created with messages
        // 2. Session is renamed (title updated in DB)
        // 3. Agent is shut down (sessions paused: end_session called)
        // 4. Agent is restarted with the same agent_id
        // 5. All sessions including the renamed one must be listable with
        //    their titles and messages intact.
        let store = TursoAgentStorage::open_in_memory().await.unwrap();
        let agent_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();

        // 1. Start session + add messages.
        store.start_session(agent_id, session_id).await.unwrap();
        store
            .append_message(session_id, &Message::user("hello world"))
            .await
            .unwrap();
        store
            .append_message(
                session_id,
                &Message::assistant_text("hi there").with_model("test-model"),
            )
            .await
            .unwrap();

        // 2. Rename the session.
        store
            .update_session_title(session_id, "Important Analysis")
            .await
            .unwrap();

        // 3. Pause: the agent calls end_session on shutdown.
        store.end_session(session_id).await.unwrap();

        // 4. Restart: list_session_records must find the session.
        let records = store.list_session_records(agent_id).await.unwrap();
        assert_eq!(records.len(), 1, "renamed session must survive restart");
        assert_eq!(records[0].session_id, session_id);
        assert_eq!(
            records[0].title.as_deref(),
            Some("Important Analysis"),
            "renamed title must be preserved"
        );

        // 5. Messages must be recoverable.
        let messages = store
            .get_messages_since_for_session(session_id, 0)
            .await
            .unwrap();
        assert_eq!(messages.len(), 2, "all messages must survive restart");
    }

    #[tokio::test]
    async fn test_multiple_sessions_rename_one_all_survive() {
        // Test that renaming one session doesn't somehow corrupt or
        // displace other sessions in the same agent.
        let store = TursoAgentStorage::open_in_memory().await.unwrap();
        let agent_id = Uuid::new_v4();
        let s1 = Uuid::new_v4();
        let s2 = Uuid::new_v4();
        let s3 = Uuid::new_v4();

        // Create three sessions.
        store.start_session(agent_id, s1).await.unwrap();
        store.start_session(agent_id, s2).await.unwrap();
        store.start_session(agent_id, s3).await.unwrap();

        // Rename s2.
        store.update_session_title(s2, "Renamed").await.unwrap();

        // Pause all (agent shutdown).
        store.end_session(s1).await.unwrap();
        store.end_session(s2).await.unwrap();
        store.end_session(s3).await.unwrap();

        // All three must survive with correct titles.
        let records = store.list_session_records(agent_id).await.unwrap();
        assert_eq!(records.len(), 3);
        for rec in &records {
            if rec.session_id == s2 {
                assert_eq!(rec.title.as_deref(), Some("Renamed"));
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Torn-WAL recovery helpers
// ═══════════════════════════════════════════════════════════════════════

/// Return `true` if `e`'s Display looks like turso's open-time "short read
/// on WAL frame" error — i.e. the WAL index (SHM) claims a frame is valid
/// but the WAL file is shorter than expected (truncated, missing, or torn).
///
/// We match on the exact literal turso emits from its WAL reader so we do
/// not falsely trigger recovery on unrelated I/O errors (disk full,
/// permission denied, broken pipe, etc.).
fn is_torn_wal_error(e: &StorageError) -> bool {
    e.to_string().contains("short read on WAL frame")
}

/// Quarantine every WAL / SHM sidecar present next to `db_path` by
/// renaming it to `<db_path>-<suffix>.corrupt-<unix_secs>`. The main DB
/// file at `db_path` is never touched. Safe to call when no sidecars
/// exist (no-op). On rename failure we log and continue so the retry can
/// still attempt the open.
fn quarantine_wal_sidecars(db_path: &Path) {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // Suffixes turso / SQLite may use. `-tshm` is the turso convention
    // observed in the wild; the others are standard SQLite.
    for suffix in ["-wal", "-shm", "-twal", "-tshm"] {
        let sidecar = append_suffix(db_path, suffix);
        if !sidecar.exists() {
            continue;
        }
        let target = append_suffix(db_path, &format!("{suffix}.corrupt-{ts}"));
        match std::fs::rename(&sidecar, &target) {
            Ok(()) => tracing::warn!(
                from = %sidecar.display(),
                to = %target.display(),
                "quarantined torn WAL sidecar"
            ),
            Err(e) => tracing::warn!(
                from = %sidecar.display(),
                to = %target.display(),
                error = %e,
                "failed to quarantine torn WAL sidecar (continuing)"
            ),
        }
    }
}

/// Append `suffix` to `p` literally, without touching any existing
/// extension. (`Path::with_extension` would *replace* the extension,
/// turning `agent.db` into `agent-wal`, which is not what we want.)
fn append_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

// ═══════════════════════════════════════════════════════════════════════
// Tests for torn-WAL recovery
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod wal_recovery_tests {
    use super::*;
    use crate::storage::StorageError;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Std-only equivalent of `tempfile::tempdir()`: creates a uniquely
    /// named subdir under `std::env::temp_dir()` and returns its path.
    /// We don't auto-cleanup; tests are expected to be idempotent and
    /// leave no sidecars behind on success.
    fn fresh_tmpdir(label: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let pid = std::process::id();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "agentik-turso-test-{label}-{pid}-{n}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn is_torn_wal_error_matches_known_phrase() {
        // Real shape of the error observed in production:
        //   "storage error: open agent database: I/O error: short read on WAL
        //    frame at offset 1751032: expected 4096 bytes, got 0"
        let e = StorageError::Other(
            "open agent database: I/O error: short read on WAL frame at offset 1751032: expected 4096 bytes, got 0"
                .into(),
        );
        assert!(is_torn_wal_error(&e));
    }

    #[test]
    fn is_torn_wal_error_rejects_unrelated_io() {
        // Disk-full / permission-denied must NOT trigger recovery.
        for msg in [
            "open agent database: I/O error: disk full",
            "open agent database: permission denied",
            "open agent database: database is locked",
            "create db parent dir: not a directory",
            "connect agent database: broken pipe",
        ] {
            let e = StorageError::Other(msg.into());
            assert!(
                !is_torn_wal_error(&e),
                "false positive on unrelated error: {msg}"
            );
        }
    }

    #[test]
    fn quarantine_renames_present_sidecars_only() {
        let tmp = fresh_tmpdir("quarantine_renames_present_sidecars_only");
        let db = tmp.join("agent.db");
        std::fs::write(&db, b"main-db-bytes").unwrap();

        // Drop a mix of sidecars — only the ones that exist should move.
        std::fs::write(append_suffix(&db, "-tshm"), b"shm").unwrap();
        std::fs::write(append_suffix(&db, "-wal"), b"wal").unwrap();
        // No -shm or -twal — should be skipped silently.

        quarantine_wal_sidecars(&db);

        // Main DB untouched.
        assert!(db.exists());
        assert_eq!(std::fs::read(&db).unwrap(), b"main-db-bytes");

        // Each present sidecar got a .corrupt-<ts> twin and is gone.
        let tshm = append_suffix(&db, "-tshm");
        let wal = append_suffix(&db, "-wal");
        assert!(!tshm.exists(), "-tshm should have been moved");
        assert!(!wal.exists(), "-wal should have been moved");

        // Verify the quarantine target pattern matches what we created.
        let mut found_tshm = false;
        let mut found_wal = false;
        for entry in std::fs::read_dir(&tmp).unwrap() {
            let name = entry.unwrap().file_name();
            let s = name.to_string_lossy();
            if s.starts_with("agent.db-tshm.corrupt-") {
                found_tshm = true;
            }
            if s.starts_with("agent.db-wal.corrupt-") {
                found_wal = true;
            }
        }
        assert!(found_tshm, "expected -tshm.corrupt-* quarantine file");
        assert!(found_wal, "expected -wal.corrupt-* quarantine file");
    }

    #[test]
    fn quarantine_is_noop_when_no_sidecars() {
        let tmp = fresh_tmpdir("quarantine_is_noop_when_no_sidecars");
        let db = tmp.join("agent.db");
        std::fs::write(&db, b"only-main").unwrap();

        // Must not panic, must not delete the main file.
        quarantine_wal_sidecars(&db);
        assert!(db.exists());
        assert_eq!(std::fs::read(&db).unwrap(), b"only-main");
    }
}
