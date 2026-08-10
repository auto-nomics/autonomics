//! Persistence for the resource catalog's registered manifest.

use std::path::Path;

use async_trait::async_trait;
use turso::{params_from_iter, Connection, Value};

use crate::entry::ResourceEntry;
use crate::error::{ResourceError, Result};
use crate::kind::ResourceKind;

/// A backend that stores the registered manifest (never a live scan).
#[async_trait]
pub trait ManifestStore: Send + Sync {
    /// Persist the given entries.
    async fn save(&self, entries: &[ResourceEntry]) -> Result<()>;
    /// Load all persisted entries.
    async fn load(&self) -> Result<Vec<ResourceEntry>>;
}

/// SQLite/Turso-backed manifest store, mirroring the `DagHistory` pattern in
/// `dag-core`. Kept offline-first so bootstrap never depends on the Iceberg
/// REST catalog being reachable.
pub struct TursoManifestStore {
    conn: Connection,
}

impl TursoManifestStore {
    /// Open (or create) the manifest database at `path`.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path_str = path
            .as_ref()
            .to_str()
            .ok_or_else(|| ResourceError::Persistence("manifest db path is not UTF-8".into()))?;

        let db = turso::Builder::new_local(path_str)
            .build()
            .await
            .map_err(|e| ResourceError::Persistence(format!("open manifest db: {e}")))?;
        let conn = db
            .connect()
            .map_err(|e| ResourceError::Persistence(format!("connect manifest db: {e}")))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS resource_manifest (
                name          TEXT PRIMARY KEY,
                kind          TEXT NOT NULL,
                description   TEXT NOT NULL,
                address_json  TEXT NOT NULL,
                metadata_json TEXT NOT NULL,
                tags_json     TEXT NOT NULL
            );",
        )
        .await
        .map_err(|e| ResourceError::Persistence(format!("manifest schema init: {e}")))?;

        Ok(Self { conn })
    }

    /// An in-memory store (for tests / ephemeral sessions).
    pub async fn open_in_memory() -> Result<Self> {
        Self::open(":memory:").await
    }
}

#[async_trait]
impl ManifestStore for TursoManifestStore {
    async fn save(&self, entries: &[ResourceEntry]) -> Result<()> {
        for entry in entries {
            let address_json = serde_json::to_string(&entry.address)?;
            let metadata_json = serde_json::to_string(&entry.metadata)?;
            let tags_json = serde_json::to_string(&entry.tags)?;
            self.conn
                .execute(
                    "INSERT INTO resource_manifest
                        (name, kind, description, address_json, metadata_json, tags_json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(name) DO UPDATE SET
                        kind = excluded.kind,
                        description = excluded.description,
                        address_json = excluded.address_json,
                        metadata_json = excluded.metadata_json,
                        tags_json = excluded.tags_json",
                    params_from_iter([
                        Value::Text(entry.name.clone()),
                        Value::Text(entry.kind.as_str().into()),
                        Value::Text(entry.description.clone()),
                        Value::Text(address_json),
                        Value::Text(metadata_json),
                        Value::Text(tags_json),
                    ]),
                )
                .await
                .map_err(|e| ResourceError::Persistence(format!("save entry: {e}")))?;
        }
        Ok(())
    }

    async fn load(&self) -> Result<Vec<ResourceEntry>> {
        let mut rows = self
            .conn
            .query(
                "SELECT name, kind, description, address_json, metadata_json, tags_json
                 FROM resource_manifest ORDER BY name",
                (),
            )
            .await
            .map_err(|e| ResourceError::Persistence(format!("manifest load query: {e}")))?;

        let mut out = Vec::new();
        loop {
            match rows.next().await {
                Ok(Some(row)) => {
                    let name = text_value(&row, 0)?;
                    let kind_str = text_value(&row, 1)?;
                    let description = text_value(&row, 2)?;
                    let address_json = text_value(&row, 3)?;
                    let metadata_json = text_value(&row, 4)?;
                    let tags_json = text_value(&row, 5)?;

                    let kind = ResourceKind::from_str(&kind_str).ok_or_else(|| {
                        ResourceError::Persistence(format!("unknown resource kind: {kind_str}"))
                    })?;
                    let address = serde_json::from_str(&address_json)?;
                    let metadata = serde_json::from_str(&metadata_json)?;
                    let tags = serde_json::from_str(&tags_json)?;

                    out.push(ResourceEntry {
                        name,
                        kind,
                        description,
                        address,
                        metadata,
                        tags,
                    });
                }
                Ok(None) => break,
                Err(e) => {
                    return Err(ResourceError::Persistence(format!(
                        "manifest load row: {e}"
                    )))
                }
            }
        }
        Ok(out)
    }
}

/// Extract a TEXT column as `String`.
fn text_value(row: &turso::Row, idx: usize) -> Result<String> {
    match row.get_value(idx).map_err(|e| {
        ResourceError::Persistence(format!("manifest column read: {e}"))
    })? {
        Value::Text(s) => Ok(s),
        Value::Null => Ok(String::new()),
        other => Err(ResourceError::Persistence(format!(
            "expected TEXT at column {idx}, got {other:?}"
        ))),
    }
}
