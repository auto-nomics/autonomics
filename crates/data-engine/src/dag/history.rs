//! Git-like version control for DAG snapshots, backed by a local Turso
//! (SQLite-compatible) database.
//!
//! Each [`DagHistory`] owns a [`turso::Connection`] to a local database file.
//! Snapshots are stored as rows in the `snapshots` table; named refs (branches)
//! track lineage heads. The connection is opened once at engine-init time and
//! lives for the engine's lifetime — callers never touch raw SQL.
//!
//! # Schema
//!
//! ```sql
//! CREATE TABLE snapshots (
//!     id              TEXT PRIMARY KEY,   -- blake3(parent_id || manifest_hash || ts)
//!     parent_id       TEXT,               -- predecessor snapshot (NULL = root)
//!     manifest_hash   TEXT NOT NULL,      -- blake3 of canonical manifest JSON
//!     manifest_json   TEXT NOT NULL,      -- full DAG blueprint (nodes + edges + specs)
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
//! ```

use std::path::Path;

use blake3::Hasher;
use serde::{Deserialize, Serialize};
use turso::{Value, params_from_iter};

use super::NodeId;
use super::error::DagError;

// ── content-addressable data model ───────────────────────────────────────────

/// A pure-data, fully serializable description of a DAG's topology + node
/// configuration — sufficient to reconstruct the DAG from a [`crate::node_registry::NodeRegistry`].
///
/// This is the "tree" object (to borrow git terminology): its content hash
/// identifies a unique DAG configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagManifest {
    pub nodes: Vec<NodeEntry>,
    pub edges: Vec<EdgeEntry>,
}

/// One node's identity + configuration in a manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeEntry {
    pub id: NodeId,
    /// Factory kind string (e.g. `"source_file"`, `"sql"`), matching
    /// [`crate::node_registry::registry::NodeFactory::kind`].
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
    /// Compute the blake3 content hash of this manifest's canonical JSON form.
    ///
    /// Two manifests with the same nodes + edges (in the same order) always
    /// produce the same hash — enabling dedup at the storage layer.
    pub fn content_hash(&self) -> String {
        // serde_json with sorted keys gives a canonical byte sequence.
        let json = serde_json::to_vec(self).unwrap_or_default();
        let mut hasher = Hasher::new();
        hasher.update(&json);
        hasher.finalize().to_hex().to_string()
    }

    /// Build a manifest from raw DAG state (used by [`super::graph::DAG::to_manifest`]).
    pub fn from_parts(nodes: Vec<NodeEntry>, edges: Vec<EdgeEntry>) -> Self {
        DagManifest { nodes, edges }
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
        let path_str = path
            .as_ref()
            .to_str()
            .ok_or_else(|| DagError::History("history db path is not valid UTF-8".into()))?;

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
        let engine_version = env!("CARGO_PKG_VERSION").to_string();

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

#[cfg(test)]
mod tests {
    use super::*;

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
                kind: "source_file".into(),
                spec: serde_json::json!({"path": "/tmp/x.csv"}),
            }],
            edges: vec![],
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
        };
        let h1 = m.content_hash();
        let h2 = m.content_hash();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // blake3 hex = 32 bytes = 64 chars
    }

    /// A stub type implementing Serialize to stand in for RunReport in tests.
    #[derive(Serialize)]
    struct RunReportStub {
        ok: bool,
    }
}
