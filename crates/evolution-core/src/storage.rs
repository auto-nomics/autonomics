//! The storage contract observation consumers depend on, plus the
//! Turso-backed implementation.
//!
//! Observations live in `<state_dir>/evolution.db`: idempotent
//! content-addressed records, stable id ordering, and a vector
//! side-table keyed by model signature. (The pre-database layout —
//! one TOML file per record under `<state_dir>/skill-observations/`
//! — is retired; its one-shot import path was removed with it.)

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{Mutex, OnceCell};
use turso::Value;

use crate::cluster::Cluster;
use crate::error::{Error, Result};
use crate::observation::{NODE_KIND_RENAMES, Observation, ObservationInput};

/// Database file name under the state dir.
pub const EVOLUTION_DB: &str = "evolution.db";

/// Durable observation storage: records plus per-signature vectors.
///
/// Vectors are cached embeddings, not a second source of truth — a
/// read is a hit only when the stored signature matches the caller's
/// current embedder, so swapping models invalidates the whole cache
/// without migration.
#[async_trait]
pub trait ObservationStorage: Send + Sync {
    /// Insert one observation. Idempotent on content: an existing id
    /// returns the stored record unchanged.
    async fn record(&self, input: ObservationInput) -> Result<Observation>;

    /// Every stored observation, ordered by id (stable across runs).
    async fn list(&self) -> Vec<Observation>;

    /// The cached vector for one observation under one model
    /// signature, if present.
    async fn load_embedding(&self, id: &str, signature: &str) -> Result<Option<Vec<f32>>>;

    /// Cache a vector for one observation. Overwrites any vector
    /// previously stored for that observation (one embedding slot per
    /// record — the active embedder owns it).
    async fn save_embedding(&self, id: &str, signature: &str, vector: &[f32]) -> Result<()>;

    async fn load_clusters(&self) -> Result<Vec<Cluster>>;

    async fn update_cluster(&self, cluster: Cluster) -> Result<()>;

    /// Remove one persisted cluster — the consolidation step drops
    /// clusters merged into a sibling.
    async fn delete_cluster(&self, id: &str) -> Result<()>;

    /// Drop every persisted cluster. Called when the stored
    /// clusters' model signature no longer matches the active
    /// embedder — the set is rebuilt from scratch on the next pass.
    async fn clear_clusters(&self) -> Result<()>;
}

/// Turso-backed production storage over `<state_dir>/evolution.db`.
///
/// Construction is synchronous — the database is built lazily on
/// first use, because `turso::Builder::build` is async and callers
/// construct stores on synchronous paths (manager constructors).
pub struct TursoObservationStorage {
    db_path: PathBuf,
    conn: OnceCell<Arc<Mutex<turso::Connection>>>,
}

impl TursoObservationStorage {
    /// Storage over `<state_dir>/evolution.db`. No I/O happens here.
    pub fn new(state_dir: impl Into<PathBuf>) -> Self {
        Self {
            db_path: state_dir.into().join(EVOLUTION_DB),
            conn: OnceCell::new(),
        }
    }

    /// Build the database (once) and create the schema.
    async fn ensure(&self) -> Result<Arc<Mutex<turso::Connection>>> {
        self.conn
            .get_or_try_init(|| async {
                if let Some(parent) = self.db_path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| Error::Storage(format!("create {}: {e}", parent.display())))?;
                }
                let path = self.db_path.to_string_lossy().to_string();
                let db = turso::Builder::new_local(&path)
                    .experimental_multiprocess_wal(true)
                    .build()
                    .await
                    .map_err(|e| Error::Storage(format!("open {}: {e}", self.db_path.display())))?;
                let conn = db
                    .connect()
                    .map_err(|e| Error::Storage(format!("connect evolution db: {e}")))?;
                let conn = Arc::new(Mutex::new(conn));
                Self::init_schema(&conn).await?;
                Ok(conn)
            })
            .await
            .map(Arc::clone)
    }

    async fn init_schema(conn: &Arc<Mutex<turso::Connection>>) -> Result<()> {
        conn.lock()
            .await
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS observation (
                    id TEXT PRIMARY KEY,
                    created_at INTEGER NOT NULL,
                    kind TEXT NOT NULL,
                    source TEXT NOT NULL,
                    summary TEXT NOT NULL,
                    body TEXT NOT NULL,
                    node_kind TEXT,
                    error TEXT
                );
                CREATE TABLE IF NOT EXISTS observation_embedding (
                    observation_id TEXT PRIMARY KEY
                        REFERENCES observation(id) ON DELETE CASCADE,
                    model_signature TEXT NOT NULL,
                    dim INTEGER NOT NULL,
                    vector BLOB NOT NULL
                );
                CREATE TABLE IF NOT EXISTS cluster_centroids (
                    id TEXT PRIMARY KEY,
                    model_signature TEXT NOT NULL,
                    partition_key TEXT NOT NULL,
                    dim INTEGER NOT NULL,
                    vector BLOB NOT NULL,
                    members TEXT NOT NULL
                )
                ",
            )
            .await
            .map_err(|e| Error::Storage(format!("init evolution schema: {e}")))
    }

    /// Apply [`NODE_KIND_RENAMES`] in place and persist the rewrite,
    /// so observations anchored on retired node kinds rejoin
    /// distillation on read.
    async fn rewrite_node_kind(&self, observation: &mut Observation, conn: &mut turso::Connection) {
        for (from, to) in NODE_KIND_RENAMES {
            if observation.node_kind.as_deref() == Some(*from) {
                observation.node_kind = Some((*to).to_string());
                let _ = conn
                    .execute(
                        "UPDATE observation SET node_kind = ?1 WHERE id = ?2",
                        turso::params![*to, observation.id.clone()],
                    )
                    .await;
            }
        }
    }
}

#[async_trait]
impl ObservationStorage for TursoObservationStorage {
    async fn record(&self, input: ObservationInput) -> Result<Observation> {
        let conn = self.ensure().await?;
        let observation = Observation::from_input(input, crate::observation::unix_now());
        let mut guard = conn.lock().await;
        // Idempotent on content: an existing row wins and is returned
        // unchanged (its created_at is the original recording time).
        insert_observation(&mut guard, &observation)
            .await
            .map_err(|e| Error::Storage(format!("record observation: {e}")))?;
        let stored = query_observation(&mut guard, &observation.id)
            .await
            .map_err(|e| Error::Storage(format!("load observation: {e}")))?;
        Ok(stored.unwrap_or(observation))
    }

    async fn list(&self) -> Vec<Observation> {
        let conn = match self.ensure().await {
            Ok(conn) => conn,
            Err(error) => {
                tracing::warn!(%error, "cannot open observation database");
                return Vec::new();
            }
        };
        let mut guard = conn.lock().await;
        let rows = match guard
            .query(
                "SELECT id, created_at, kind, source, summary, body, node_kind, error
                 FROM observation ORDER BY id",
                (),
            )
            .await
        {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!(%error, "cannot list observations");
                return Vec::new();
            }
        };
        let mut out = Vec::new();
        let mut rows = rows;
        while let Ok(Some(row)) = rows.next().await {
            let Ok(mut observation) = observation_from_row(&row) else {
                tracing::warn!("skipping malformed observation row");
                continue;
            };
            self.rewrite_node_kind(&mut observation, &mut guard).await;
            out.push(observation);
        }
        out
    }

    async fn load_embedding(&self, id: &str, signature: &str) -> Result<Option<Vec<f32>>> {
        let conn = self.ensure().await?;
        let guard = conn.lock().await;
        let mut rows = guard
            .query(
                "SELECT dim, vector FROM observation_embedding
                 WHERE observation_id = ?1 AND model_signature = ?2",
                turso::params![id, signature],
            )
            .await
            .map_err(|e| Error::Storage(format!("load embedding: {e}")))?;
        let Some(row) = rows
            .next()
            .await
            .map_err(|e| Error::Storage(format!("load embedding: {e}")))?
        else {
            return Ok(None);
        };
        let (dim, blob) = match (row.get_value(0), row.get_value(1)) {
            (Ok(Value::Integer(dim)), Ok(Value::Blob(blob))) => (dim as usize, blob),
            _ => return Err(Error::Storage("malformed embedding row".into())),
        };
        if blob.len() != dim * 4 {
            return Err(Error::Storage(format!(
                "corrupt embedding for {id:?}: {} bytes for dim {dim}",
                blob.len()
            )));
        }
        Ok(Some(
            blob.chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
        ))
    }

    async fn save_embedding(&self, id: &str, signature: &str, vector: &[f32]) -> Result<()> {
        let conn = self.ensure().await?;
        let bytes: Vec<u8> = vector.iter().flat_map(|v| v.to_le_bytes()).collect();
        conn.lock()
            .await
            .execute(
                "INSERT INTO observation_embedding
                    (observation_id, model_signature, dim, vector)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(observation_id) DO UPDATE SET
                    model_signature = excluded.model_signature,
                    dim = excluded.dim,
                    vector = excluded.vector",
                turso::params![id, signature, vector.len() as i64, bytes],
            )
            .await
            .map_err(|e| Error::Storage(format!("save embedding: {e}")))?;
        Ok(())
    }

    async fn load_clusters(&self) -> Result<Vec<Cluster>> {
        let conn = self.ensure().await?;
        let guard = conn.lock().await;
        let mut rows = guard
            .query(
                "SELECT id, model_signature, partition_key, dim, vector, members
                 FROM cluster_centroids",
                (),
            )
            .await
            .map_err(|e| Error::Storage(format!("load clusters: {e}")))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| Error::Storage(format!("load clusters: {e}")))?
        {
            let text = |idx: usize| -> Result<String> {
                match row.get_value(idx) {
                    Ok(Value::Text(value)) => Ok(value),
                    _ => Err(Error::Storage("malformed cluster row".into())),
                }
            };
            let dim = match row.get_value(3) {
                Ok(Value::Integer(dim)) => dim as usize,
                _ => return Err(Error::Storage("malformed cluster row".into())),
            };
            let blob = match row.get_value(4) {
                Ok(Value::Blob(blob)) => blob,
                _ => return Err(Error::Storage("malformed cluster row".into())),
            };
            if blob.len() != dim * 4 {
                return Err(Error::Storage(format!(
                    "corrupt cluster vector: {} bytes for dim {dim}",
                    blob.len()
                )));
            }
            let members = text(5)?
                .split('\0')
                .filter(|id| !id.is_empty())
                .map(str::to_string)
                .collect::<BTreeSet<_>>();
            out.push(Cluster {
                id: text(0)?,
                model_signature: text(1)?,
                partition_key: text(2)?,
                vector: blob
                    .chunks_exact(4)
                    .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                    .collect(),
                members,
            });
        }
        Ok(out)
    }

    async fn update_cluster(&self, cluster: Cluster) -> Result<()> {
        let conn = self.ensure().await?;
        let bytes: Vec<u8> = cluster
            .vector
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let members = cluster
            .members
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\0");
        conn.lock()
            .await
            .execute(
                "INSERT INTO cluster_centroids
                    (id, model_signature, partition_key, dim, vector, members)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO UPDATE SET
                    model_signature = excluded.model_signature,
                    partition_key = excluded.partition_key,
                    dim = excluded.dim,
                    vector = excluded.vector,
                    members = excluded.members",
                turso::params![
                    cluster.id,
                    cluster.model_signature,
                    cluster.partition_key,
                    cluster.vector.len() as i64,
                    bytes,
                    members,
                ],
            )
            .await
            .map_err(|e| Error::Storage(format!("save cluster: {e}")))?;
        Ok(())
    }

    async fn delete_cluster(&self, id: &str) -> Result<()> {
        let conn = self.ensure().await?;
        conn.lock()
            .await
            .execute(
                "DELETE FROM cluster_centroids WHERE id = ?1",
                turso::params![id],
            )
            .await
            .map_err(|e| Error::Storage(format!("delete cluster: {e}")))?;
        Ok(())
    }

    async fn clear_clusters(&self) -> Result<()> {
        let conn = self.ensure().await?;
        conn.lock()
            .await
            .execute("DELETE FROM cluster_centroids", ())
            .await
            .map_err(|e| Error::Storage(format!("clear clusters: {e}")))?;
        Ok(())
    }
}

/// INSERT OR IGNORE — returns whether a row was inserted (false means
/// the content-addressed id already existed).
async fn insert_observation(
    conn: &mut turso::Connection,
    observation: &Observation,
) -> std::result::Result<bool, turso::Error> {
    let changed = conn
        .execute(
            "INSERT OR IGNORE INTO observation
                (id, created_at, kind, source, summary, body, node_kind, error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            turso::params![
                observation.id.clone(),
                observation.created_at,
                kind_column(observation),
                source_column(observation),
                observation.summary.clone(),
                observation.body.clone(),
                observation.node_kind.clone(),
                observation.error.clone(),
            ],
        )
        .await?;
    Ok(changed > 0)
}

async fn query_observation(
    conn: &mut turso::Connection,
    id: &str,
) -> std::result::Result<Option<Observation>, turso::Error> {
    let mut rows = conn
        .query(
            "SELECT id, created_at, kind, source, summary, body, node_kind, error
             FROM observation WHERE id = ?1",
            turso::params![id],
        )
        .await?;
    match rows.next().await? {
        Some(row) => observation_from_row(&row)
            .map_err(|reason| turso::Error::Error(format!("malformed observation row: {reason}")))
            .map(Some),
        None => Ok(None),
    }
}

fn observation_from_row(row: &turso::Row) -> std::result::Result<Observation, String> {
    let text = |idx: usize, field: &str| -> std::result::Result<String, String> {
        match row.get_value(idx) {
            Ok(Value::Text(value)) => Ok(value),
            _ => Err(format!("missing {field}")),
        }
    };
    let opt_text = |idx: usize| -> Option<String> {
        match row.get_value(idx) {
            Ok(Value::Text(value)) => Some(value),
            _ => None,
        }
    };
    let created_at = match row.get_value(1) {
        Ok(Value::Integer(value)) => value,
        _ => return Err("missing created_at".into()),
    };
    Ok(Observation {
        id: text(0, "id")?,
        created_at,
        kind: parse_kind(&text(2, "kind")?)?,
        source: parse_source(&text(3, "source")?)?,
        summary: text(4, "summary")?,
        body: text(5, "body")?,
        node_kind: opt_text(6),
        error: opt_text(7),
    })
}

fn kind_column(observation: &Observation) -> String {
    observation.kind.label().to_string()
}

fn source_column(observation: &Observation) -> String {
    match observation.source {
        crate::observation::ObservationSource::Agent => "agent".into(),
        crate::observation::ObservationSource::Eval => "eval".into(),
        crate::observation::ObservationSource::Cli => "cli".into(),
    }
}

fn parse_kind(label: &str) -> std::result::Result<crate::observation::ObservationKind, String> {
    match label {
        "failure" => Ok(crate::observation::ObservationKind::Failure),
        "recipe" => Ok(crate::observation::ObservationKind::Recipe),
        "caveat" => Ok(crate::observation::ObservationKind::Caveat),
        other => Err(format!("unknown kind {other:?}")),
    }
}

fn parse_source(label: &str) -> std::result::Result<crate::observation::ObservationSource, String> {
    match label {
        "agent" => Ok(crate::observation::ObservationSource::Agent),
        "eval" => Ok(crate::observation::ObservationSource::Eval),
        // Retired producers map to agent so old rows stay readable.
        "workflow_run" => Ok(crate::observation::ObservationSource::Agent),
        "cli" => Ok(crate::observation::ObservationSource::Cli),
        other => Err(format!("unknown source {other:?}")),
    }
}

/// Load many cached vectors in one pass — distillation warms its
/// cache view with this before clustering.
pub async fn load_embedding_map(
    storage: &dyn ObservationStorage,
    ids: &[String],
    signature: &str,
) -> Result<HashMap<String, Vec<f32>>> {
    let mut out = HashMap::with_capacity(ids.len());
    for id in ids {
        if let Some(vector) = storage.load_embedding(id, signature).await? {
            out.insert(id.clone(), vector);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observation::{ObservationKind, ObservationSource};

    fn input(summary: &str) -> ObservationInput {
        ObservationInput {
            kind: ObservationKind::Failure,
            source: ObservationSource::Agent,
            summary: summary.into(),
            body: "body".into(),
            node_kind: Some("sql".into()),
            error: Some("boom".into()),
        }
    }

    async fn temp_storage() -> (tempfile::TempDir, TursoObservationStorage) {
        let dir = tempfile::tempdir().unwrap();
        let storage = TursoObservationStorage::new(dir.path());
        (dir, storage)
    }

    #[tokio::test]
    async fn record_is_idempotent_and_stable_across_reopens() {
        let (dir, storage) = temp_storage().await;
        let first = storage.record(input("fix for boom")).await.unwrap();
        // Same content → same id, original created_at preserved.
        let second = storage.record(input("fix for boom")).await.unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(first.created_at, second.created_at);
        // Fresh storage handle over the same file sees the row.
        let reopened = TursoObservationStorage::new(dir.path());
        let listed = reopened.list().await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, first.id);
        assert_eq!(listed[0].node_kind.as_deref(), Some("sql"));
    }

    #[tokio::test]
    async fn list_orders_by_id() {
        let (_dir, storage) = temp_storage().await;
        storage.record(input("zzz")).await.unwrap();
        storage.record(input("aaa")).await.unwrap();
        let listed = storage.list().await;
        let ids: Vec<&str> = listed.iter().map(|o| o.id.as_str()).collect();
        assert!(ids.windows(2).all(|w| w[0] <= w[1]));
    }

    #[tokio::test]
    async fn embeddings_round_trip_and_signature_scopes() {
        let (_dir, storage) = temp_storage().await;
        let observation = storage.record(input("cached")).await.unwrap();
        assert!(
            storage
                .load_embedding(&observation.id, "hashing-256")
                .await
                .unwrap()
                .is_none()
        );
        storage
            .save_embedding(&observation.id, "hashing-256", &[0.25, -1.5, 3.0])
            .await
            .unwrap();
        // Different signature: miss — a model swap invalidates.
        assert!(
            storage
                .load_embedding(&observation.id, "onnx-other")
                .await
                .unwrap()
                .is_none()
        );
        let loaded = storage
            .load_embedding(&observation.id, "hashing-256")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded, vec![0.25, -1.5, 3.0]);
        // Overwrite under a new signature replaces the slot.
        storage
            .save_embedding(&observation.id, "onnx-other", &[1.0])
            .await
            .unwrap();
        assert_eq!(
            storage
                .load_embedding(&observation.id, "onnx-other")
                .await
                .unwrap()
                .unwrap(),
            vec![1.0]
        );
    }
}
