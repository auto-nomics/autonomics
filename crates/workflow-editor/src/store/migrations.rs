//! Tiny migration runner for rusqlite (no external `sqlx::migrate!` available).
//!
//! Embeds the `migrations/` directory at compile time, tracks applied
//! versions in a `_workflow_editor_migrations` table, and applies only the
//! ones missing. This mirrors sqlx's `ignore_missing = true` behavior so
//! adding a new migration never bricks older binaries.

use crate::error::StorageError;
use crate::store::pool::DbPool;
use rusqlite::params;

/// Embedded migration files in lexicographic order. The numeric prefix is
/// parsed as the version id.
const EMBEDDED: &[(&str, &str)] = &[
    ("0001_init", include_str!("../../migrations/0001_init.sql")),
    (
        "0002_skills",
        include_str!("../../migrations/0002_skills.sql"),
    ),
    (
        "0003_node_kinds",
        include_str!("../../migrations/0003_node_kinds.sql"),
    ),
];

/// Run all pending migrations against `pool`.
///
/// Idempotent — calling it twice in a row is a no-op the second time.
pub fn run(pool: &DbPool) -> Result<(), StorageError> {
    let mut conn = pool.lock();

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _workflow_editor_migrations (
            version     INTEGER PRIMARY KEY,
            name        TEXT NOT NULL,
            applied_at  TEXT NOT NULL
        )",
    )?;

    let mut applied: std::collections::BTreeSet<i64> = std::collections::BTreeSet::new();
    {
        let mut stmt = conn.prepare("SELECT version FROM _workflow_editor_migrations")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let v: i64 = row.get(0)?;
            applied.insert(v);
        }
    }

    let now = chrono::Utc::now().to_rfc3339();
    for (name, sql) in EMBEDDED {
        let version: i64 = name
            .split('_')
            .next()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| {
                StorageError::Migration(format!(
                    "migration filename must start with numeric version: {name}"
                ))
            })?;
        if applied.contains(&version) {
            continue;
        }
        let tx = conn.transaction()?;
        tx.execute_batch(sql).map_err(|e| {
            StorageError::Migration(format!("migration {name} failed: {e}\n---\n{}\n---", sql))
        })?;
        tx.execute(
            "INSERT INTO _workflow_editor_migrations (version, name, applied_at)
             VALUES (?1, ?2, ?3)",
            params![version, name, now],
        )?;
        tx.commit()?;
    }

    Ok(())
}
