use std::sync::Arc;

use agentik_core::tools::Toolset;
use agentik_sdk::types::tools::{ToolResult, ToolResultContent, ToolUse};
use data_engine::data_engine::DataEngine;
use data_engine::runtime::spawn_with_engine;
use serde_json::json;
use vfs::{
    BackendConfig, BackendDefinition, MountDefinition, MountedObjectStore, OpendalFileStorage,
    VfsManifest,
};

fn build_tooluse(id: &str, name: &str, input: serde_json::Value) -> ToolUse {
    ToolUse {
        id: id.to_string(),
        name: name.to_string(),
        input,
    }
}

fn check_ok(result: &ToolResult, label: &str) {
    assert!(
        !result.is_error.unwrap_or(false),
        "{label} failed: {:?}",
        result.content
    );
}

async fn dag_shell(toolset: &Toolset, id: &str, script: &str) -> serde_json::Value {
    let results = toolset
        .execute(
            &[build_tooluse(id, "dag_shell", json!({"script": script}))],
            None,
        )
        .await
        .unwrap();
    check_ok(&results[0], id);
    result_json(&results[0])
}

fn result_json(result: &ToolResult) -> serde_json::Value {
    match &result.content {
        ToolResultContent::Json(value) => value.clone(),
        other => panic!("expected JSON tool result, got: {other:?}"),
    }
}



/// Hermetic per-test storage: a tempdir-backed mount at `/`, the shape
/// production engines run with. Fixture files are written into `files`
/// (the mount source) with `std::fs`; nodes address them by their virtual
/// `/…` paths.
///
/// This replaced `OpendalFileStorage::new("/mnt/disk3/test")`, which both
/// wrote fixtures into a real data disk and mis-modeled addressing: a bare
/// `/x` is a *host* path unless a mount covers it, so `file_to_dataframe`
/// correctly rejected it once host-path pre-validation landed — that, not
/// a product bug, is what rotted the get_output tests.
struct HermeticVfs {
    storage: Arc<OpendalFileStorage>,
    /// The mount table — engines must also get this via `.with_vfs(…)`,
    /// or their DataFusion session has no `vfs://` object store and node
    /// reads of mounted paths fail.
    mounted: Arc<MountedObjectStore>,
    /// Mount source: fixture files live here for the test's lifetime.
    files: tempfile::TempDir,
    /// Default-backend root (unused under the `/` mount, kept alive so the
    /// tempdir outlives the storage).
    _data: tempfile::TempDir,
}

fn hermetic_vfs() -> HermeticVfs {
    let files = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let manifest = VfsManifest {
        backend: vec![BackendDefinition {
            id: "default".into(),
            config: BackendConfig::local("/"),
        }],
        mount: vec![MountDefinition {
            domain: None,
            path: "/".into(),
            backend: "default".into(),
            source: files.path().to_string_lossy().to_string(),
            read_only: false,
            permissions: Default::default(),
        }],
    };
    let mounted = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
    HermeticVfs {
        storage: Arc::new(OpendalFileStorage::with_mounts(
            data.path(),
            mounted.clone(),
        )),
        mounted,
        files,
        _data: data,
    }
}

/// Write a fixture file into the hermetic mount at virtual `path`.
fn put_fixture(vfs: &HermeticVfs, path: &str, bytes: &[u8]) {
    let rel = path.trim_start_matches('/');
    std::fs::write(vfs.files.path().join(rel), bytes).unwrap();
}

#[tokio::test]
async fn test_add_source_sql_run_dag() {
    // 1. Set up hermetic storage and write test data
    let vfs = hermetic_vfs();
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let csv_path =
        std::path::Path::new(&manifest_dir).join("../data-engine/test_datasets/insurance.csv");
    let csv_data = std::fs::read(csv_path).unwrap();
    put_fixture(&vfs, "/insurance.csv", &csv_data);

    // 2. Build DataEngine and spawn server
    let engine = DataEngine::builder()
        .register_opendal_fs(vfs.storage.clone())
        .unwrap()
        .with_vfs((*vfs.mounted).clone())
        .build();
    let (client, _handle) = spawn_with_engine(engine);

    // 3. Register tools.
    let tools = data_engine_tools::registrations(Arc::new(client.clone()));
    let mut registry = agentik_core::tools::ToolRegistry::new();
    registry.register_all(tools).unwrap();
    let toolset = Toolset::from_registry(Arc::new(registry), None);

    // Build the whole graph in one transaction.
    let outcome = dag_shell(
        &toolset,
        "build",
        r#"
        node("src", "file_to_dataframe", {"path": "/insurance.csv"});
        node("sql", "sql", {"sql_query": "SELECT age, charges FROM port_0 WHERE age > 30 LIMIT 5"});
        node("sink", "dataframe_to_file", {"path": "/output.csv", "format": "csv", "mode": "overwrite"});
        edge("src", 0, "sql", 0);
        edge("sql", 0, "sink", 0);
        commit()
        "#,
    )
    .await;
    assert_eq!(outcome["applied"], json!(true));
    assert_eq!(outcome["graph"]["process_count"], json!(3));
    assert_eq!(outcome["graph"]["wire_count"], json!(2));
}

#[tokio::test]
async fn dag_shell_builds_then_removes_edge_and_node() {
    let engine = DataEngine::builder().build();
    let (client, _handle) = spawn_with_engine(engine);
    let tools = data_engine_tools::registrations(Arc::new(client));
    let mut registry = agentik_core::tools::ToolRegistry::new();
    registry.register_all(tools).unwrap();
    let toolset = Toolset::from_registry(Arc::new(registry), None);

    dag_shell(
        &toolset,
        "build",
        r#"
        node("source", "file_to_dataframe", {"path": None});
        node("transform", "sql", {"sql_query": "SELECT * FROM port_0"});
        edge("source", 0, "transform", 0);
        commit()
        "#,
    )
    .await;

    let removed = dag_shell(
        &toolset,
        "remove",
        r#"
        remove_edge("source", 0, "transform", 0);
        remove_node("transform");
        commit()
        "#,
    )
    .await;
    assert_eq!(removed["graph"]["process_count"], json!(1));
}



#[tokio::test]
async fn test_dag_runs_log_records_execution_audit_trail() {
    // Mounted-VFS storage arrangement (as in
    // `test_get_output_file_to_dataframe_csv_parquet_json`) so the source
    // node's path resolves through the mount.
    let mounted_root = tempfile::tempdir().unwrap();
    let data_root = tempfile::tempdir().unwrap();
    let manifest = VfsManifest {
        backend: vec![BackendDefinition {
            id: "default".into(),
            config: BackendConfig::local("/"),
        }],
        mount: vec![MountDefinition {
            domain: None,
            path: "/".into(),
            backend: "default".into(),
            source: mounted_root.path().to_string_lossy().to_string(),
            read_only: false,
            permissions: Default::default(),
        }],
    };
    let mounted = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
    let file_storage = Arc::new(OpendalFileStorage::with_mounts(
        data_root.path(),
        mounted.clone(),
    ));
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let csv_path =
        std::path::Path::new(&manifest_dir).join("../data-engine/test_datasets/insurance.csv");
    let csv_data = std::fs::read(csv_path).unwrap();
    file_storage
        .resolve("/insurance.csv")
        .write(&file_storage.resolve_path("/insurance.csv"), csv_data)
        .await
        .unwrap();

    let history = data_engine::dag::DagHistory::open_in_memory()
        .await
        .unwrap();
    let engine = DataEngine::builder()
        .register_opendal_fs(file_storage.clone())
        .unwrap()
        .with_vfs((*mounted).clone())
        .build()
        .with_history(history);
    let (client, _handle) = spawn_with_engine(engine);

    let tools = data_engine_tools::registrations(Arc::new(client.clone()));
    let mut registry = agentik_core::tools::ToolRegistry::new();
    registry.register_all(tools).unwrap();
    let toolset = Toolset::from_registry(Arc::new(registry), None);

    dag_shell(
        &toolset,
        "build",
        r#"
        node("src", "file_to_dataframe", {"path": "/insurance.csv"});
        node("sql", "sql", {"sql_query": "SELECT age FROM port_0 LIMIT 3"});
        edge("src", 0, "sql", 0);
        commit()
        "#,
    )
    .await;

    // Two executions through the tool — the audit path under test.
    for call in ["r1", "r2"] {
        let results = toolset
            .execute(&[build_tooluse(call, "run_flow", json!({}))], None)
            .await
            .unwrap();
        check_ok(&results[0], "run_flow");
        let report = result_json(&results[0]);
        assert_eq!(report["ok"], json!(true), "run failed: {report}");
    }

    // Both executions left a run row, attributed to the session that ran
    // them.
    let runs = client.dag_runs_log(None, 10, None).await.unwrap();
    assert_eq!(runs.len(), 2, "every execution leaves a run row");
    assert_eq!(runs[0].trigger.as_deref(), Some("agent:default"));
    assert_eq!(runs[1].trigger.as_deref(), Some("agent:default"));
    assert!(runs.iter().all(|run| run.ok));

    // The listing tool surfaces the audit trail as text.
    let results = toolset
        .execute(
            &[build_tooluse("l1", "dag_runs_log", json!({"limit": 5}))],
            None,
        )
        .await
        .unwrap();
    check_ok(&results[0], "dag_runs_log");

    // The detail view decodes the flow run report, including the per-process
    // summary rows.
    let results = toolset
        .execute(
            &[build_tooluse(
                "l2",
                "dag_runs_log",
                json!({"run_id": runs[0].id}),
            )],
            None,
        )
        .await
        .unwrap();
    check_ok(&results[0], "dag_runs_log detail");
    let detail = result_json(&results[0]);
    assert_eq!(detail["trigger"], json!("agent:default"));
    assert!(detail["source_revision"].is_string());
    assert_eq!(
        detail["processes"]["sql"]["total"],
        json!(1),
        "sql process summary is surfaced"
    );
}

// Execute the real source/sink implementations, then archive their measured
// output in the historical DAG report shape. These tests exercise export of
// existing records; the retired run_dag/dag_export_run agent tools are not
// part of the current tool registry.