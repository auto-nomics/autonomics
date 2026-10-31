//! Reshape + recovery for the database the 2026-10 KMS unification left
//! behind: it dropped `scope_id` from the memory tables, switched
//! stage1/jobs/summaries to singleton primary keys, and converted the flat
//! entries into knowledge-tree nodes. Opening such a database with the
//! restored layout used to fail inside `CREATE INDEX IF NOT EXISTS`
//! ("invalid expression in CREATE INDEX: scope_id").

use agentik_memory::TursoMemoryStorage;

const NIL: &str = "00000000-0000-0000-0000-000000000000";

/// The KMS-era shapes as they exist in a real unified knowledge.db.
const KMS_TABLES: &[&str] = &[
    "CREATE TABLE memory_stage1 (session_id TEXT NOT NULL PRIMARY KEY, source_hash TEXT NOT NULL, raw_memory TEXT NOT NULL, rollout_summary TEXT NOT NULL, rollout_slug TEXT, status TEXT NOT NULL, generated_at INTEGER NOT NULL, lease_until INTEGER NOT NULL DEFAULT 0, attempts INTEGER NOT NULL DEFAULT 0, last_error TEXT, updated_at INTEGER NOT NULL)",
    "CREATE INDEX idx_memory_stage1_generated ON memory_stage1 (generated_at DESC)",
    "CREATE TABLE memory_jobs (singleton INTEGER PRIMARY KEY DEFAULT 0, status TEXT NOT NULL, source_hash TEXT NOT NULL, lease_until INTEGER NOT NULL, attempts INTEGER NOT NULL DEFAULT 0, last_error TEXT, updated_at INTEGER NOT NULL)",
    "CREATE TABLE memory_entries (id TEXT PRIMARY KEY, scope_id TEXT NOT NULL, entry_type TEXT NOT NULL, title TEXT NOT NULL, body_md TEXT NOT NULL, status TEXT NOT NULL, confidence REAL NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL)",
    "CREATE INDEX idx_memory_entries_scope_status ON memory_entries (scope_id, status, updated_at DESC)",
    "CREATE TABLE memory_summaries (singleton INTEGER PRIMARY KEY DEFAULT 0, schema_version TEXT NOT NULL, summary_md TEXT NOT NULL, source_hash TEXT NOT NULL, generated_at INTEGER NOT NULL)",
    "CREATE TABLE memory_notes (id TEXT PRIMARY KEY, slug TEXT NOT NULL, content TEXT NOT NULL, status TEXT NOT NULL, created_at INTEGER NOT NULL)",
    "CREATE INDEX idx_memory_notes_status ON memory_notes (status, created_at)",
    "CREATE TABLE memory_semantic_observations (id TEXT PRIMARY KEY, subject TEXT NOT NULL, predicate TEXT NOT NULL, object TEXT NOT NULL, content TEXT NOT NULL, status TEXT NOT NULL, confidence REAL NOT NULL, source_hash TEXT NOT NULL, created_at INTEGER NOT NULL, last_error TEXT)",
    "CREATE INDEX idx_memory_semantic_status ON memory_semantic_observations (status, created_at DESC)",
    "CREATE TABLE memory_storage_metadata (\"key\" TEXT PRIMARY KEY, value TEXT NOT NULL)",
];

const KMS_ROWS: &[&str] = &[
    "INSERT INTO memory_stage1 (session_id, source_hash, raw_memory, rollout_summary, status, generated_at, updated_at) VALUES ('s1', 'h1', 'raw', 'sum', 'done', 10, 20)",
    "INSERT INTO memory_jobs (singleton, status, source_hash, lease_until, attempts, updated_at) VALUES (0, 'done', 'h1', 0, 1, 20)",
    "INSERT INTO memory_summaries (singleton, schema_version, summary_md, source_hash, generated_at) VALUES (0, 'v1', '# Summary', 'h1', 30)",
    "INSERT INTO memory_notes (id, slug, content, status, created_at) VALUES ('n1', 'slug-1', 'note body', 'ready', 40)",
    "INSERT INTO memory_semantic_observations (id, subject, predicate, object, content, status, confidence, source_hash, created_at) VALUES ('o1', 'A', 'uses', 'B', 'obs', 'ready', 0.9, 'h2', 50)",
    "INSERT INTO memory_storage_metadata (key, value) VALUES ('legacy_agent_db_memory_migrated_v1', '/frozen/agent.db')",
    "INSERT INTO memory_storage_metadata (key, value) VALUES ('memory_entries_to_knowledge_nodes_v1', 'migrated')",
];

async fn raw_connection(path: &std::path::Path) -> turso::Connection {
    let db = turso::Builder::new_local(path.to_str().unwrap())
        .experimental_multiprocess_wal(true)
        .build()
        .await
        .unwrap();
    db.connect().unwrap()
}

async fn exec_all(conn: &turso::Connection, statements: &[&str]) {
    for sql in statements {
        conn.execute(sql, ()).await.unwrap();
    }
}

async fn scalar_text(conn: &turso::Connection, sql: &str) -> String {
    let mut rows = conn.query(sql, ()).await.unwrap();
    let row = rows.next().await.unwrap().expect("query returned no rows");
    row.get::<String>(0).unwrap()
}

async fn scalar_count(conn: &turso::Connection, sql: &str) -> i64 {
    let mut rows = conn.query(sql, ()).await.unwrap();
    let row = rows.next().await.unwrap().expect("query returned no rows");
    row.get::<i64>(0).unwrap()
}

#[tokio::test]
async fn kms_era_database_is_reshaped_and_entries_recovered() {
    let dir = tempfile::tempdir().unwrap();
    let knowledge_db = dir.path().join("knowledge.db");
    let agent_db = dir.path().join("agent.db");

    // The unified knowledge.db: KMS-era shapes, markers, live rows.
    let conn = raw_connection(&knowledge_db).await;
    exec_all(&conn, KMS_TABLES).await;
    exec_all(&conn, KMS_ROWS).await;
    drop(conn);

    // The frozen agent.db: current layout, still holding the flat entries
    // the unification converted into knowledge-tree nodes.
    let conn = raw_connection(&agent_db).await;
    exec_all(
        &conn,
        &[
            "CREATE TABLE memory_entries (id TEXT PRIMARY KEY, scope_id TEXT NOT NULL, entry_type TEXT NOT NULL, title TEXT NOT NULL, body_md TEXT NOT NULL, status TEXT NOT NULL, confidence REAL NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL)",
            "INSERT INTO memory_entries VALUES ('e1', '00000000-0000-0000-0000-000000000000', 'fact', 'Title A', 'body a', 'ready', 0.8, 1, 2)",
            "INSERT INTO memory_entries VALUES ('e2', '00000000-0000-0000-0000-000000000000', 'fact', 'Title B', 'body b', 'ready', 0.7, 3, 4)",
        ],
    )
    .await;
    drop(conn);

    // The migration itself: open reshapes, migrate_from_agent_db recovers.
    let store = TursoMemoryStorage::open(&knowledge_db).await.unwrap();
    store.migrate_from_agent_db(&agent_db).await.unwrap();
    drop(store);

    let conn = raw_connection(&knowledge_db).await;

    // Shapes: every table back on the current layout.
    for (table, needle) in [
        ("memory_stage1", "PRIMARY KEY (scope_id, session_id)"),
        ("memory_jobs", "scope_id TEXT PRIMARY KEY"),
        ("memory_summaries", "scope_id TEXT PRIMARY KEY"),
        ("memory_notes", "scope_id TEXT NOT NULL"),
        ("memory_semantic_observations", "scope_id TEXT NOT NULL"),
    ] {
        let ddl = scalar_text(
            &conn,
            &format!("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = '{table}'"),
        )
        .await;
        assert!(ddl.contains(needle), "{table} not reshaped: {ddl}");
    }

    // Rows preserved under the process-wide nil scope.
    assert_eq!(
        scalar_text(
            &conn,
            "SELECT scope_id || '/' || session_id FROM memory_stage1 WHERE session_id = 's1'"
        )
        .await,
        format!("{NIL}/s1")
    );
    assert_eq!(
        scalar_text(&conn, "SELECT scope_id FROM memory_jobs").await,
        NIL
    );
    assert_eq!(
        scalar_text(
            &conn,
            "SELECT scope_id || '/' || schema_version FROM memory_summaries"
        )
        .await,
        format!("{NIL}/v1")
    );
    assert_eq!(
        scalar_text(&conn, "SELECT scope_id FROM memory_notes WHERE id = 'n1'").await,
        NIL
    );
    assert_eq!(
        scalar_text(
            &conn,
            "SELECT scope_id FROM memory_semantic_observations WHERE id = 'o1'"
        )
        .await,
        NIL
    );

    // Entries recovered from the frozen agent.db copy.
    assert_eq!(
        scalar_count(&conn, "SELECT COUNT(*) FROM memory_entries").await,
        2
    );
    assert_eq!(
        scalar_text(
            &conn,
            "SELECT group_concat(title) FROM (SELECT title FROM memory_entries ORDER BY id)"
        )
        .await,
        "Title A,Title B"
    );

    // Indexes: current set present, unified-era leftovers and staging
    // tables gone.
    for index in [
        "idx_memory_stage1_generated",
        "idx_memory_entries_scope_status",
        "idx_memory_notes_scope_status",
        "idx_memory_semantic_scope_status",
    ] {
        assert_eq!(
            scalar_count(
                &conn,
                &format!(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = '{index}'"
                )
            )
            .await,
            1,
            "missing index {index}"
        );
    }
    for gone in [
        "idx_memory_notes_status",
        "idx_memory_semantic_status",
        "memory_stage1__kms_mig",
        "memory_jobs__kms_mig",
        "memory_summaries__kms_mig",
    ] {
        assert_eq!(
            scalar_count(
                &conn,
                &format!("SELECT COUNT(*) FROM sqlite_master WHERE name = '{gone}'")
            )
            .await,
            0,
            "{gone} should be gone"
        );
    }

    // Recovery marker set; legacy marker untouched.
    assert_eq!(
        scalar_count(
            &conn,
            "SELECT COUNT(*) FROM memory_storage_metadata WHERE key = 'kms_memory_entries_recovered_v1'"
        )
        .await,
        1
    );
    drop(conn);

    // Re-open: a reshaped database must open cleanly (the original failure
    // mode — re-declaring an existing multi-column index) and stay at two
    // entries.
    let store = TursoMemoryStorage::open(&knowledge_db).await.unwrap();
    store.migrate_from_agent_db(&agent_db).await.unwrap();
    drop(store);
    let conn = raw_connection(&knowledge_db).await;
    assert_eq!(
        scalar_count(&conn, "SELECT COUNT(*) FROM memory_entries").await,
        2,
        "reopening duplicated entries"
    );
}

#[tokio::test]
async fn fresh_database_opens_with_the_current_layout() {
    let dir = tempfile::tempdir().unwrap();
    let knowledge_db = dir.path().join("knowledge.db");

    let store = TursoMemoryStorage::open(&knowledge_db).await.unwrap();
    store.migrate_from_agent_db(&knowledge_db).await.unwrap();
    drop(store);

    let conn = raw_connection(&knowledge_db).await;
    for object in [
        "memory_stage1",
        "memory_jobs",
        "memory_entries",
        "memory_summaries",
        "memory_notes",
        "memory_semantic_observations",
        "memory_storage_metadata",
        "idx_memory_stage1_generated",
        "idx_memory_entries_scope_status",
        "idx_memory_notes_scope_status",
        "idx_memory_semantic_scope_status",
    ] {
        assert_eq!(
            scalar_count(
                &conn,
                &format!("SELECT COUNT(*) FROM sqlite_master WHERE name = '{object}'")
            )
            .await,
            1,
            "missing {object}"
        );
    }
    // Second open creates nothing new (guarded index creation).
    drop(conn);
    let store = TursoMemoryStorage::open(&knowledge_db).await.unwrap();
    drop(store);
}
