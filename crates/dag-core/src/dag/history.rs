//! Execution audit trail and flow-workspace persistence, backed by a local
//! Turso (SQLite-compatible) database.
//!
//! Each [`DagHistory`] owns a [`turso::Connection`] to a local database file.
//! The connection is opened once at engine-init time and lives for the
//! engine's lifetime — callers never touch raw SQL.
//!
//! Executions are versioned in the append-only `runs` table (see
//! [`RunRecord`]): every invocation of a flow leaves a trace, and the
//! serialized run report carries the per-instance evidence. The session's
//! authored flow and instance resume cache persist per agent in
//! `flow_workspaces` / `flow_resume_cache` so a restarted agent restores its
//! workspace.
//!
//! The legacy node-graph snapshot/ref tables are gone; existing databases
//! that still carry them are unaffected — those rows are simply never read.
//!
//! # Schema
//!
//! ```sql
//! CREATE TABLE runs (                     -- one row per execution, never updated
//!     id              TEXT PRIMARY KEY,   -- uuid minted by the engine per run
//!     ref_name        TEXT NOT NULL,
//!     snapshot_id     TEXT,               -- legacy column; flow runs store NULL
//!     manifest_hash   TEXT NOT NULL,      -- content hash of the executed definition
//!     trigger_source  TEXT,               -- e.g. "agent:/root/researcher"; NULL = internal
//!     started_at      TEXT NOT NULL,      -- ISO 8601 UTC
//!     finished_at     TEXT NOT NULL,
//!     ok              INTEGER NOT NULL,
//!     cancelled       INTEGER NOT NULL DEFAULT 0,
//!     error           TEXT,               -- top-level error summary, if the run errored
//!     message         TEXT,
//!     engine_version  TEXT NOT NULL,
//!     source_revision TEXT NOT NULL,      -- git short sha at build time
//!     run_report_json TEXT                -- serialized run report
//! );
//!
//! CREATE TABLE flow_workspaces (
//!     agent_id TEXT PRIMARY KEY,
//!     flow_json TEXT NOT NULL
//! );
//!
//! CREATE TABLE flow_resume_cache (
//!     agent_id   TEXT PRIMARY KEY,
//!     cache_json TEXT NOT NULL
//! );
//! ```

use std::path::Path;

use serde::Serialize;
use turso::{Value, params_from_iter};

use super::error::DagError;

/// One append-only execution record.
///
/// A `RunRecord` captures a single *execution* of the session flow: which
/// definition it ran (by content hash), who triggered it, and the full run
/// report with per-instance execution evidence. Rows are never updated — a
/// re-run is a new row.
#[derive(Debug, Clone, Serialize)]
pub struct RunRecord {
    /// Engine-minted uuid for this execution.
    pub id: String,
    /// Lineage label the engine recorded, e.g. `"flow"`.
    pub ref_name: String,
    /// Legacy column: snapshot id of the executed manifest. Flow runs store
    /// `None`; historical node-graph rows may still carry an id.
    pub snapshot_id: Option<String>,
    /// Content hash of the executed definition (for flows: the canonical
    /// JSON hash of the authored graph).
    pub manifest_hash: String,
    /// Who initiated the run, e.g. `"agent:/root/researcher"`. `None` for
    /// internal / unattributed executions (tests, direct embeds).
    pub trigger: Option<String>,
    /// ISO 8601 UTC, taken when the engine entered the run.
    pub started_at: String,
    /// ISO 8601 UTC, taken after the run resolved.
    pub finished_at: String,
    pub ok: bool,
    pub cancelled: bool,
    /// Top-level error summary when the run itself returned `Err` (as opposed
    /// to an instance failure, which lives inside `run_report_json`).
    pub error: Option<String>,
    /// Commit message associated with the run, if any.
    pub message: Option<String>,
    pub engine_version: String,
    /// Git short sha of the build (`crate::source_revision`).
    pub source_revision: String,
    /// Serialized run report, when the run produced one.
    pub run_report_json: Option<String>,
}

// ── history store ─────────────────────────────────────────────────────────────

/// Owns the Turso connection and provides all run-record / flow-persistence
/// operations.
///
/// Created once via [`DagHistory::open`] and held for the engine's lifetime.
/// Internally wraps a `turso::Connection` (which is `Arc`-backed and `Clone`),
/// so cloning a `DagHistory` is cheap and safe.
#[derive(Clone)]
pub struct DagHistory {
    conn: turso::Connection,
    strict_evidence: bool,
}

impl DagHistory {
    /// Open (or create) a local history database at `path`.
    ///
    /// Creates the schema if it doesn't exist. The database file is created
    /// on first use. Damaged WAL requires explicit recovery in every mode;
    /// no automatic sidecar quarantine or shortened-history fallback is allowed.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, DagError> {
        Self::open_local(path.as_ref(), false).await
    }

    /// Formal evidence may not discard damaged WAL sidecars and silently
    /// create a shorter history. Corruption requires explicit investigation.
    pub async fn open_strict(path: impl AsRef<Path>) -> Result<Self, DagError> {
        Self::open_local(path.as_ref(), true).await
    }

    pub fn is_strict_evidence(&self) -> bool {
        self.strict_evidence
    }

    async fn open_local(path: &Path, strict_evidence: bool) -> Result<Self, DagError> {
        let path_str = path
            .to_str()
            .ok_or_else(|| DagError::History("history db path is not valid UTF-8".into()))?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                DagError::History(format!("create history db parent directory: {e}"))
            })?;
        }

        Self::open_once(path, strict_evidence, || Self::try_open_local(path_str)).await
    }

    async fn open_once<F, Fut>(
        path: &Path,
        strict_evidence: bool,
        attempt: F,
    ) -> Result<Self, DagError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Self, DagError>>,
    {
        match attempt().await {
            Ok(mut history) => {
                history.strict_evidence = strict_evidence;
                Ok(history)
            }
            Err(error) if is_torn_wal_error(&error) => Err(DagError::History(format!(
                "RECOVERY_REQUIRED: DAG history WAL cannot be read at {}: {error}. \
                 Automatic WAL quarantine/retry is disabled; preserve the database \
                 and sidecars and investigate a verified offline copy",
                path.display()
            ))),
            Err(error) => Err(error),
        }
    }

    async fn try_open_local(path_str: &str) -> Result<Self, DagError> {
        let db = turso::Builder::new_local(path_str)
            .experimental_multiprocess_wal(true)
            .build()
            .await
            .map_err(|e| DagError::History(format!("failed to open history database: {e}")))?;

        let conn = db.connect().map_err(|e| {
            DagError::History(format!("failed to connect to history database: {e}"))
        })?;

        // `multiprocess_wal` above enables WAL (concurrent readers + writer
        // across processes); busy_timeout is per-connection, so re-apply on
        // every open to make contended writes wait instead of erroring.
        conn.pragma_update("busy_timeout", 5000)
            .await
            .map_err(|e| DagError::History(format!("set busy_timeout: {e}")))?;

        let history = Self {
            conn,
            strict_evidence: false,
        };
        history.init_schema().await?;
        Ok(history)
    }

    /// Create an in-memory database (for tests / ephemeral sessions).
    pub async fn open_in_memory() -> Result<Self, DagError> {
        let db = turso::Builder::new_local(":memory:")
            .build()
            .await
            .map_err(|e| DagError::History(format!("failed to create in-memory db: {e}")))?;

        let conn = db
            .connect()
            .map_err(|e| DagError::History(format!("failed to connect in-memory db: {e}")))?;

        let history = Self {
            conn,
            strict_evidence: false,
        };
        history.init_schema().await?;
        Ok(history)
    }

    /// Idempotently create the tables + indexes if they don't exist.
    async fn init_schema(&self) -> Result<(), DagError> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS runs (
                    id              TEXT PRIMARY KEY,
                    ref_name        TEXT NOT NULL,
                    snapshot_id     TEXT,
                    manifest_hash   TEXT NOT NULL,
                    trigger_source  TEXT,
                    started_at      TEXT NOT NULL,
                    finished_at     TEXT NOT NULL,
                    ok              INTEGER NOT NULL,
                    cancelled       INTEGER NOT NULL DEFAULT 0,
                    error           TEXT,
                    message         TEXT,
                    engine_version  TEXT NOT NULL,
                    source_revision TEXT NOT NULL,
                    run_report_json TEXT
                );
                CREATE INDEX IF NOT EXISTS idx_runs_snapshot
                    ON runs(snapshot_id);
                CREATE INDEX IF NOT EXISTS idx_runs_started
                    ON runs(started_at);
                CREATE INDEX IF NOT EXISTS idx_runs_flow_agent_hash
                    ON runs(ref_name, trigger_source, manifest_hash, started_at);

                CREATE TABLE IF NOT EXISTS flow_workspaces (
                    agent_id TEXT PRIMARY KEY,
                    flow_json TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS flow_resume_cache (
                    agent_id   TEXT PRIMARY KEY,
                    cache_json TEXT NOT NULL
                );
                ",
            )
            .await
            .map_err(|e| DagError::History(format!("schema init failed: {e}")))?;
        Ok(())
    }

    /// Load the last committed authored flow for an agent.
    pub async fn load_flow_workspace(&self, agent_id: &str) -> Result<Option<String>, DagError> {
        let mut rows = self
            .conn
            .query(
                "SELECT flow_json FROM flow_workspaces WHERE agent_id = ?1",
                params_from_iter([Value::Text(agent_id.to_owned())]),
            )
            .await
            .map_err(|e| DagError::History(format!("load flow workspace: {e}")))?;
        match rows.next().await {
            Ok(Some(row)) => Ok(Some(text_value(&row, 0)?)),
            Ok(None) => Ok(None),
            Err(e) => Err(DagError::History(format!("load flow workspace row: {e}"))),
        }
    }

    /// Atomically replace an agent's authored flow.
    pub async fn store_flow_workspace(
        &self,
        agent_id: &str,
        flow_json: &str,
    ) -> Result<(), DagError> {
        self.conn.execute(
            "INSERT INTO flow_workspaces (agent_id, flow_json) VALUES (?1, ?2) ON CONFLICT(agent_id) DO UPDATE SET flow_json = ?2",
            params_from_iter([Value::Text(agent_id.to_owned()), Value::Text(flow_json.to_owned())]),
        ).await.map_err(|e| DagError::History(format!("store flow workspace: {e}")))?;
        Ok(())
    }

    /// Load the run cache belonging to a single agent session.
    pub async fn load_flow_cache(&self, agent_id: &str) -> Result<Option<String>, DagError> {
        let mut rows = self
            .conn
            .query(
                "SELECT cache_json FROM flow_resume_cache WHERE agent_id = ?1",
                params_from_iter([Value::Text(agent_id.to_owned())]),
            )
            .await
            .map_err(|e| DagError::History(format!("load flow cache: {e}")))?;
        match rows.next().await {
            Ok(Some(row)) => Ok(Some(text_value(&row, 0)?)),
            Ok(None) => Ok(None),
            Err(e) => Err(DagError::History(format!("load flow cache row: {e}"))),
        }
    }

    /// Atomically replace one agent's last completed cache snapshot.
    pub async fn store_flow_cache(&self, agent_id: &str, cache_json: &str) -> Result<(), DagError> {
        self.conn
            .execute(
                "INSERT INTO flow_resume_cache (agent_id, cache_json) VALUES (?1, ?2) \
             ON CONFLICT(agent_id) DO UPDATE SET cache_json = ?2",
                params_from_iter([
                    Value::Text(agent_id.to_owned()),
                    Value::Text(cache_json.to_owned()),
                ]),
            )
            .await
            .map_err(|e| DagError::History(format!("store flow cache: {e}")))?;
        Ok(())
    }

    /// Append one execution record. Runs are an append-only audit trail —
    /// this is a pure INSERT; corrections are new rows, never updates.
    pub async fn record_run(&self, run: &RunRecord) -> Result<(), DagError> {
        self.conn
            .execute(
                "INSERT INTO runs
                    (id, ref_name, snapshot_id, manifest_hash, trigger_source,
                     started_at, finished_at, ok, cancelled, error, message,
                     engine_version, source_revision, run_report_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params_from_iter([
                    Value::Text(run.id.clone()),
                    Value::Text(run.ref_name.clone()),
                    opt_text(run.snapshot_id.clone()),
                    Value::Text(run.manifest_hash.clone()),
                    opt_text(run.trigger.clone()),
                    Value::Text(run.started_at.clone()),
                    Value::Text(run.finished_at.clone()),
                    Value::Integer(run.ok as i64),
                    Value::Integer(run.cancelled as i64),
                    opt_text(run.error.clone()),
                    opt_text(run.message.clone()),
                    Value::Text(run.engine_version.clone()),
                    Value::Text(run.source_revision.clone()),
                    opt_text(run.run_report_json.clone()),
                ]),
            )
            .await
            .map_err(|e| DagError::History(format!("run insert failed: {e}")))?;
        Ok(())
    }

    /// List recent runs, newest first. `ref_name = None` spans all refs.
    pub async fn list_runs(
        &self,
        limit: usize,
        ref_name: Option<&str>,
    ) -> Result<Vec<RunRecord>, DagError> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, ref_name, snapshot_id, manifest_hash, trigger_source,
                        started_at, finished_at, ok, cancelled, error, message,
                        engine_version, source_revision, run_report_json
                 FROM runs
                 WHERE (?1 IS NULL OR ref_name = ?1)
                 ORDER BY started_at DESC
                 LIMIT ?2",
                params_from_iter([
                    opt_text(ref_name.map(|name| name.to_string())),
                    Value::Integer(limit as i64),
                ]),
            )
            .await
            .map_err(|e| DagError::History(format!("list_runs query: {e}")))?;

        let mut runs = Vec::new();
        loop {
            match rows.next().await {
                Ok(Some(row)) => runs.push(row_to_run(&row)?),
                Ok(None) => break,
                Err(e) => return Err(DagError::History(format!("list_runs row: {e}"))),
            }
        }
        Ok(runs)
    }

    /// Latest run for one agent and authored flow, including failed runs with no report.
    pub async fn latest_flow_run(
        &self,
        agent_id: &str,
        manifest_hash: &str,
    ) -> Result<Option<RunRecord>, DagError> {
        let mut rows = self.conn.query(
            "SELECT id, ref_name, snapshot_id, manifest_hash, trigger_source, started_at, finished_at, ok, cancelled, error, message, engine_version, source_revision, run_report_json FROM runs WHERE ref_name = 'flow' AND trigger_source = ?1 AND manifest_hash = ?2 ORDER BY started_at DESC, rowid DESC LIMIT 1",
            params_from_iter([Value::Text(format!("agent:{agent_id}")), Value::Text(manifest_hash.to_owned())]),
        ).await.map_err(|e| DagError::History(format!("latest flow run query: {e}")))?;
        match rows.next().await {
            Ok(Some(row)) => Ok(Some(row_to_run(&row)?)),
            Ok(None) => Ok(None),
            Err(e) => Err(DagError::History(format!("latest flow run row: {e}"))),
        }
    }

    /// Fetch a single run by id.
    pub async fn get_run(&self, id: &str) -> Result<Option<RunRecord>, DagError> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, ref_name, snapshot_id, manifest_hash, trigger_source,
                        started_at, finished_at, ok, cancelled, error, message,
                        engine_version, source_revision, run_report_json
                 FROM runs WHERE id = ?1",
                params_from_iter([Value::Text(id.to_string())]),
            )
            .await
            .map_err(|e| DagError::History(format!("get_run query: {e}")))?;

        match rows.next().await {
            Ok(Some(row)) => Ok(Some(row_to_run(&row)?)),
            Ok(None) => Ok(None),
            Err(e) => Err(DagError::History(format!("get_run row: {e}"))),
        }
    }
}

/// Extract a TEXT column as `String`.
fn text_value(row: &turso::Row, idx: usize) -> Result<String, DagError> {
    match row.get_value(idx).map_err(val_err)? {
        turso::Value::Text(s) => Ok(s),
        turso::Value::Null => Ok(String::new()),
        other => Err(DagError::History(format!(
            "expected TEXT at column {idx}, got {other:?}"
        ))),
    }
}

/// Extract a nullable TEXT column as `Option<String>`.
fn opt_text_value(row: &turso::Row, idx: usize) -> Result<Option<String>, DagError> {
    match row.get_value(idx).map_err(val_err)? {
        turso::Value::Null => Ok(None),
        turso::Value::Text(s) => Ok(Some(s)),
        other => Err(DagError::History(format!(
            "expected TEXT or NULL at column {idx}, got {other:?}"
        ))),
    }
}

fn val_err(e: turso::Error) -> DagError {
    DagError::History(format!("column read error: {e}"))
}

/// Map an `Option<String>` to a turso parameter value.
fn opt_text(value: Option<String>) -> Value {
    value.map(Value::Text).unwrap_or(Value::Null)
}

fn row_to_run(row: &turso::Row) -> Result<RunRecord, DagError> {
    let bool_value = |idx: usize| -> Result<bool, DagError> {
        match row.get_value(idx).map_err(val_err)? {
            turso::Value::Integer(v) => Ok(v != 0),
            turso::Value::Null => Ok(false),
            other => Err(DagError::History(format!(
                "expected INTEGER at column {idx}, got {other:?}"
            ))),
        }
    };
    Ok(RunRecord {
        id: text_value(row, 0)?,
        ref_name: text_value(row, 1)?,
        snapshot_id: opt_text_value(row, 2)?,
        manifest_hash: text_value(row, 3)?,
        trigger: opt_text_value(row, 4)?,
        started_at: text_value(row, 5)?,
        finished_at: text_value(row, 6)?,
        ok: bool_value(7)?,
        cancelled: bool_value(8)?,
        error: opt_text_value(row, 9)?,
        message: opt_text_value(row, 10)?,
        engine_version: text_value(row, 11)?,
        source_revision: text_value(row, 12)?,
        run_report_json: opt_text_value(row, 13)?,
    })
}

fn is_torn_wal_error(error: &DagError) -> bool {
    error.to_string().contains("short read on WAL frame")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn flow_cache_is_upserted_per_agent() {
        let history = DagHistory::open_in_memory().await.unwrap();
        assert!(history.load_flow_cache("a").await.unwrap().is_none());
        history.store_flow_cache("a", "first").await.unwrap();
        history.store_flow_cache("b", "other").await.unwrap();
        history.store_flow_cache("a", "second").await.unwrap();
        assert_eq!(
            history.load_flow_cache("a").await.unwrap().as_deref(),
            Some("second")
        );
        assert_eq!(
            history.load_flow_cache("b").await.unwrap().as_deref(),
            Some("other")
        );
    }

    #[tokio::test]
    async fn flow_workspace_is_upserted_per_agent() {
        let history = DagHistory::open_in_memory().await.unwrap();
        assert!(history.load_flow_workspace("a").await.unwrap().is_none());
        history
            .store_flow_workspace("a", r#"{"processes":[]}"#)
            .await
            .unwrap();
        history
            .store_flow_workspace("a", r#"{"processes":[{"id":"p"}]}"#)
            .await
            .unwrap();
        assert_eq!(
            history.load_flow_workspace("a").await.unwrap().as_deref(),
            Some(r#"{"processes":[{"id":"p"}]}"#)
        );
    }

    #[test]
    fn torn_wal_error_matches_turso_message() {
        let error = DagError::History(
            "I/O error: short read on WAL frame at offset 2933472: expected 4096 bytes, got 0"
                .into(),
        );
        assert!(is_torn_wal_error(&error));
    }

    #[test]
    fn unrelated_database_errors_do_not_trigger_wal_recovery() {
        for message in ["disk full", "database is locked", "permission denied"] {
            let error = DagError::History(message.into());
            assert!(!is_torn_wal_error(&error));
        }
    }

    #[tokio::test]
    async fn torn_wal_open_refuses_retry_in_both_modes_and_preserves_evidence() {
        for strict in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("dag-history.db");
            let fixtures = [
                ("dag-history.db", b"main".as_slice()),
                ("dag-history.db-wal", b"committed runs"),
                ("dag-history.db-tshm", b"index"),
            ];
            for (name, bytes) in fixtures {
                std::fs::write(dir.path().join(name), bytes).unwrap();
            }
            let calls = std::sync::atomic::AtomicUsize::new(0);
            let result = DagHistory::open_once(&path, strict, || {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                std::future::ready(Err(DagError::History(
                    "I/O error: short read on WAL frame at offset 4096: expected 4096 bytes, got 0"
                        .into(),
                )))
            })
            .await;
            let error = match result {
                Ok(_) => panic!("torn WAL must refuse startup"),
                Err(e) => e,
            };
            assert!(error.to_string().contains("RECOVERY_REQUIRED"));
            assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
            assert_eq!(
                std::fs::read_dir(dir.path()).unwrap().count(),
                fixtures.len()
            );
            for (name, bytes) in fixtures {
                assert_eq!(std::fs::read(dir.path().join(name)).unwrap(), bytes);
            }
        }
    }

    #[tokio::test]
    async fn strict_open_preserves_committed_history_and_rejects_corrupt_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("strict-history.db");
        {
            let history = DagHistory::open_strict(&path).await.unwrap();
            history
                .record_run(&sample_run("111", true))
                .await
                .unwrap();
        }
        let reopened = DagHistory::open_strict(&path).await.unwrap();
        assert!(reopened.is_strict_evidence());
        assert_eq!(reopened.get_run("111").await.unwrap().unwrap().id, "111");
        drop(reopened);
        assert!(!DagHistory::open(&path).await.unwrap().is_strict_evidence());
        assert!(
            !DagHistory::open_in_memory()
                .await
                .unwrap()
                .is_strict_evidence()
        );
        let broken = dir.path().join("broken.db");
        let bytes = b"NOT A DATABASE: preserve failure evidence";
        std::fs::write(&broken, bytes).unwrap();
        assert!(DagHistory::open_strict(&broken).await.is_err());
        assert_eq!(std::fs::read(&broken).unwrap(), bytes);
        assert!(!std::fs::read_dir(dir.path()).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("corrupt-")
        }));
    }

    fn sample_run(id: &str, ok: bool) -> RunRecord {
        RunRecord {
            id: id.to_string(),
            ref_name: "main".to_string(),
            snapshot_id: None,
            manifest_hash: "abc123".to_string(),
            trigger: Some("agent:researcher".to_string()),
            started_at: format!("2026-09-29T10:00:{id}Z"),
            finished_at: format!("2026-09-29T10:01:{id}Z"),
            ok,
            cancelled: false,
            error: if ok { None } else { Some("boom".to_string()) },
            message: Some("flow run".to_string()),
            engine_version: env!("CARGO_PKG_VERSION").to_string(),
            source_revision: crate::source_revision().to_string(),
            run_report_json: Some(r#"{"ok":false}"#.to_string()),
        }
    }

    #[tokio::test]
    async fn record_run_and_list_runs_roundtrip() {
        let history = DagHistory::open_in_memory().await.unwrap();
        history
            .record_run(&sample_run("111", true))
            .await
            .unwrap();
        history
            .record_run(&sample_run("222", false))
            .await
            .unwrap();

        let runs = history.list_runs(10, None).await.unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].id, "222", "newest first");
        assert_eq!(runs[1].id, "111");
        assert_eq!(runs[0].trigger.as_deref(), Some("agent:researcher"));
        assert!(!runs[0].ok);
        assert!(runs[0].error.is_some());
        assert!(runs[1].ok);
        assert_eq!(runs[1].error, None);

        let fetched = history.get_run("111").await.unwrap().unwrap();
        assert_eq!(fetched.id, "111");
        assert_eq!(fetched.source_revision, crate::source_revision());
        assert!(history.get_run("missing").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_runs_filters_by_ref() {
        let history = DagHistory::open_in_memory().await.unwrap();
        let mut main_run = sample_run("111", true);
        main_run.ref_name = "main".to_string();
        let mut other_run = sample_run("222", true);
        other_run.ref_name = "other".to_string();
        history.record_run(&main_run).await.unwrap();
        history.record_run(&other_run).await.unwrap();

        let main_runs = history.list_runs(10, Some("main")).await.unwrap();
        assert_eq!(main_runs.len(), 1);
        assert_eq!(main_runs[0].id, "111");

        let all = history.list_runs(10, None).await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn latest_flow_run_matches_agent_and_hash() {
        let history = DagHistory::open_in_memory().await.unwrap();
        let mut run = sample_run("111", true);
        run.ref_name = "flow".to_string();
        run.manifest_hash = "flowhash".to_string();
        history.record_run(&run).await.unwrap();

        let miss = history
            .latest_flow_run("someone-else", "flowhash")
            .await
            .unwrap();
        assert!(miss.is_none());
        let hit = history.latest_flow_run("researcher", "flowhash").await.unwrap();
        assert_eq!(hit.unwrap().id, "111");
    }
}
