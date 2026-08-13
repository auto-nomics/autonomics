//! SQLite connection helper (synchronous `rusqlite::Connection` guarded by
//! a `parking_lot::Mutex`).
//!
//! The pool abstraction here is intentionally minimal — one connection,
//! serialized. SQLite WAL mode allows concurrent readers + one writer, and
//! `WorkflowRepo::save` is the only writer. We could swap in `r2d2_sqlite`
//! later if write throughput becomes a bottleneck.
//!
//! For async callers: `WorkflowClient` (Phase 1) wraps repo calls in
//! `tokio::task::spawn_blocking`.

use crate::error::StorageError;
use parking_lot::Mutex;
use rusqlite::Connection;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// Shared handle to a SQLite database.
///
/// Cheap to clone (internally `Arc`).
#[derive(Clone)]
pub struct DbPool {
    inner: Arc<Mutex<Connection>>,
    /// Keep the path alive for diagnostics; not used at runtime.
    #[allow(dead_code)]
    path: Arc<Path>,
}

impl DbPool {
    /// Open the pool at `path`. WAL journal, 5s busy timeout, foreign keys on.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let path = path.as_ref();
        let conn = Connection::open(path).map_err(StorageError::from)?;
        configure(&conn)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(conn)),
            path: Arc::from(path),
        })
    }

    /// Acquire the underlying connection lock.
    pub fn lock(&self) -> parking_lot::MutexGuard<'_, Connection> {
        self.inner.lock()
    }

    /// Path the pool was opened from.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Apply our standard PRAGMAs to a fresh connection.
fn configure(conn: &Connection) -> Result<(), StorageError> {
    conn.busy_timeout(Duration::from_secs(5))
        .map_err(StorageError::from)?;
    // WAL is file-only; silently no-op for in-memory databases.
    let _: Result<(), _> = conn.pragma_update(None, "journal_mode", "WAL");
    conn.pragma_update(None, "synchronous", "NORMAL")
        .map_err(StorageError::from)?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(StorageError::from)?;
    Ok(())
}

/// Open a SQLite pool at the given path.
pub fn open(path: impl AsRef<Path>) -> Result<DbPool, StorageError> {
    DbPool::open(path)
}

/// Convenience: open at a known file path.
pub fn open_file(path: impl AsRef<Path>) -> Result<DbPool, StorageError> {
    DbPool::open(path)
}
