//! Storage facade: owns the Turso connection and schema.
//!
//! Ported from dendrite's sqlx-backed `Storage`. The original held three
//! separate repo structs (`SqliteEntityRepo`, `SqliteKnowledgeRepo`,
//! `SqliteIndexRepo`) each cloning the pool. With Turso, we hold one shared
//! async connection lock. Production code opens a dedicated `knowledge.db`
//! file via [`Storage::new`]; the daemon and the KMS TUI open it
//! concurrently, so the file is opened in multiprocess-WAL mode.

pub mod error;
pub mod migration;
pub mod repo;
pub mod types;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Mutex;

#[derive(Clone)]
pub struct Storage {
    conn: Arc<Mutex<turso::Connection>>,
}

impl Storage {
    /// Open (or create) a Turso database at `db_path` and run the KMS
    /// schema migration.
    ///
    /// Non-`:memory:` databases are opened in multiprocess-WAL mode with a
    /// 5 s busy timeout so the daemon and the KMS TUI can hold the file at
    /// the same time (same pattern as `bib-base`). If the on-disk files are
    /// in a torn-WAL state (e.g. the TUI was SIGKILL'd mid-transaction and
    /// the WAL index points past EOF), the matching `-wal` / `-shm` /
    /// `-twal` / `-tshm` sidecars are quarantined and the open is retried
    /// once; the main `.db` file is never touched.
    pub async fn new(db_path: &str) -> Result<Self, String> {
        if db_path != ":memory:" {
            if let Some(parent) = Path::new(db_path).parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent).ok();
                }
            }
        }

        match Self::try_open_local(db_path).await {
            Ok(storage) => Ok(storage),
            Err(error) if db_path != ":memory:" && is_torn_wal_error(&error) => {
                tracing::warn!(
                    db = db_path,
                    error = %error,
                    "WAL torn on open — quarantining -wal/-shm sidecars and retrying once"
                );
                quarantine_wal_sidecars(Path::new(db_path));
                Self::try_open_local(db_path).await
            }
            Err(error) => Err(error),
        }
    }

    /// Internal: actually open the database without recovery. Split out so
    /// the recovery wrapper can call it twice.
    async fn try_open_local(db_path: &str) -> Result<Self, String> {
        let mut builder = turso::Builder::new_local(db_path);
        if db_path != ":memory:" {
            builder = builder.experimental_multiprocess_wal(true);
        }
        let db = builder.build().await.map_err(|e| e.to_string())?;
        let conn = db.connect().map_err(|e| e.to_string())?;

        if db_path != ":memory:" {
            conn.pragma_update("busy_timeout", 5000)
                .await
                .map_err(|e| e.to_string())?;
        }

        Self::from_shared_connection(Arc::new(Mutex::new(conn))).await
    }

    /// Create an in-memory database (useful for tests).
    pub async fn open_in_memory() -> Result<Self, String> {
        Self::new(":memory:").await
    }

    /// Share an already-open Turso connection and initialize the KMS schema.
    ///
    /// Production code opens a dedicated `knowledge.db` via [`Storage::new`];
    /// this constructor serves tests and the legacy-migration seeding path
    /// that needs the KMS schema on a caller-supplied connection.
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

                CREATE TABLE IF NOT EXISTS kms_meta (
                    key TEXT PRIMARY KEY NOT NULL,
                    value TEXT NOT NULL
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

// ───────────────────── torn-WAL recovery ─────────────────────
//
// Private copies of the helpers in
// `agentik-core/src/storage/turso_storage.rs` (kept private there). The kms
// crate deliberately does not depend on agentik-core, so the ~50 lines are
// duplicated here against a `String` error type instead of `StorageError`.

/// Real shape of the error observed in production:
/// "…I/O error: short read on WAL frame at offset 1751032: expected 4096
/// bytes, got 0". Disk-full / permission-denied must NOT match.
fn is_torn_wal_error(msg: &str) -> bool {
    msg.contains("short read on WAL frame")
}

/// Move each existing `-wal` / `-shm` / `-twal` / `-tshm` sidecar next to
/// `db_path` to `<sidecar>.corrupt-<unix-ts>` so a retry starts from the
/// last checkpointed main file instead of failing forever.
fn quarantine_wal_sidecars(db_path: &Path) {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    for suffix in ["-wal", "-shm", "-twal", "-tshm"] {
        let sidecar = append_suffix(db_path, suffix);
        if !sidecar.is_file() {
            continue;
        }
        let target = append_suffix(&sidecar, &format!(".corrupt-{ts}"));
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
/// turning `knowledge.db` into `knowledge-wal`, which is not what we want.)
fn append_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Std-only equivalent of `tempfile::tempdir()`: creates a uniquely
    /// named subdir under `std::env::temp_dir()`.
    fn fresh_tmpdir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "kms-storage-test-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn is_torn_wal_error_matches_known_phrase() {
        let msg = "open database: I/O error: short read on WAL frame at offset 1751032: expected 4096 bytes, got 0";
        assert!(is_torn_wal_error(msg));
    }

    #[test]
    fn is_torn_wal_error_rejects_unrelated_io() {
        for msg in [
            "open database: I/O error: disk full",
            "open database: permission denied",
            "open database: database is locked",
        ] {
            assert!(!is_torn_wal_error(msg), "false positive on: {msg}");
        }
    }

    #[test]
    fn quarantine_renames_present_sidecars_only() {
        let tmp = fresh_tmpdir("quarantine_renames_present_sidecars_only");
        let db = tmp.join("knowledge.db");
        std::fs::write(&db, b"main-db-bytes").unwrap();
        std::fs::write(append_suffix(&db, "-tshm"), b"shm").unwrap();
        std::fs::write(append_suffix(&db, "-wal"), b"wal").unwrap();

        quarantine_wal_sidecars(&db);

        assert!(db.exists());
        assert_eq!(std::fs::read(&db).unwrap(), b"main-db-bytes");
        assert!(!append_suffix(&db, "-tshm").exists());
        assert!(!append_suffix(&db, "-wal").exists());
        let mut found_tshm = false;
        let mut found_wal = false;
        for entry in std::fs::read_dir(&tmp).unwrap() {
            let name = entry.unwrap().file_name();
            let s = name.to_string_lossy();
            if s.starts_with("knowledge.db-tshm.corrupt-") {
                found_tshm = true;
            }
            if s.starts_with("knowledge.db-wal.corrupt-") {
                found_wal = true;
            }
        }
        assert!(found_tshm, "expected -tshm.corrupt-* quarantine file");
        assert!(found_wal, "expected -wal.corrupt-* quarantine file");
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn quarantine_is_noop_when_no_sidecars() {
        let tmp = fresh_tmpdir("quarantine_is_noop_when_no_sidecars");
        let db = tmp.join("knowledge.db");
        std::fs::write(&db, b"only-main").unwrap();

        quarantine_wal_sidecars(&db);
        assert!(db.exists());
        assert_eq!(std::fs::read(&db).unwrap(), b"only-main");
        std::fs::remove_dir_all(&tmp).ok();
    }
}
