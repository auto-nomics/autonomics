//! Workflow persistence — CRUD on `workflows` + git-style `workflow_snapshots`.
//!
//! Snapshots are append-only and content-addressed by `blake3(manifest_json)`.
//! Two identical saves to the same workflow produce two snapshots pointing at
//! the same hash, which lets the history UI collapse "no-op" saves.

use crate::error::{Result, StorageError};
use crate::model::{SnapshotInfo, WorkflowManifest};
use crate::store::pool::DbPool;
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use uuid::Uuid;

/// Workflow repository — wraps a [`DbPool`]. Cheap to clone.
#[derive(Clone)]
pub struct WorkflowRepo {
    pool: DbPool,
}

impl WorkflowRepo {
    /// Wrap an existing pool.
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Borrow the underlying pool.
    pub fn pool(&self) -> DbPool {
        self.pool.clone()
    }

    /// Create a brand new workflow with the given manifest. Returns the id.
    pub fn create(&self, manifest: &WorkflowManifest) -> Result<Uuid> {
        let json = serde_json::to_string(manifest)?;
        let hash = manifest.content_hash().to_hex().to_string();
        let now = Utc::now().to_rfc3339();
        let id = manifest.id;

        let mut conn = self.pool.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO workflows (id, name, manifest_json, manifest_hash, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![id.to_string(), manifest.name, json, hash, now],
        )?;
        tx.execute(
            "INSERT INTO workflow_snapshots
                (id, workflow_id, parent_id, manifest_hash, manifest_json, commit_message, created_at)
             VALUES (?1, ?2, NULL, ?3, ?4, 'initial', ?5)",
            params![Uuid::new_v4().to_string(), id.to_string(), hash, json, now],
        )?;
        tx.commit()?;
        Ok(id)
    }

    /// Fetch a workflow's current manifest.
    pub fn get(&self, id: Uuid) -> Result<WorkflowManifest> {
        let conn = self.pool.lock();
        let json: String = conn
            .query_row(
                "SELECT manifest_json FROM workflows WHERE id = ?1",
                params![id.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound {
                kind: "workflow",
                id: id.to_string(),
            })?;
        Ok(serde_json::from_str(&json)?)
    }

    /// Save a new snapshot. Updates `workflows.manifest_*` and appends a
    /// `workflow_snapshots` row whose `parent_id` points at the previous
    /// latest snapshot.
    pub fn save(
        &self,
        id: Uuid,
        manifest: &WorkflowManifest,
        message: impl Into<String>,
    ) -> Result<Uuid> {
        manifest.validate()?;
        let json = serde_json::to_string(manifest)?;
        let hash = manifest.content_hash().to_hex().to_string();
        let message = message.into();
        let now = Utc::now().to_rfc3339();
        let snapshot_id = Uuid::new_v4();

        let mut conn = self.pool.lock();
        let tx = conn.transaction()?;

        let prev: Option<String> = tx
            .query_row(
                "SELECT id FROM workflow_snapshots
                 WHERE workflow_id = ?1
                 ORDER BY created_at DESC LIMIT 1",
                params![id.to_string()],
                |row| row.get(0),
            )
            .optional()?;

        tx.execute(
            "UPDATE workflows
                SET name = ?1, manifest_json = ?2, manifest_hash = ?3, updated_at = ?4
              WHERE id = ?5",
            params![manifest.name, json, hash, now, id.to_string()],
        )?;
        tx.execute(
            "INSERT INTO workflow_snapshots
                (id, workflow_id, parent_id, manifest_hash, manifest_json, commit_message, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                snapshot_id.to_string(),
                id.to_string(),
                prev,
                hash,
                json,
                message,
                now
            ],
        )?;
        tx.commit()?;
        Ok(snapshot_id)
    }

    /// Delete a workflow and (cascading) all its snapshots.
    pub fn delete(&self, id: Uuid) -> Result<()> {
        let conn = self.pool.lock();
        let n = conn.execute(
            "DELETE FROM workflows WHERE id = ?1",
            params![id.to_string()],
        )?;
        if n == 0 {
            return Err(StorageError::NotFound {
                kind: "workflow",
                id: id.to_string(),
            }
            .into());
        }
        Ok(())
    }

    /// List all workflows, newest first.
    pub fn list(&self) -> Result<Vec<WorkflowSummary>> {
        let conn = self.pool.lock();
        let mut stmt = conn.prepare(
            "SELECT id, name, manifest_hash, updated_at
             FROM workflows ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let name: String = row.get(1)?;
            let hash: String = row.get(2)?;
            let updated: String = row.get(3)?;
            Ok((id, name, hash, updated))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (id, name, hash, updated) = r?;
            let updated = DateTime::parse_from_rfc3339(&updated)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            out.push(WorkflowSummary {
                id: Uuid::parse_str(&id).map_err(|e| StorageError::Decode(e.to_string()))?,
                name,
                manifest_hash: hash,
                updated_at: updated,
            });
        }
        Ok(out)
    }

    /// History (snapshots) for a workflow, newest first.
    pub fn history(&self, workflow_id: Uuid) -> Result<Vec<SnapshotInfo>> {
        let conn = self.pool.lock();
        let mut stmt = conn.prepare(
            "SELECT id, workflow_id, parent_id, manifest_hash, commit_message, created_at
             FROM workflow_snapshots
             WHERE workflow_id = ?1
             ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![workflow_id.to_string()], |row| {
            let id: String = row.get(0)?;
            let wf: String = row.get(1)?;
            let parent: Option<String> = row.get(2)?;
            let hash: String = row.get(3)?;
            let msg: String = row.get(4)?;
            let created: String = row.get(5)?;
            Ok((id, wf, parent, hash, msg, created))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (id, wf, parent, hash, msg, created) = r?;
            let created = DateTime::parse_from_rfc3339(&created)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            out.push(SnapshotInfo {
                id: Uuid::parse_str(&id).map_err(|e| StorageError::Decode(e.to_string()))?,
                workflow_id: Uuid::parse_str(&wf)
                    .map_err(|e| StorageError::Decode(e.to_string()))?,
                parent_id: parent
                    .map(|s| Uuid::parse_str(&s))
                    .transpose()
                    .map_err(|e| StorageError::Decode(e.to_string()))?,
                manifest_hash: hash,
                commit_message: msg,
                created_at: created,
            });
        }
        Ok(out)
    }

    /// Restore the workflow's *current* state to the snapshot's manifest.
    pub fn checkout(&self, workflow_id: Uuid, snapshot_id: Uuid) -> Result<()> {
        let conn = self.pool.lock();
        let json: String = conn
            .query_row(
                "SELECT manifest_json FROM workflow_snapshots WHERE id = ?1",
                params![snapshot_id.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound {
                kind: "snapshot",
                id: snapshot_id.to_string(),
            })?;
        let m: WorkflowManifest = serde_json::from_str(&json)?;
        drop(conn);
        let _ = self.save(workflow_id, &m, "checkout")?;
        Ok(())
    }
}

/// Lightweight summary returned by [`WorkflowRepo::list`].
#[derive(Debug, Clone)]
pub struct WorkflowSummary {
    /// Workflow id.
    pub id: Uuid,
    /// Human-readable name.
    pub name: String,
    /// blake3 hash of the current manifest.
    pub manifest_hash: String,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
}
