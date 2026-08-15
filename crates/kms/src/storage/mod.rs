//! Storage facade: owns the Turso connection and schema.
//!
//! Ported from dendrite's sqlx-backed `Storage`. The original held three
//! separate repo structs (`SqliteEntityRepo`, `SqliteKnowledgeRepo`,
//! `SqliteIndexRepo`) each cloning the pool. With Turso, we hold one shared
//! async connection lock so KMS can safely share `agent.db` with the agent
//! storage without triggering Turso's concurrent-use guard.

pub mod error;
pub mod repo;
pub mod types;

use std::path::Path;
use std::sync::Arc;

use tokio::sync::Mutex;

#[derive(Clone)]
pub struct Storage {
    conn: Arc<Mutex<turso::Connection>>,
}

impl Storage {
    /// Open (or create) a Turso database at `db_path` and run the KMS
    /// schema migration.
    pub async fn new(db_path: &str) -> Result<Self, String> {
        if let Some(parent) = Path::new(db_path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).ok();
            }
        }

        let db = turso::Builder::new_local(db_path)
            .build()
            .await
            .map_err(|e| e.to_string())?;

        let conn = db.connect().map_err(|e| e.to_string())?;

        conn.pragma_update("busy_timeout", 5000)
            .await
            .map_err(|e| e.to_string())?;

        Self::from_shared_connection(Arc::new(Mutex::new(conn))).await
    }

    /// Create an in-memory database (useful for tests).
    pub async fn open_in_memory() -> Result<Self, String> {
        let db = turso::Builder::new_local(":memory:")
            .build()
            .await
            .map_err(|e| e.to_string())?;

        let conn = db.connect().map_err(|e| e.to_string())?;
        Self::from_shared_connection(Arc::new(Mutex::new(conn))).await
    }

    /// Share an already-open Turso connection and initialize the KMS schema.
    pub async fn from_shared_connection(
        conn: Arc<Mutex<turso::Connection>>,
    ) -> Result<Self, String> {
        let storage = Self { conn };
        storage.init_schema().await.map_err(|e| e.to_string())?;
        Ok(storage)
    }

    async fn init_schema(&self) -> Result<(), turso::Error> {
        let conn = self.conn.lock().await;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS entities (
                    id TEXT PRIMARY KEY NOT NULL,
                    definition TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS nomenclatures (
                    id TEXT PRIMARY KEY NOT NULL,
                    entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
                    lang TEXT NOT NULL,
                    full TEXT NOT NULL,
                    abbr TEXT,
                    UNIQUE(lang, full)
                );

                CREATE TABLE IF NOT EXISTS knowledges (
                    id TEXT PRIMARY KEY NOT NULL,
                    title TEXT NOT NULL,
                    knowledge_type TEXT NOT NULL,
                    entities TEXT NOT NULL,
                    content TEXT,
                    source_document_id TEXT,
                    source_chunk_idx INTEGER,
                    UNIQUE(title)
                );

                CREATE TABLE IF NOT EXISTS indexes (
                    id TEXT PRIMARY KEY NOT NULL,
                    title TEXT,
                    target TEXT,
                    target_type TEXT NOT NULL DEFAULT 'group',
                    parent_id TEXT,
                    position INTEGER NOT NULL
                );

                CREATE TRIGGER IF NOT EXISTS indexes_cascade_delete
                AFTER DELETE ON indexes
                FOR EACH ROW
                BEGIN
                    DELETE FROM indexes WHERE parent_id = OLD.id;
                END;",
        )
        .await?;
        Ok(())
    }

    /// Lock the underlying Turso connection. Used by repo free functions and
    /// the diagnostics runner.
    pub async fn conn(&self) -> tokio::sync::MutexGuard<'_, turso::Connection> {
        self.conn.lock().await
    }
}
