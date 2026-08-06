//! Storage facade: owns the Turso connection and schema.
//!
//! Ported from dendrite's sqlx-backed `Storage`. The original held three
//! separate repo structs (`SqliteEntityRepo`, `SqliteKnowledgeRepo`,
//! `SqliteIndexRepo`) each cloning the pool. With Turso, we hold a single
//! `turso::Connection` (which is `Clone` + `Arc`-backed internally) and
//! expose the repo functions as free functions in [`repo`]. The
//! `Storage` struct acts as a thin connection holder.

pub mod error;
pub mod repo;
pub mod types;

use std::path::Path;

#[derive(Clone)]
pub struct Storage {
    conn: turso::Connection,
}

impl Storage {
    /// Open (or create) a Turso database at `db_path` and run the KMS
    /// schema migration. Also creates the system root index if absent.
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

        let storage = Self { conn };
        storage.init_schema().await.map_err(|e| e.to_string())?;
        Ok(storage)
    }

    /// Create an in-memory database (useful for tests).
    pub async fn open_in_memory() -> Result<Self, String> {
        let db = turso::Builder::new_local(":memory:")
            .build()
            .await
            .map_err(|e| e.to_string())?;

        let conn = db.connect().map_err(|e| e.to_string())?;
        let storage = Self { conn };
        storage.init_schema().await.map_err(|e| e.to_string())?;
        Ok(storage)
    }

    async fn init_schema(&self) -> Result<(), turso::Error> {
        self.conn
            .execute_batch(
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

    /// Borrow the underlying Turso connection. Used by repo free functions
    /// and the diagnostics runner.
    pub fn conn(&self) -> &turso::Connection {
        &self.conn
    }
}
