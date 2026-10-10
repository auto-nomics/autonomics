//! One-time migration of legacy KMS rows out of the shared `agent.db`.
//!
//! Older versions kept the KMS tables inside the agent persistence
//! database. Production code now opens a dedicated knowledge database via
//! [`Storage::new`](super::Storage::new); on startup the rows are copied
//! out of the legacy agent database once. The copy is idempotent (a marker
//! row in `kms_meta` records the decision permanently), atomic (one
//! transaction on the target), and safe when the daemon and the KMS TUI
//! run it concurrently (`BEGIN IMMEDIATE` serializes the writers and the
//! marker is re-checked inside the transaction).
//!
//! Legacy tables are deliberately left in `agent.db` — non-destructive, no
//! write lock on the agent database during startup, and the old binary
//! keeps working if a downgrade is ever needed.

use std::sync::Arc;

use tokio::sync::Mutex;
use turso::{Value, params_from_iter};

use super::Storage;

/// `kms_meta` key recording that the legacy migration has been considered.
pub const LEGACY_MIGRATION_MARKER_KEY: &str = "legacy_agent_db_migrated_v1";

/// Marker value written when rows were copied out of the agent database.
const MARKER_MIGRATED: &str = "migrated";
/// Marker value written when the agent database predates KMS (or the
/// installation is fresh) — nothing to copy, ever.
const MARKER_NO_LEGACY_TABLES: &str = "no_legacy_tables";
/// Marker value written when the target already held data without a
/// marker — merging could violate primary keys, so the legacy rows stay.
const MARKER_SKIPPED_TARGET_NONEMPTY: &str = "skipped_target_nonempty";

/// Outcome of [`migrate_from_legacy_agent_db`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationOutcome {
    /// Marker already present — nothing to do.
    AlreadyMigrated,
    /// Rows copied inside one transaction; marker written.
    Migrated {
        entities: usize,
        nomenclatures: usize,
        knowledges: usize,
        indexes: usize,
    },
    /// Source had no KMS tables (agent.db predates KMS or fresh install).
    NoLegacyTables,
    /// Target already held user data without a marker; skipped to avoid
    /// primary-key conflicts. Legacy rows remain in the agent database.
    SkippedTargetNonEmpty,
}

/// Copy legacy KMS rows from the agent database connection into `target`.
///
/// Call this **before** constructing a [`KmsService`](crate::KmsService):
/// `from_storage` seeds a root index row, which would make a fresh target
/// look "non-empty without a marker" and permanently block the migration.
/// A failure is returned as `Err` for the same reason — callers must not
/// continue into service construction on a half-migrated database.
///
/// Lock order is always target → source and only this function ever holds
/// both, so it cannot deadlock against repo access.
pub async fn migrate_from_legacy_agent_db(
    target: &Storage,
    source: Arc<Mutex<turso::Connection>>,
) -> Result<MigrationOutcome, String> {
    // Fast path: a present marker makes this a no-op without touching the
    // source connection at all.
    {
        let conn = target.conn().await;
        if marker_value(&conn).await?.is_some() {
            return Ok(MigrationOutcome::AlreadyMigrated);
        }
    }

    // Slow path: hold both locks (target first, then source) for the whole
    // copy so the source snapshot and the target transaction stay coherent.
    let target_conn = target.conn().await;
    let source_conn = source.lock().await;

    let legacy_tables = legacy_kms_table_count(&source_conn).await?;

    target_conn
        .execute("BEGIN IMMEDIATE", ())
        .await
        .map_err(|e| format!("begin migration transaction: {e}"))?;

    let result = run_migration(&target_conn, &source_conn, legacy_tables).await;
    match result {
        Ok(outcome) => {
            target_conn
                .execute("COMMIT", ())
                .await
                .map_err(|e| format!("commit migration transaction: {e}"))?;
            Ok(outcome)
        }
        Err(error) => {
            let _ = target_conn.execute("ROLLBACK", ()).await;
            Err(error)
        }
    }
}

/// Body of the migration, running inside an open `BEGIN IMMEDIATE`
/// transaction on `target`. On `Err` the caller rolls back.
async fn run_migration(
    target: &turso::Connection,
    source: &turso::Connection,
    legacy_tables: i64,
) -> Result<MigrationOutcome, String> {
    // Re-check inside the transaction: a concurrent process may have
    // finished the whole migration while we were waiting on the write lock.
    if marker_value(target).await?.is_some() {
        return Ok(MigrationOutcome::AlreadyMigrated);
    }

    if legacy_tables == 0 {
        set_marker(target, MARKER_NO_LEGACY_TABLES).await?;
        return Ok(MigrationOutcome::NoLegacyTables);
    }

    // Guard: rows in the target without a marker mean someone already uses
    // this knowledge database (e.g. a previous migration attempt was
    // interrupted and the service seeded a root index). Merging now could
    // violate primary keys, so record the skip permanently.
    if target_row_count(target).await? > 0 {
        set_marker(target, MARKER_SKIPPED_TARGET_NONEMPTY).await?;
        return Ok(MigrationOutcome::SkippedTargetNonEmpty);
    }

    let entities = copy_table(target, source, "entities", &["id", "definition"]).await?;
    let nomenclatures = copy_table(
        target,
        source,
        "nomenclatures",
        &["id", "entity_id", "lang", "full", "abbr"],
    )
    .await?;
    let knowledges = copy_table(
        target,
        source,
        "knowledges",
        &[
            "id",
            "title",
            "knowledge_type",
            "entities",
            "content",
            "source_document_id",
            "source_chunk_idx",
        ],
    )
    .await?;
    let indexes = copy_table(
        target,
        source,
        "indexes",
        &[
            "id",
            "title",
            "target",
            "target_type",
            "parent_id",
            "position",
        ],
    )
    .await?;

    set_marker(target, MARKER_MIGRATED).await?;
    Ok(MigrationOutcome::Migrated {
        entities,
        nomenclatures,
        knowledges,
        indexes,
    })
}

/// Read the marker value from `kms_meta`, if present.
async fn marker_value(conn: &turso::Connection) -> Result<Option<String>, String> {
    let mut rows = conn
        .query(
            "SELECT value FROM kms_meta WHERE key = ?1",
            params_from_iter([Value::Text(LEGACY_MIGRATION_MARKER_KEY.to_string())]),
        )
        .await
        .map_err(|e| format!("read migration marker: {e}"))?;
    match rows.next().await.map_err(|e| e.to_string())? {
        Some(row) => match row.get_value(0).map_err(|e| e.to_string())? {
            Value::Text(value) => Ok(Some(value)),
            other => Err(format!("migration marker is not TEXT: {other:?}")),
        },
        None => Ok(None),
    }
}

/// Write the marker value (upsert — the caller checked it is absent, but a
/// concurrent writer may have committed in between on a different path).
async fn set_marker(conn: &turso::Connection, value: &str) -> Result<(), String> {
    conn.execute(
        "INSERT OR REPLACE INTO kms_meta (key, value) VALUES (?1, ?2)",
        params_from_iter([
            Value::Text(LEGACY_MIGRATION_MARKER_KEY.to_string()),
            Value::Text(value.to_string()),
        ]),
    )
    .await
    .map_err(|e| format!("write migration marker: {e}"))?;
    Ok(())
}

/// Number of legacy KMS tables present in the source database.
async fn legacy_kms_table_count(conn: &turso::Connection) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN
                ('entities', 'nomenclatures', 'knowledges', 'indexes')",
            params_from_iter([] as [Value; 0]),
        )
        .await
        .map_err(|e| format!("scan legacy tables: {e}"))?;
    let row = rows
        .next()
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "legacy table scan returned no row".to_string())?;
    match row.get_value(0).map_err(|e| e.to_string())? {
        Value::Integer(count) => Ok(count),
        other => Err(format!("legacy table count is not INTEGER: {other:?}")),
    }
}

/// Total row count across the four KMS tables in the target database.
async fn target_row_count(conn: &turso::Connection) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT (SELECT COUNT(*) FROM entities)
                + (SELECT COUNT(*) FROM nomenclatures)
                + (SELECT COUNT(*) FROM knowledges)
                + (SELECT COUNT(*) FROM indexes)",
            params_from_iter([] as [Value; 0]),
        )
        .await
        .map_err(|e| format!("count target rows: {e}"))?;
    let row = rows
        .next()
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "target row count returned no row".to_string())?;
    match row.get_value(0).map_err(|e| e.to_string())? {
        Value::Integer(count) => Ok(count),
        other => Err(format!("target row count is not INTEGER: {other:?}")),
    }
}

/// Copy every row of `table` from `source` into `target`, passing the
/// column values through untouched so NULL / empty-string distinctions are
/// preserved. Returns the number of rows copied.
async fn copy_table(
    target: &turso::Connection,
    source: &turso::Connection,
    table: &str,
    cols: &[&str],
) -> Result<usize, String> {
    let col_list = cols.join(", ");
    let placeholders: Vec<String> = (1..=cols.len()).map(|i| format!("?{i}")).collect();
    let select = format!("SELECT {col_list} FROM {table}");
    let insert = format!(
        "INSERT INTO {table} ({col_list}) VALUES ({})",
        placeholders.join(", ")
    );

    // Read the source fully before writing to the target — a `Rows` cursor
    // and concurrent statements on one connection do not mix in Turso.
    let mut rows = source
        .query(select.as_str(), params_from_iter([] as [Value; 0]))
        .await
        .map_err(|e| format!("read legacy {table}: {e}"))?;
    let mut copied = Vec::new();
    loop {
        match rows.next().await {
            Ok(Some(row)) => {
                let mut values = Vec::with_capacity(cols.len());
                for idx in 0..cols.len() {
                    values.push(row.get_value(idx).map_err(|e| e.to_string())?);
                }
                copied.push(values);
            }
            Ok(None) => break,
            Err(e) => return Err(format!("read legacy {table}: {e}")),
        }
    }

    for values in &copied {
        target
            .execute(insert.as_str(), params_from_iter(values.iter().cloned()))
            .await
            .map_err(|e| format!("copy into {table}: {e}"))?;
    }
    Ok(copied.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Open a bare Turso file database (no KMS schema) — stands in for a
    /// legacy `agent.db` that predates KMS.
    async fn bare_connection(path: &std::path::Path) -> Arc<Mutex<turso::Connection>> {
        let db = turso::Builder::new_local(path.to_string_lossy().as_ref())
            .build()
            .await
            .unwrap();
        Arc::new(Mutex::new(db.connect().unwrap()))
    }

    /// Open a "legacy" agent.db with the KMS schema seeded onto a bare
    /// connection — mirrors how the old runtime initialized KMS inside
    /// agent.db via `from_shared_connection`.
    async fn legacy_kms_db(path: &std::path::Path) -> Arc<Mutex<turso::Connection>> {
        let conn = bare_connection(path).await;
        Storage::from_shared_connection(conn.clone()).await.unwrap();
        conn
    }

    #[tokio::test]
    async fn migration_is_noop_when_source_has_no_kms_tables() {
        let dir = tempfile::tempdir().unwrap();
        let source = bare_connection(&dir.path().join("agent.db")).await;
        let target = Storage::new(dir.path().join("knowledge.db").to_string_lossy().as_ref())
            .await
            .unwrap();

        let outcome = migrate_from_legacy_agent_db(&target, source.clone())
            .await
            .unwrap();
        assert_eq!(outcome, MigrationOutcome::NoLegacyTables);

        // The decision is recorded permanently — a second run short-circuits.
        let conn = target.conn().await;
        assert_eq!(
            marker_value(&conn).await.unwrap().as_deref(),
            Some(MARKER_NO_LEGACY_TABLES)
        );
        drop(conn);
        let outcome = migrate_from_legacy_agent_db(&target, source).await.unwrap();
        assert_eq!(outcome, MigrationOutcome::AlreadyMigrated);
    }

    #[tokio::test]
    async fn migration_copies_all_rows_with_null_fidelity() {
        let dir = tempfile::tempdir().unwrap();
        let source = legacy_kms_db(&dir.path().join("agent.db")).await;
        {
            // Seed raw rows exercising every nullable column.
            let conn = source.lock().await;
            conn.execute_batch(
                "INSERT INTO entities (id, definition) VALUES ('e1', 'def one');
                 INSERT INTO nomenclatures (id, entity_id, lang, full, abbr)
                    VALUES ('n1', 'e1', 'en', 'Foo Bar', NULL),
                           ('n2', 'e1', 'zh', '福', 'F');
                 INSERT INTO knowledges (id, title, knowledge_type, entities, content,
                         source_document_id, source_chunk_idx)
                    VALUES ('k1', 'Title A', 'aspect', '[\"e1\"]', NULL, NULL, NULL),
                           ('k2', 'Title B', 'relation', '[\"e1\"]', 'body', 'doc-9', 3);
                 INSERT INTO indexes (id, title, target, target_type, parent_id, position)
                    VALUES ('root', NULL, NULL, 'group', NULL, 0),
                           ('leaf', 'Leaf', 'k1', 'knowledge', 'root', 0);",
            )
            .await
            .unwrap();
        }

        let target = Storage::new(dir.path().join("knowledge.db").to_string_lossy().as_ref())
            .await
            .unwrap();
        let outcome = migrate_from_legacy_agent_db(&target, source.clone())
            .await
            .unwrap();
        assert_eq!(
            outcome,
            MigrationOutcome::Migrated {
                entities: 1,
                nomenclatures: 2,
                knowledges: 2,
                indexes: 2
            }
        );

        let conn = target.conn().await;
        // NULLs survive the copy verbatim.
        let mut rows = conn
            .query(
                "SELECT abbr FROM nomenclatures WHERE id = 'n1'",
                params_from_iter([] as [Value; 0]),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert!(matches!(row.get_value(0).unwrap(), Value::Null));

        let mut rows = conn
            .query(
                "SELECT content, source_document_id, source_chunk_idx
                    FROM knowledges WHERE id = 'k1'",
                params_from_iter([] as [Value; 0]),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert!(matches!(row.get_value(0).unwrap(), Value::Null));
        assert!(matches!(row.get_value(1).unwrap(), Value::Null));
        assert!(matches!(row.get_value(2).unwrap(), Value::Null));

        let mut rows = conn
            .query(
                "SELECT title, target, parent_id FROM indexes WHERE id = 'root'",
                params_from_iter([] as [Value; 0]),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert!(matches!(row.get_value(0).unwrap(), Value::Null));
        assert!(matches!(row.get_value(1).unwrap(), Value::Null));
        assert!(matches!(row.get_value(2).unwrap(), Value::Null));

        assert_eq!(
            marker_value(&conn).await.unwrap().as_deref(),
            Some(MARKER_MIGRATED)
        );
        drop(conn);

        // Idempotent: second run is a no-op and the rows are unchanged.
        let outcome = migrate_from_legacy_agent_db(&target, source.clone())
            .await
            .unwrap();
        assert_eq!(outcome, MigrationOutcome::AlreadyMigrated);
        let conn = target.conn().await;
        assert_eq!(target_row_count(&conn).await.unwrap(), 7);

        // The legacy source is left untouched — rows stay in agent.db.
        let source_conn = source.lock().await;
        let mut rows = source_conn
            .query(
                "SELECT (SELECT COUNT(*) FROM entities)
                    + (SELECT COUNT(*) FROM nomenclatures)
                    + (SELECT COUNT(*) FROM knowledges)
                    + (SELECT COUNT(*) FROM indexes)",
                params_from_iter([] as [Value; 0]),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert!(matches!(row.get_value(0).unwrap(), Value::Integer(7)));
    }

    #[tokio::test]
    async fn migration_skips_when_target_nonempty_without_marker() {
        let dir = tempfile::tempdir().unwrap();
        let source = legacy_kms_db(&dir.path().join("agent.db")).await;
        {
            let conn = source.lock().await;
            conn.execute_batch(
                "INSERT INTO entities (id, definition) VALUES ('e1', 'legacy def');",
            )
            .await
            .unwrap();
        }

        // Target already holds a row and has no marker — merging would risk
        // primary-key conflicts, so the migration must refuse.
        let target = Storage::new(dir.path().join("knowledge.db").to_string_lossy().as_ref())
            .await
            .unwrap();
        {
            let conn = target.conn().await;
            conn.execute(
                "INSERT INTO entities (id, definition) VALUES ('mine', 'own row')",
                params_from_iter([] as [Value; 0]),
            )
            .await
            .unwrap();
        }

        let outcome = migrate_from_legacy_agent_db(&target, source.clone())
            .await
            .unwrap();
        assert_eq!(outcome, MigrationOutcome::SkippedTargetNonEmpty);

        let conn = target.conn().await;
        assert_eq!(
            marker_value(&conn).await.unwrap().as_deref(),
            Some(MARKER_SKIPPED_TARGET_NONEMPTY)
        );
        // The target rows are untouched (still exactly the one own row).
        assert_eq!(target_row_count(&conn).await.unwrap(), 1);
        drop(conn);

        // And the source keeps its legacy rows.
        let source_conn = source.lock().await;
        let mut rows = source_conn
            .query(
                "SELECT COUNT(*) FROM entities",
                params_from_iter([] as [Value; 0]),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert!(matches!(row.get_value(0).unwrap(), Value::Integer(1)));
    }

    #[tokio::test]
    async fn migration_is_visible_through_the_service_layer() {
        // End-to-end: a legacy agent.db seeded via the service APIs keeps
        // working after migration — the root index comes across, so
        // `KmsService::from_storage` finds it instead of seeding a new one.
        let dir = tempfile::tempdir().unwrap();
        let source = legacy_kms_db(&dir.path().join("agent.db")).await;
        let legacy_service = crate::KmsService::from_storage(
            Storage::from_shared_connection(source.clone())
                .await
                .unwrap(),
        )
        .await
        .unwrap();
        legacy_service
            .create_entity(
                vec![crate::Nomenclature {
                    id: uuid::Uuid::new_v4(),
                    lang: crate::Language::ZH,
                    full: "迁移实体".to_string(),
                    abbr: None,
                }],
                "Entity stored in the legacy agent.db",
            )
            .await
            .unwrap();
        drop(legacy_service);

        let target = Storage::new(dir.path().join("knowledge.db").to_string_lossy().as_ref())
            .await
            .unwrap();
        migrate_from_legacy_agent_db(&target, source).await.unwrap();

        let service = crate::KmsService::from_storage(target).await.unwrap();
        let entities = service
            .list_entities(crate::EntityFilter::All)
            .await
            .unwrap();
        assert_eq!(entities.len(), 1);
        assert_eq!(
            entities[0].definition,
            "Entity stored in the legacy agent.db"
        );
        // The migrated root (parent_id IS NULL, title "Root") is found
        // instead of a freshly seeded one.
        let root = service.find_root().await.unwrap();
        assert_eq!(root.title.as_deref(), Some("Root"));
    }
}
