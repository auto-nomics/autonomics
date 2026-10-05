//! Git-like version control for DAG snapshots, backed by a local Turso
//! (SQLite-compatible) database.
//!
//! Each [`DagHistory`] owns a [`turso::Connection`] to a local database file.
//! Snapshots are stored as rows in the `snapshots` table; named refs (branches)
//! track lineage heads. The connection is opened once at engine-init time and
//! lives for the engine's lifetime — callers never touch raw SQL.
//!
//! Snapshots version the DAG *definition*; executions are versioned separately
//! in the append-only `runs` table (see [`RunRecord`]) so that every
//! invocation of a run leaves a trace even when the manifest is unchanged.
//!
//! # Schema
//!
//! ```sql
//! CREATE TABLE snapshots (
//!     id              TEXT PRIMARY KEY,   -- blake3(parent_id || manifest_hash || ts)
//!     parent_id       TEXT,               -- predecessor snapshot (NULL = root)
//!     manifest_hash   TEXT NOT NULL,      -- blake3 of canonical manifest JSON
//!     manifest_json   TEXT NOT NULL,      -- logical source + physical blueprint
//!     run_report_json TEXT,               -- serialized RunReport (NULL if saved without running)
//!     timestamp       TEXT NOT NULL,      -- ISO 8601 UTC
//!     message         TEXT NOT NULL,      -- human-readable label
//!     engine_version  TEXT NOT NULL       -- crate version that produced this snapshot
//! );
//!
//! CREATE TABLE refs (
//!     name         TEXT PRIMARY KEY,      -- e.g. "main"
//!     snapshot_id  TEXT NOT NULL,         -- head of this lineage
//!     pinned       INTEGER DEFAULT 0      -- 1 = tag (immutable ref)
//! );
//!
//! CREATE TABLE runs (                     -- one row per execution, never updated
//!     id              TEXT PRIMARY KEY,   -- uuid minted by the engine per run
//!     ref_name        TEXT NOT NULL,
//!     snapshot_id     TEXT,               -- executed definition (new commit or existing head)
//!     manifest_hash   TEXT NOT NULL,
//!     trigger_source  TEXT,               -- e.g. "agent:/root/researcher"; NULL = internal
//!     started_at      TEXT NOT NULL,      -- ISO 8601 UTC
//!     finished_at     TEXT NOT NULL,
//!     ok              INTEGER NOT NULL,
//!     cancelled       INTEGER NOT NULL DEFAULT 0,
//!     error           TEXT,               -- top-level error summary, if the run errored
//!     message         TEXT,               -- commit message used (or pending), if any
//!     engine_version  TEXT NOT NULL,
//!     source_revision TEXT NOT NULL,      -- git short sha at build time
//!     run_report_json TEXT                -- serialized RunReport (snapshot_id backfilled)
//! );
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use blake3::Hasher;
use serde::{Deserialize, Serialize};
use turso::{Value, params_from_iter};

use super::NodeId;
use super::error::DagError;
use super::logical::LogicalGraph;
use super::physical::PhysicalJobRef;

// ── content-addressable data model ───────────────────────────────────────────

/// Current shape of the persisted DAG manifest.
///
/// Version 1 contained only the expanded physical graph. Version 2 adds the
/// logical source graph(s) and explicit physical-job provenance while keeping
/// those original physical `nodes`/`edges` fields at the top level for
/// backward-compatible deserialization.
pub const MANIFEST_SCHEMA_VERSION: u16 = 2;

/// Version of the logical-to-physical expansion semantics.
///
/// The stored physical graph remains authoritative for exact restore. This
/// version makes compiler drift visible when a historical logical graph is
/// re-expanded with a newer planner.
pub const LOGICAL_COMPILER_VERSION: u16 = 2;

/// A pure-data, fully serializable description of a DAG's logical source and
/// expanded physical topology — sufficient to reconstruct the DAG from a
/// [`crate::registry::NodeRegistry`].
///
/// This is the "tree" object (to borrow git terminology): its content hash
/// identifies a unique DAG configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagManifest {
    #[serde(default = "default_manifest_schema_version")]
    pub schema_version: u16,
    #[serde(default)]
    pub logical: LogicalManifest,
    pub nodes: Vec<NodeEntry>,
    pub edges: Vec<EdgeEntry>,
    #[serde(default)]
    pub physical_jobs: BTreeMap<NodeId, PhysicalJobRef>,
}

/// The logical source layer persisted alongside its compiled physical graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogicalManifest {
    #[serde(default = "default_logical_compiler_version")]
    pub compiler_version: u16,
    #[serde(default)]
    pub graphs: Vec<LogicalGraph>,
}

impl Default for LogicalManifest {
    fn default() -> Self {
        Self {
            compiler_version: default_logical_compiler_version(),
            graphs: Vec::new(),
        }
    }
}

impl Default for DagManifest {
    fn default() -> Self {
        Self::from_parts(Vec::new(), Vec::new())
    }
}

fn default_manifest_schema_version() -> u16 {
    1
}

fn default_logical_compiler_version() -> u16 {
    LOGICAL_COMPILER_VERSION
}

/// One node's identity + configuration in a manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeEntry {
    pub id: NodeId,
    /// Factory kind string (e.g. `"file_to_dataframe"`, `"sql"`), matching
    /// [`crate::registry::NodeFactory::kind`].
    pub kind: String,
    /// The original (post-normalization) spec JSON used to build this node.
    pub spec: serde_json::Value,
}

/// One edge's connectivity in a manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeEntry {
    pub from: NodeId,
    pub from_port: u8,
    pub to: NodeId,
    pub to_port: u8,
}

impl DagManifest {
    /// Validate cross-layer metadata before persisting or restoring a manifest.
    pub fn validate_layers(&self) -> Result<(), DagError> {
        if self.schema_version == 0 || self.schema_version > MANIFEST_SCHEMA_VERSION {
            return Err(DagError::History(format!(
                "unsupported DAG manifest schema version {} (supported: 1..={MANIFEST_SCHEMA_VERSION})",
                self.schema_version
            )));
        }
        if self.logical.compiler_version > LOGICAL_COMPILER_VERSION {
            return Err(DagError::History(format!(
                "unsupported logical compiler version {} (supported: <={LOGICAL_COMPILER_VERSION})",
                self.logical.compiler_version
            )));
        }

        let physical_ids = self
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<BTreeSet<_>>();
        let logical_ids = self
            .logical
            .graphs
            .iter()
            .flat_map(|graph| graph.nodes().iter().map(|node| node.id.as_str()))
            .collect::<BTreeSet<_>>();
        for (physical_id, job) in &self.physical_jobs {
            if !physical_ids.contains(physical_id.as_str()) {
                return Err(DagError::History(format!(
                    "physical job provenance references missing physical node `{physical_id}`"
                )));
            }
            if !logical_ids.contains(job.logical_node.as_str()) {
                return Err(DagError::History(format!(
                    "physical node `{physical_id}` references missing logical node `{}`",
                    job.logical_node
                )));
            }
        }
        Ok(())
    }

    /// Compute the blake3 content hash of this manifest's canonical JSON form.
    ///
    /// Two manifests with the same logical graphs, physical nodes + edges, and
    /// physical-job provenance always produce the same hash — enabling dedup at
    /// the storage layer.
    pub fn content_hash(&self) -> String {
        // serde_json with sorted keys gives a canonical byte sequence.
        let json = serde_json::to_vec(self).unwrap_or_default();
        let mut hasher = Hasher::new();
        hasher.update(&json);
        hasher.finalize().to_hex().to_string()
    }

    /// Build a manifest from raw DAG state (used by [`super::graph::DAG::to_manifest`]).
    pub fn from_parts(nodes: Vec<NodeEntry>, edges: Vec<EdgeEntry>) -> Self {
        DagManifest {
            schema_version: MANIFEST_SCHEMA_VERSION,
            logical: LogicalManifest::default(),
            nodes,
            edges,
            physical_jobs: BTreeMap::new(),
        }
    }
}

/// One immutable point-in-time record of a DAG configuration + optional run
/// result. The "commit" object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub parent_id: Option<String>,
    pub manifest_hash: String,
    pub manifest_json: String,
    pub run_report_json: Option<String>,
    pub timestamp: String,
    pub message: String,
    pub engine_version: String,
}

impl Snapshot {
    /// Deserialize the embedded manifest.
    pub fn manifest(&self) -> Result<DagManifest, serde_json::Error> {
        serde_json::from_str(&self.manifest_json)
    }
}

/// One append-only execution record.
///
/// Where a [`Snapshot`] captures a DAG *definition* change, a `RunRecord`
/// captures a single *execution*: which definition it ran (newly committed
/// snapshot or unchanged head), who triggered it, and the full `RunReport`
/// with per-node execution evidence. Rows are never updated — a re-run is a
/// new row.
#[derive(Debug, Clone, Serialize)]
pub struct RunRecord {
    /// Engine-minted uuid for this execution.
    pub id: String,
    /// Ref lineage the engine was on (`history_ref`), e.g. `"main"`.
    pub ref_name: String,
    /// Snapshot of the executed manifest: the id committed by this run, or
    /// the existing head when the manifest was unchanged. `None` only when no
    /// history store resolved a head.
    pub snapshot_id: Option<String>,
    pub manifest_hash: String,
    /// Who initiated the run, e.g. `"agent:/root/researcher"`. `None` for
    /// internal / unattributed executions (tests, direct embeds).
    pub trigger: Option<String>,
    /// ISO 8601 UTC, taken when the engine entered the run.
    pub started_at: String,
    /// ISO 8601 UTC, taken after the run (and snapshot commit) resolved.
    pub finished_at: String,
    pub ok: bool,
    pub cancelled: bool,
    /// Top-level error summary when the run itself returned `Err` (as opposed
    /// to a node failure, which lives inside `run_report_json`).
    pub error: Option<String>,
    /// Commit message associated with the run, if any.
    pub message: Option<String>,
    pub engine_version: String,
    /// Git short sha of the build (`crate::source_revision`).
    pub source_revision: String,
    /// Serialized `RunReport` with `snapshot_id` backfilled, when the run
    /// produced one.
    pub run_report_json: Option<String>,
}

// ── history store ─────────────────────────────────────────────────────────────

/// Owns the Turso connection and provides all snapshot / ref operations.
///
/// Created once via [`DagHistory::open`] and held for the engine's lifetime.
/// Internally wraps a `turso::Connection` (which is `Arc`-backed and `Clone`),
/// so cloning a `DagHistory` is cheap and safe.
#[derive(Clone)]
pub struct DagHistory {
    conn: turso::Connection,
}

impl DagHistory {
    /// Open (or create) a local history database at `path`.
    ///
    /// Creates the schema if it doesn't exist. The database file is created
    /// on first use.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, DagError> {
        let path = path.as_ref();
        let path_str = path
            .to_str()
            .ok_or_else(|| DagError::History("history db path is not valid UTF-8".into()))?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                DagError::History(format!("create history db parent directory: {e}"))
            })?;
        }

        match Self::try_open_local(path_str).await {
            Ok(history) => Ok(history),
            Err(error) if is_torn_wal_error(&error) => {
                tracing::warn!(
                    db = path_str,
                    error = %error,
                    "DAG history WAL torn on open; quarantining sidecars and retrying"
                );
                quarantine_wal_sidecars(path);
                Self::try_open_local(path_str).await
            }
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

        let history = Self { conn };
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

        let history = Self { conn };
        history.init_schema().await?;
        Ok(history)
    }

    /// Idempotently create the tables + indexes if they don't exist.
    async fn init_schema(&self) -> Result<(), DagError> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS snapshots (
                    id              TEXT PRIMARY KEY,
                    parent_id       TEXT,
                    manifest_hash   TEXT NOT NULL,
                    manifest_json   TEXT NOT NULL,
                    run_report_json TEXT,
                    timestamp       TEXT NOT NULL,
                    message         TEXT NOT NULL,
                    engine_version  TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_snapshots_parent
                    ON snapshots(parent_id);
                CREATE INDEX IF NOT EXISTS idx_snapshots_manifest_hash
                    ON snapshots(manifest_hash);

                CREATE TABLE IF NOT EXISTS refs (
                    name         TEXT PRIMARY KEY,
                    snapshot_id  TEXT NOT NULL,
                    pinned       INTEGER DEFAULT 0
                );

                CREATE TABLE IF NOT EXISTS runs (
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
                ",
            )
            .await
            .map_err(|e| DagError::History(format!("schema init failed: {e}")))?;
        Ok(())
    }

    /// Commit a new snapshot and advance `ref_name` to point at it.
    ///
    /// If the manifest hash matches the current head's manifest hash, the
    /// snapshot is still recorded (with a new timestamp) but the ref is
    /// updated — this preserves the "every run = one snapshot" audit trail
    /// even when the DAG didn't change.
    ///
    /// Returns the new snapshot id.
    pub async fn commit(
        &self,
        ref_name: &str,
        manifest: &DagManifest,
        run_report: Option<&impl Serialize>,
        message: &str,
    ) -> Result<String, DagError> {
        manifest.validate_layers()?;
        let manifest_hash = manifest.content_hash();
        let manifest_json = serde_json::to_string(manifest)
            .map_err(|e| DagError::History(format!("manifest serialization failed: {e}")))?;

        let run_report_json = match run_report {
            Some(rr) => Some(
                serde_json::to_string(rr)
                    .map_err(|e| DagError::History(format!("run_report serialization: {e}")))?,
            ),
            None => None,
        };

        // Resolve parent from the current ref head.
        let parent_id = self.ref_head(ref_name).await?.map(|s| s.id);
        let parent_str = parent_id.as_deref().unwrap_or("");

        let timestamp = chrono::Utc::now().to_rfc3339();
        let engine_version = crate::engine_version().to_string();

        // Snapshot id = blake3(parent_id || manifest_hash || timestamp || message).
        let mut hasher = Hasher::new();
        hasher.update(parent_str.as_bytes());
        hasher.update(manifest_hash.as_bytes());
        hasher.update(timestamp.as_bytes());
        hasher.update(message.as_bytes());
        let snapshot_id = hasher.finalize().to_hex().to_string();

        self.conn
            .execute(
                "INSERT INTO snapshots
                    (id, parent_id, manifest_hash, manifest_json, run_report_json,
                     timestamp, message, engine_version)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params_from_iter([
                    Value::Text(snapshot_id.clone()),
                    parent_id.clone().map(Value::Text).unwrap_or(Value::Null),
                    Value::Text(manifest_hash.clone()),
                    Value::Text(manifest_json),
                    run_report_json.map(Value::Text).unwrap_or(Value::Null),
                    Value::Text(timestamp),
                    Value::Text(message.to_string()),
                    Value::Text(engine_version),
                ]),
            )
            .await
            .map_err(|e| DagError::History(format!("snapshot insert failed: {e}")))?;

        // Advance the ref.  UPSERT handles both "create new ref" and
        // "advance existing ref".
        self.conn
            .execute(
                "INSERT INTO refs (name, snapshot_id, pinned)
                 VALUES (?1, ?2, 0)
                 ON CONFLICT(name) DO UPDATE SET snapshot_id = ?2
                 WHERE pinned = 0",
                params_from_iter([
                    Value::Text(ref_name.to_string()),
                    Value::Text(snapshot_id.clone()),
                ]),
            )
            .await
            .map_err(|e| DagError::History(format!("ref update failed: {e}")))?;

        Ok(snapshot_id)
    }

    /// Read the snapshot that `ref_name` currently points to.
    /// Returns `Ok(None)` if the ref doesn't exist.
    pub async fn ref_head(&self, ref_name: &str) -> Result<Option<Snapshot>, DagError> {
        let mut rows = self
            .conn
            .query(
                "SELECT s.id, s.parent_id, s.manifest_hash, s.manifest_json,
                        s.run_report_json, s.timestamp, s.message, s.engine_version
                 FROM refs r
                 JOIN snapshots s ON r.snapshot_id = s.id
                 WHERE r.name = ?1",
                params_from_iter([Value::Text(ref_name.to_string())]),
            )
            .await
            .map_err(|e| DagError::History(format!("ref_head query failed: {e}")))?;

        match rows.next().await {
            Ok(Some(row)) => {
                let snapshot = self.row_to_snapshot(&row)?;
                Ok(Some(snapshot))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(DagError::History(format!("ref_head row read: {e}"))),
        }
    }

    /// Walk the lineage backwards from `ref_name`, newest-first.
    pub async fn log(&self, ref_name: &str, limit: usize) -> Result<Vec<Snapshot>, DagError> {
        let mut snapshots = Vec::new();
        let mut current = self.ref_head(ref_name).await?;

        while let Some(snap) = current {
            if snapshots.len() >= limit {
                break;
            }
            let parent_id = snap.parent_id.clone();
            snapshots.push(snap);
            current = match parent_id {
                Some(pid) => self.get_snapshot(&pid).await?,
                None => None,
            };
        }
        Ok(snapshots)
    }

    /// Fetch a single snapshot by id.
    pub async fn get_snapshot(&self, id: &str) -> Result<Option<Snapshot>, DagError> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, parent_id, manifest_hash, manifest_json,
                        run_report_json, timestamp, message, engine_version
                 FROM snapshots WHERE id = ?1",
                params_from_iter([Value::Text(id.to_string())]),
            )
            .await
            .map_err(|e| DagError::History(format!("get_snapshot query: {e}")))?;

        match rows.next().await {
            Ok(Some(row)) => Ok(Some(self.row_to_snapshot(&row)?)),
            Ok(None) => Ok(None),
            Err(e) => Err(DagError::History(format!("get_snapshot row read: {e}"))),
        }
    }

    /// Create a new branch pointing at the current head of `from_ref`.
    pub async fn branch(&self, name: &str, from_ref: &str) -> Result<(), DagError> {
        if self.ref_head(name).await?.is_some() {
            return Err(DagError::History(format!(
                "ref '{name}' already exists. Use a different name."
            )));
        }

        let head = self
            .ref_head(from_ref)
            .await?
            .ok_or_else(|| DagError::History(format!("ref '{from_ref}' does not exist")))?;

        self.conn
            .execute(
                "INSERT INTO refs (name, snapshot_id, pinned)
                 VALUES (?1, ?2, 0)",
                params_from_iter([Value::Text(name.to_string()), Value::Text(head.id.clone())]),
            )
            .await
            .map_err(|e| DagError::History(format!("branch create failed: {e}")))?;
        Ok(())
    }

    /// Create a new branch pointing at an **arbitrary** snapshot (not just a
    /// ref head). Use this to diverge from any historical point.
    ///
    /// The snapshot id is resolved via [`Self::resolve_snapshot`] so short
    /// hash prefixes are accepted.
    pub async fn branch_from_snapshot(
        &self,
        name: &str,
        snapshot_id_or_prefix: &str,
    ) -> Result<(), DagError> {
        // Reject if the ref name already exists — never silently overwrite.
        if self.ref_head(name).await?.is_some() {
            return Err(DagError::History(format!(
                "ref '{name}' already exists. Use a different name, or \
                 switch_dag_ref to activate it."
            )));
        }

        let snap = self
            .resolve_snapshot(snapshot_id_or_prefix)
            .await?
            .ok_or_else(|| {
                DagError::History(format!("snapshot '{snapshot_id_or_prefix}' not found"))
            })?;

        self.conn
            .execute(
                "INSERT INTO refs (name, snapshot_id, pinned)
                 VALUES (?1, ?2, 0)",
                params_from_iter([Value::Text(name.to_string()), Value::Text(snap.id.clone())]),
            )
            .await
            .map_err(|e| DagError::History(format!("branch_from_snapshot failed: {e}")))?;
        Ok(())
    }

    /// Resolve a snapshot by exact id or short-hash prefix.
    ///
    /// Tries exact match first; if that fails, searches all snapshots for
    /// ids starting with the given prefix. Returns `Ok(None)` if no match.
    pub async fn resolve_snapshot(&self, id_or_prefix: &str) -> Result<Option<Snapshot>, DagError> {
        // Exact match.
        if let Some(snap) = self.get_snapshot(id_or_prefix).await? {
            return Ok(Some(snap));
        }

        // Prefix search across all snapshots.
        let pattern = format!("{id_or_prefix}%");
        let mut rows = self
            .conn
            .query(
                "SELECT id, parent_id, manifest_hash, manifest_json,
                        run_report_json, timestamp, message, engine_version
                 FROM snapshots WHERE id LIKE ?1
                 ORDER BY timestamp DESC
                 LIMIT 1",
                params_from_iter([Value::Text(pattern)]),
            )
            .await
            .map_err(|e| DagError::History(format!("resolve_snapshot query: {e}")))?;

        match rows.next().await {
            Ok(Some(row)) => Ok(Some(self.row_to_snapshot(&row)?)),
            Ok(None) => Ok(None),
            Err(e) => Err(DagError::History(format!("resolve_snapshot row: {e}"))),
        }
    }

    /// Reset `ref_name` to a historical snapshot (does not delete any objects).
    pub async fn reset(&self, ref_name: &str, target_id: &str) -> Result<(), DagError> {
        // Verify the target exists.
        let _ = self
            .get_snapshot(target_id)
            .await?
            .ok_or_else(|| DagError::History(format!("snapshot '{target_id}' not found")))?;

        self.conn
            .execute(
                "UPDATE refs SET snapshot_id = ?2 WHERE name = ?1 AND pinned = 0",
                params_from_iter([
                    Value::Text(ref_name.to_string()),
                    Value::Text(target_id.to_string()),
                ]),
            )
            .await
            .map_err(|e| DagError::History(format!("reset failed: {e}")))?;
        Ok(())
    }

    /// List all refs.
    pub async fn list_refs(&self) -> Result<Vec<(String, String, bool)>, DagError> {
        let mut rows = self
            .conn
            .query(
                "SELECT name, snapshot_id, pinned FROM refs ORDER BY name",
                (),
            )
            .await
            .map_err(|e| DagError::History(format!("list_refs query: {e}")))?;

        let mut refs = Vec::new();
        loop {
            match rows.next().await {
                Ok(Some(row)) => {
                    let name = text_value(&row, 0)?;
                    let snapshot_id = text_value(&row, 1)?;
                    let pinned = match row.get_value(2).map_err(val_err)? {
                        turso::Value::Integer(v) => v != 0,
                        _ => false,
                    };
                    refs.push((name, snapshot_id, pinned));
                }
                Ok(None) => break,
                Err(e) => return Err(DagError::History(format!("list_refs row: {e}"))),
            }
        }
        Ok(refs)
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

    // ── helpers ───────────────────────────────────────────────────────────────

    fn row_to_snapshot(&self, row: &turso::Row) -> Result<Snapshot, DagError> {
        Ok(Snapshot {
            id: text_value(row, 0)?,
            parent_id: opt_text_value(row, 1)?,
            manifest_hash: text_value(row, 2)?,
            manifest_json: text_value(row, 3)?,
            run_report_json: opt_text_value(row, 4)?,
            timestamp: text_value(row, 5)?,
            message: text_value(row, 6)?,
            engine_version: text_value(row, 7)?,
        })
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

fn quarantine_wal_sidecars(db_path: &Path) {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);

    for suffix in ["-wal", "-shm", "-twal", "-tshm"] {
        let sidecar = append_path_suffix(db_path, suffix);
        if !sidecar.exists() {
            continue;
        }
        let target = append_path_suffix(db_path, &format!("{suffix}.corrupt-{timestamp}"));
        match std::fs::rename(&sidecar, &target) {
            Ok(()) => tracing::warn!(
                from = %sidecar.display(),
                to = %target.display(),
                "quarantined DAG history WAL sidecar"
            ),
            Err(error) => tracing::warn!(
                from = %sidecar.display(),
                to = %target.display(),
                error = %error,
                "failed to quarantine DAG history WAL sidecar"
            ),
        }
    }
}

fn append_path_suffix(path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut result = path.as_os_str().to_owned();
    result.push(suffix);
    std::path::PathBuf::from(result)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn quarantine_moves_history_sidecars_without_touching_main_db() {
        let directory = tempfile::tempdir().unwrap();
        let db_path = directory.path().join("dag-history.db");
        std::fs::write(&db_path, b"main-db").unwrap();
        std::fs::write(append_path_suffix(&db_path, "-wal"), b"wal").unwrap();
        std::fs::write(append_path_suffix(&db_path, "-tshm"), b"tshm").unwrap();

        quarantine_wal_sidecars(&db_path);

        assert_eq!(std::fs::read(&db_path).unwrap(), b"main-db");
        assert!(!append_path_suffix(&db_path, "-wal").exists());
        assert!(!append_path_suffix(&db_path, "-tshm").exists());
        let names: Vec<String> = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert!(
            names
                .iter()
                .any(|name| name.starts_with("dag-history.db-wal.corrupt-"))
        );
        assert!(
            names
                .iter()
                .any(|name| name.starts_with("dag-history.db-tshm.corrupt-"))
        );
    }

    #[tokio::test]
    async fn open_in_memory_and_init_schema() {
        let history = DagHistory::open_in_memory().await.unwrap();
        // A fresh in-memory db has no refs yet.
        let refs = history.list_refs().await.unwrap();
        assert!(refs.is_empty());
    }

    #[tokio::test]
    async fn commit_and_read_back() {
        let history = DagHistory::open_in_memory().await.unwrap();

        let manifest = DagManifest {
            nodes: vec![NodeEntry {
                id: "src".into(),
                kind: "file_to_dataframe".into(),
                spec: serde_json::json!({"path": "/tmp/x.csv"}),
            }],
            edges: vec![],
            ..Default::default()
        };

        let id1 = history
            .commit("main", &manifest, None::<&RunReportStub>, "first run")
            .await
            .unwrap();
        assert!(!id1.is_empty());

        let head = history.ref_head("main").await.unwrap().unwrap();
        assert_eq!(head.id, id1);
        assert!(head.parent_id.is_none());
        assert_eq!(head.message, "first run");

        let manifest_back = head.manifest().unwrap();
        assert_eq!(manifest_back.nodes.len(), 1);
        assert_eq!(manifest_back.nodes[0].id, "src");
    }

    #[tokio::test]
    async fn lineage_chain() {
        let history = DagHistory::open_in_memory().await.unwrap();

        let m1 = DagManifest {
            nodes: vec![NodeEntry {
                id: "a".into(),
                kind: "echo".into(),
                spec: serde_json::json!({}),
            }],
            edges: vec![],
            ..Default::default()
        };
        let m2 = DagManifest {
            nodes: vec![
                NodeEntry {
                    id: "a".into(),
                    kind: "echo".into(),
                    spec: serde_json::json!({}),
                },
                NodeEntry {
                    id: "b".into(),
                    kind: "sql".into(),
                    spec: serde_json::json!({"sql_query": "SELECT 1"}),
                },
            ],
            edges: vec![],
            ..Default::default()
        };

        let _ = history
            .commit("main", &m1, None::<&RunReportStub>, "v1")
            .await
            .unwrap();
        let id2 = history
            .commit("main", &m2, None::<&RunReportStub>, "v2")
            .await
            .unwrap();

        let log = history.log("main", 10).await.unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].id, id2); // newest first
        assert_eq!(log[0].parent_id.as_deref(), Some(log[1].id.as_str()));
        assert!(log[1].parent_id.is_none()); // root
    }

    #[tokio::test]
    async fn branch_and_reset() {
        let history = DagHistory::open_in_memory().await.unwrap();

        let m = DagManifest {
            nodes: vec![NodeEntry {
                id: "a".into(),
                kind: "echo".into(),
                spec: serde_json::json!({}),
            }],
            edges: vec![],
            ..Default::default()
        };

        let id1 = history
            .commit("main", &m, None::<&RunReportStub>, "v1")
            .await
            .unwrap();

        history.branch("experiment", "main").await.unwrap();
        let exp_head = history.ref_head("experiment").await.unwrap().unwrap();
        assert_eq!(exp_head.id, id1);

        let id2 = history
            .commit("main", &m, None::<&RunReportStub>, "v2")
            .await
            .unwrap();

        // main advanced, experiment still points at v1.
        assert_eq!(history.ref_head("main").await.unwrap().unwrap().id, id2);
        assert_eq!(
            history.ref_head("experiment").await.unwrap().unwrap().id,
            id1
        );

        // Reset main back to v1.
        history.reset("main", &id1).await.unwrap();
        assert_eq!(history.ref_head("main").await.unwrap().unwrap().id, id1);
    }

    #[tokio::test]
    async fn manifest_content_hash_is_deterministic() {
        let m = DagManifest {
            nodes: vec![NodeEntry {
                id: "x".into(),
                kind: "sql".into(),
                spec: serde_json::json!({"sql_query": "SELECT 1"}),
            }],
            edges: vec![EdgeEntry {
                from: "x".into(),
                from_port: 0,
                to: "y".into(),
                to_port: 0,
            }],
            ..Default::default()
        };
        let h1 = m.content_hash();
        let h2 = m.content_hash();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // blake3 hex = 32 bytes = 64 chars
    }

    #[test]
    fn manifest_deserializes_v1_physical_only_snapshots() {
        let manifest: DagManifest = serde_json::from_str(r#"{"nodes":[],"edges":[]}"#).unwrap();
        assert_eq!(manifest.schema_version, 1);
        assert!(manifest.logical.graphs.is_empty());
        assert!(manifest.physical_jobs.is_empty());
    }

    #[test]
    fn manifest_roundtrips_logical_and_physical_layers() {
        let graph = super::super::LogicalGraph::builder()
            .add_node(super::super::LogicalNode::for_each(
                "read",
                "file_to_dataframe",
                serde_json::json!({"path": "{{item.path}}"}),
                "sample",
                vec![serde_json::json!({"key": "a", "path": "/tmp/a.csv"})],
            ))
            .build();
        let manifest = DagManifest {
            logical: LogicalManifest {
                graphs: vec![graph],
                ..Default::default()
            },
            ..Default::default()
        };

        let encoded = serde_json::to_string(&manifest).unwrap();
        let decoded: DagManifest = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded.schema_version, MANIFEST_SCHEMA_VERSION);
        assert_eq!(decoded.logical.compiler_version, LOGICAL_COMPILER_VERSION);
        assert_eq!(decoded.logical.graphs.len(), 1);
        assert_eq!(decoded.logical.graphs[0].nodes()[0].id, "read");
    }

    #[tokio::test]
    async fn commit_rejects_invalid_layer_provenance() {
        let history = DagHistory::open_in_memory().await.unwrap();
        let manifest = DagManifest {
            nodes: vec![NodeEntry {
                id: "read#sample=a".into(),
                kind: "echo".into(),
                spec: serde_json::json!({}),
            }],
            physical_jobs: BTreeMap::from([(
                "read#sample=a".to_string(),
                PhysicalJobRef {
                    logical_node: "read".to_string(),
                    axis: Some("sample".to_string()),
                    item_key: Some("a".to_string()),
                    item: Some(serde_json::json!({"key": "a"})),
                },
            )]),
            ..Default::default()
        };

        let error = history
            .commit("main", &manifest, None::<&RunReportStub>, "invalid")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("missing logical node `read`"));
    }

    #[test]
    fn rejects_unknown_manifest_schema_version() {
        let manifest = DagManifest {
            schema_version: MANIFEST_SCHEMA_VERSION + 1,
            ..Default::default()
        };
        let error = manifest.validate_layers().unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unsupported DAG manifest schema")
        );
    }

    /// A stub type implementing Serialize to stand in for RunReport in tests.
    #[derive(Serialize)]
    struct RunReportStub {
        ok: bool,
    }

    fn sample_run(id: &str, ok: bool, snapshot_id: Option<String>) -> RunRecord {
        RunRecord {
            id: id.to_string(),
            ref_name: "main".to_string(),
            snapshot_id,
            manifest_hash: "abc123".to_string(),
            trigger: Some("agent:/root/researcher".to_string()),
            started_at: format!("2026-09-29T10:00:{id}Z"),
            finished_at: format!("2026-09-29T10:01:{id}Z"),
            ok,
            cancelled: false,
            error: if ok { None } else { Some("boom".to_string()) },
            message: Some("auto-snapshot after run".to_string()),
            engine_version: env!("CARGO_PKG_VERSION").to_string(),
            source_revision: crate::source_revision().to_string(),
            run_report_json: Some(r#"{"ok":false}"#.to_string()),
        }
    }

    #[tokio::test]
    async fn record_run_and_list_runs_roundtrip() {
        let history = DagHistory::open_in_memory().await.unwrap();
        let manifest = DagManifest {
            nodes: vec![NodeEntry {
                id: "a".into(),
                kind: "echo".into(),
                spec: serde_json::json!({}),
            }],
            edges: vec![],
            ..Default::default()
        };
        let snapshot_id = history
            .commit("main", &manifest, None::<&RunReportStub>, "v1")
            .await
            .unwrap();

        history
            .record_run(&sample_run("111", true, Some(snapshot_id.clone())))
            .await
            .unwrap();
        // Second execution of the *same* manifest — no new snapshot, run row
        // still links to the existing head.
        history
            .record_run(&sample_run("222", false, Some(snapshot_id.clone())))
            .await
            .unwrap();

        let runs = history.list_runs(10, None).await.unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].id, "222", "newest first");
        assert_eq!(runs[1].id, "111");
        assert!(
            runs.iter()
                .all(|run| run.snapshot_id.as_deref() == Some(snapshot_id.as_str()))
        );
        assert_eq!(runs[0].trigger.as_deref(), Some("agent:/root/researcher"));
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
        let manifest = DagManifest {
            nodes: vec![],
            edges: vec![],
            ..Default::default()
        };
        let _ = history
            .commit("main", &manifest, None::<&RunReportStub>, "v1")
            .await
            .unwrap();
        let _ = history
            .commit("other", &manifest, None::<&RunReportStub>, "v1")
            .await
            .unwrap();

        let mut main_run = sample_run("111", true, None);
        main_run.ref_name = "main".to_string();
        let mut other_run = sample_run("222", true, None);
        other_run.ref_name = "other".to_string();
        history.record_run(&main_run).await.unwrap();
        history.record_run(&other_run).await.unwrap();

        let main_runs = history.list_runs(10, Some("main")).await.unwrap();
        assert_eq!(main_runs.len(), 1);
        assert_eq!(main_runs[0].id, "111");

        let all = history.list_runs(10, None).await.unwrap();
        assert_eq!(all.len(), 2);
    }
}
