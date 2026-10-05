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

#[tokio::test]
async fn agent_controls_channel_operators_with_logical_graph_tool() {
    let engine = DataEngine::builder().build();
    let (client, _handle) = spawn_with_engine(engine);
    let tools = data_engine_tools::registrations(Arc::new(client));
    let mut registry = agentik_core::tools::ToolRegistry::new();
    registry.register_all(tools).unwrap();
    let toolset = Toolset::from_registry(Arc::new(registry), None);

    let results = toolset
        .execute(
            &[build_tooluse(
                "channel-graph",
                "add_logical_graph",
                json!({
                    "graph": {
                        "nodes": [
                            {
                                "id": "agent_items",
                                "definition": {
                                    "Channel": {
                                        "operator": "of_items",
                                        "items": [{"id": "one"}, {"id": "two"}]
                                    }
                                },
                                "strategy": "Once"
                            },
                            {
                                "id": "agent_mapped",
                                "definition": {
                                    "Channel": {
                                        "operator": "map",
                                        "template": {"name": "{{item.id}}"}
                                    }
                                },
                                "strategy": "Once"
                            }
                        ],
                        "edges": [
                            {
                                "from": "agent_items",
                                "from_port": 0,
                                "to": "agent_mapped",
                                "to_port": 0
                            }
                        ]
                    }
                }),
            )],
            None,
        )
        .await
        .unwrap();
    check_ok(&results[0], "add_logical_graph");
    let installed = result_json(&results[0]);
    assert_eq!(installed["logical_node_count"], json!(2));
    assert!(
        installed["jobs"]
            .as_object()
            .unwrap()
            .contains_key("agent_items#0")
    );
    assert!(
        installed["jobs"]
            .as_object()
            .unwrap()
            .contains_key("agent_mapped#0")
    );

    let results = toolset
        .execute(&[build_tooluse("channel-run", "run_dag", json!({}))], None)
        .await
        .unwrap();
    check_ok(&results[0], "run_dag");
    let report = result_json(&results[0]);
    assert_eq!(report["ok"], json!(true));
    let physical_ids = report["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["id"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert!(physical_ids.contains(&"agent_items#0".to_string()));
    assert!(physical_ids.contains(&"agent_mapped#0".to_string()));
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
            path: "/".into(),
            backend: "default".into(),
            source: files.path().to_string_lossy().to_string(),
            read_only: false,
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

    // 4. Add source node via generic add_node
    let results = toolset
        .execute(
            &[build_tooluse(
                "tc1",
                "add_node",
                json!({"id": "src", "kind": "file_to_dataframe", "spec": {"path": "/insurance.csv"}}),
            )],
            None,
        )
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    check_ok(&results[0], "add_node source");

    // 5. Add SQL node via generic add_node
    let results = toolset
        .execute(
            &[build_tooluse(
                "tc2",
                "add_node",
                json!({
                    "id": "sql",
                    "kind": "sql",
                    "spec": {"sql_query": "SELECT age, charges FROM port_0 WHERE age > 30 LIMIT 5"}
                }),
            )],
            None,
        )
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    check_ok(&results[0], "add_node sql");

    // 6. Add edge (src -> sql) via tool
    let results = toolset
        .execute(
            &[build_tooluse(
                "tc3",
                "add_edge",
                json!({"from": "src", "from_port": 0, "to": "sql", "to_port": 0}),
            )],
            None,
        )
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    check_ok(&results[0], "add_edge");

    // 7. Add file sink node via generic add_node
    let results = toolset
        .execute(
            &[build_tooluse(
                "tc4",
                "add_node",
                json!({
                    "id": "sink",
                    "kind": "dataframe_to_file",
                    "spec": {"path": "/output.csv", "format": "csv", "mode": "overwrite"}
                }),
            )],
            None,
        )
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    check_ok(&results[0], "add_node file_sink");

    // 8. Edge (sql -> sink) via tool
    let results = toolset
        .execute(
            &[build_tooluse(
                "tc5",
                "add_edge",
                json!({"from": "sql", "from_port": 0, "to": "sink", "to_port": 0}),
            )],
            None,
        )
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    check_ok(&results[0], "add_edge sql->sink");
}

#[tokio::test]
async fn test_get_output_file_to_dataframe_csv_parquet_json() {
    let mounted_root = tempfile::tempdir().unwrap();
    let data_root = tempfile::tempdir().unwrap();
    let manifest = VfsManifest {
        backend: vec![BackendDefinition {
            id: "default".into(),
            config: BackendConfig::local("/"),
        }],
        mount: vec![MountDefinition {
            path: "/".into(),
            backend: "default".into(),
            source: mounted_root.path().to_string_lossy().to_string(),
            read_only: false,
        }],
    };
    let mounted = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
    let file_storage = Arc::new(OpendalFileStorage::with_mounts(
        data_root.path(),
        mounted.clone(),
    ));
    file_storage
        .resolve("/source.csv")
        .write(
            &file_storage.resolve_path("/source.csv"),
            "id,name\n1,alice\n2,bob\n",
        )
        .await
        .unwrap();
    file_storage
        .resolve("/source.json")
        .write(
            &file_storage.resolve_path("/source.json"),
            r#"[{"id":1,"name":"alice"},{"id":2,"name":"bob"}]"#,
        )
        .await
        .unwrap();

    let parquet_dir = tempfile::tempdir().unwrap();
    let parquet_path = parquet_dir.path().join("source.parquet");
    {
        use arrow::array::{Int32Array, StringArray};
        use arrow::datatypes::{DataType, Field, Schema};
        use datafusion::dataframe::DataFrameWriteOptions;
        use datafusion::prelude::SessionContext;
        use std::sync::Arc as StdArc;

        let schema = StdArc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("name", DataType::Utf8, false),
        ]));
        let batch = arrow_array::RecordBatch::try_new(
            schema,
            vec![
                StdArc::new(Int32Array::from(vec![1, 2])),
                StdArc::new(StringArray::from(vec!["alice", "bob"])),
            ],
        )
        .unwrap();
        let ctx = SessionContext::new();
        ctx.read_batch(batch)
            .unwrap()
            .write_parquet(
                &parquet_path.to_string_lossy(),
                DataFrameWriteOptions::new().with_single_file_output(true),
                None::<datafusion::config::TableParquetOptions>,
            )
            .await
            .unwrap();
    }
    file_storage
        .resolve("/source.parquet")
        .write(
            &file_storage.resolve_path("/source.parquet"),
            std::fs::read(&parquet_path).unwrap(),
        )
        .await
        .unwrap();

    let engine = DataEngine::builder()
        .register_opendal_fs(file_storage.clone())
        .unwrap()
        .with_vfs((*mounted).clone())
        .build();
    let (client, _handle) = spawn_with_engine(engine);
    let tools = data_engine_tools::registrations(Arc::new(client));
    let mut registry = agentik_core::tools::ToolRegistry::new();
    registry.register_all(tools).unwrap();
    let toolset = Toolset::from_registry(Arc::new(registry), None);

    for format in ["csv", "json", "parquet"] {
        let results = toolset
            .execute(
                &[build_tooluse(
                    "add",
                    "add_node",
                    json!({
                        "id": "source",
                        "kind": "file_to_dataframe",
                        "spec": {"path": format!("/source.{format}")}
                    }),
                )],
                None,
            )
            .await
            .unwrap();
        check_ok(&results[0], format!("add {format} source").as_str());

        let results = toolset
            .execute(&[build_tooluse("run", "run_dag", json!({}))], None)
            .await
            .unwrap();
        check_ok(&results[0], format!("run {format} source").as_str());
        let report = result_json(&results[0]);
        assert!(
            report["ok"].as_bool().unwrap_or(false),
            "{format} source run report: {report}"
        );

        let results = toolset
            .execute(
                &[build_tooluse(
                    "get",
                    "get_output",
                    json!({"id": "source", "limit": 10}),
                )],
                None,
            )
            .await
            .unwrap();
        check_ok(&results[0], format!("get {format} source").as_str());
        let parsed = result_json(&results[0]);
        let output = &parsed["outputs"][0];
        assert_eq!(
            output["columns"].as_u64().unwrap_or(0),
            2,
            "{format} source output: {parsed}"
        );
        assert_eq!(
            output["total_rows"].as_u64().unwrap_or(0),
            2,
            "{format} source output: {parsed}"
        );
        assert_eq!(
            output["returned_rows"].as_u64().unwrap_or(0),
            2,
            "{format} source output: {parsed}"
        );
        assert_eq!(
            output["data"]["rows"].as_array().map(Vec::len).unwrap_or(0),
            2,
            "{format} source output: {parsed}"
        );

        let results = toolset
            .execute(
                &[build_tooluse(
                    "remove",
                    "remove_node",
                    json!({"id": "source"}),
                )],
                None,
            )
            .await
            .unwrap();
        check_ok(&results[0], format!("remove {format} source").as_str());
    }
}

#[tokio::test]
async fn same_turn_add_nodes_and_edge_then_remove_edge_and_upstream() {
    let engine = DataEngine::builder().build();
    let (client, _handle) = spawn_with_engine(engine);
    let tools = data_engine_tools::registrations(Arc::new(client));
    let mut registry = agentik_core::tools::ToolRegistry::new();
    registry.register_all(tools).unwrap();
    let toolset = Toolset::from_registry(Arc::new(registry), None);

    let results = toolset
        .execute(
            &[
                build_tooluse(
                    "add-source",
                    "add_node",
                    json!({"id": "source", "kind": "file_to_dataframe", "spec": {"path": null}}),
                ),
                build_tooluse(
                    "add-sql",
                    "add_node",
                    json!({"id": "transform", "kind": "sql", "spec": {"sql_query": "SELECT * FROM port_0"}}),
                ),
                build_tooluse(
                    "connect",
                    "add_edge",
                    json!({"from": "source", "from_port": 0, "to": "transform", "to_port": 0}),
                ),
            ],
            None,
        )
        .await
        .unwrap();
    assert_eq!(results.len(), 3);
    for (index, result) in results.iter().enumerate() {
        check_ok(result, &format!("same-turn result {index}"));
    }

    let results = toolset
        .execute(
            &[build_tooluse(
                "disconnect",
                "remove_edge",
                json!({"from": "source", "from_port": 0, "to": "transform", "to_port": 0}),
            )],
            None,
        )
        .await
        .unwrap();
    check_ok(&results[0], "remove_edge");

    let results = toolset
        .execute(
            &[build_tooluse(
                "remove-source",
                "remove_node",
                json!({"id": "source"}),
            )],
            None,
        )
        .await
        .unwrap();
    check_ok(&results[0], "remove source after disconnect");
}

/// Regression: when a SqlNode output contains a Struct-typed column,
/// `get_output` must report `returned_rows == total_rows > 0`, not
/// `returned_rows: 0, total_rows: N`.
///
/// The agent's obstacle #2 reported exactly that pattern on a real VCF source
/// (`SELECT * FROM port_0 LIMIT 5` → `returned_rows: 0, total_rows: 5`). The
/// VCF `info` column carries Dictionary/List-encoded subfields that, on
/// `collect().await`, can error out and get silently swallowed by
/// `unwrap_or_default()` in `get_output_tool.rs:155`, yielding an empty batch
/// vector while `df.count()` (a separate, `COUNT(*)`-shaped plan) still
/// reports the true row count.
///
/// This test exercises the exact scenario end-to-end: real `sample.vcf.gz`
/// source → `SELECT * FROM port_0 LIMIT 5` SqlNode → run → `get_output`. It
/// pins that `returned_rows` matches `total_rows`, which is the contract the
/// agent's pipeline relied on.
#[tokio::test]
async fn test_get_output_vcf_select_star_returns_correct_rows() {
    let vfs = hermetic_vfs();
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let vcf_path =
        std::path::Path::new(&manifest_dir).join("../data-engine/test_datasets/sample.vcf.gz");
    let vcf_data = std::fs::read(vcf_path).unwrap();
    put_fixture(&vfs, "/sample.vcf.gz", &vcf_data);

    let engine = DataEngine::builder()
        .register_opendal_fs(vfs.storage.clone())
        .unwrap()
        .with_vfs((*vfs.mounted).clone())
        .build();
    let (client, _handle) = spawn_with_engine(engine);
    let tools = data_engine_tools::registrations(Arc::new(client.clone()));
    let mut registry = agentik_core::tools::ToolRegistry::new();
    registry.register_all(tools).unwrap();
    let toolset = Toolset::from_registry(Arc::new(registry), None);

    // 1. VCF source — auto-detected from the `.vcf.gz` extension.
    let res = toolset
        .execute(
            &[build_tooluse(
                "v1",
                "add_node",
                json!({"id": "vcf_src", "kind": "file_to_dataframe", "spec": {"path": "/sample.vcf.gz"}}),
            )],
            None,
        )
        .await
        .unwrap();
    check_ok(&res[0], "add_node source (vcf)");

    // 2. `SELECT * FROM port_0 LIMIT 5` — the exact query from the agent's
    //    obstacle #2 report. Forces the VCF (Struct + Dictionary/List info
    //    subfields) through the SqlNode collect path.
    let res = toolset
        .execute(
            &[build_tooluse(
                "v2",
                "add_node",
                json!({
                    "id": "preview",
                    "kind": "sql",
                    "spec": {"sql_query": "SELECT * FROM port_0 LIMIT 5"}
                }),
            )],
            None,
        )
        .await
        .unwrap();
    check_ok(&res[0], "add_node sql (SELECT * LIMIT 5)");

    // 3. Edge + run.
    let res = toolset
        .execute(
            &[build_tooluse(
                "v3",
                "add_edge",
                json!({"from": "vcf_src", "from_port": 0, "to": "preview", "to_port": 0}),
            )],
            None,
        )
        .await
        .unwrap();
    check_ok(&res[0], "add_edge vcf_src->preview");

    let res = toolset
        .execute(&[build_tooluse("v4", "run_dag", json!({}))], None)
        .await
        .unwrap();
    check_ok(&res[0], "run_dag");

    // 4. get_output and parse the JSON envelope.
    let res = toolset
        .execute(
            &[build_tooluse(
                "v5",
                "get_output",
                json!({"id": "preview", "limit": 50}),
            )],
            None,
        )
        .await
        .unwrap();
    check_ok(&res[0], "get_output preview");

    let parsed = parse_tool_json(&res[0].content);

    let outputs = parsed
        .get("outputs")
        .and_then(|o| o.as_array())
        .expect("get_output JSON should have `outputs` array");
    assert!(!outputs.is_empty(), "expected at least one output entry");

    let entry = &outputs[0];
    let total_rows = entry
        .get("total_rows")
        .and_then(|v| v.as_u64())
        .expect("total_rows should be a number") as usize;
    let returned_rows = entry
        .get("returned_rows")
        .and_then(|v| v.as_u64())
        .expect("returned_rows should be a number") as usize;

    // The agent saw `total_rows: 5` and `returned_rows: 0` for exactly this
    // query. Pin that the two counts agree; if they ever diverge again the
    // `collect().await.unwrap_or_default()` swallow in
    // `get_output_tool.rs:155` is silently dropping batches.
    assert!(
        total_rows > 0,
        "expected total_rows > 0 for sample.vcf.gz; got {total_rows}"
    );
    assert_eq!(
        total_rows, returned_rows,
        "OBSTACLE #2 regression: total_rows ({total_rows}) != returned_rows \
         ({returned_rows}); `collect()` on the VCF preview likely errored and \
         was swallowed by `unwrap_or_default()` in get_output_tool.rs."
    );
}

/// When a node fails during execution, it has no cached output. `get_output`
/// must still return a machine-readable error envelope (not free-form text)
/// so agents can reliably branch on the failure and tell the user what to do
/// next. The SQL division-by-zero below forces that path end to end.
#[tokio::test]
async fn test_get_output_failed_node_returns_structured_error() {
    let vfs = hermetic_vfs();
    // 5 rows; the downstream SQL divides by zero only when values materialize.
    let csv = b"age,s\n1,abc\n2,def\n3,ghi\n4,jkl\n5,mno\n";
    put_fixture(&vfs, "/badcast.csv", csv);

    let engine = DataEngine::builder()
        .register_opendal_fs(vfs.storage.clone())
        .unwrap()
        .with_vfs((*vfs.mounted).clone())
        .build();
    let (client, _handle) = spawn_with_engine(engine);
    let tools = data_engine_tools::registrations(Arc::new(client.clone()));
    let mut registry = agentik_core::tools::ToolRegistry::new();
    registry.register_all(tools).unwrap();
    let toolset = Toolset::from_registry(Arc::new(registry), None);

    let res = toolset
        .execute(
            &[build_tooluse(
                "e1",
                "add_node",
                json!({"id": "src", "kind": "file_to_dataframe", "spec": {"path": "/badcast.csv"}}),
            )],
            None,
        )
        .await
        .unwrap();
    check_ok(&res[0], "add_node source");

    // Runtime-erroring projection. The executor has no cached output after
    // this node fails.
    let res = toolset
        .execute(
            &[build_tooluse(
                "e2",
                "add_node",
                json!({
                    "id": "badcast",
                    "kind": "sql",
                    "spec": {
                        "sql_query": "SELECT sum(age) / 0 AS n FROM port_0"
                    }
                }),
            )],
            None,
        )
        .await
        .unwrap();
    check_ok(&res[0], "add_node sql (badcast)");

    let res = toolset
        .execute(
            &[build_tooluse(
                "e3",
                "add_edge",
                json!({"from": "src", "from_port": 0, "to": "badcast", "to_port": 0}),
            )],
            None,
        )
        .await
        .unwrap();
    check_ok(&res[0], "add_edge");

    let res = toolset
        .execute(&[build_tooluse("e4", "run_dag", json!({}))], None)
        .await
        .unwrap();
    check_ok(&res[0], "run_dag");

    let res = toolset
        .execute(
            &[build_tooluse(
                "e5",
                "get_output",
                json!({"id": "badcast", "limit": 50}),
            )],
            None,
        )
        .await
        .unwrap();
    // get_output returns an error result with a stable JSON envelope.
    assert!(
        res[0].is_error.unwrap_or(false),
        "failed-node get_output should be an error result: {:?}",
        res[0].content
    );
    let parsed = parse_tool_json(&res[0].content);
    assert_eq!(parsed["node"], json!("badcast"));
    let error = parsed["error"].as_str().unwrap();
    assert!(
        error.contains("failed during the last DAG run"),
        "unexpected get_output error: {error}"
    );
    assert!(parsed.get("outputs").is_none());
}

/// Synthetic baseline: a hand-built Struct column (via `named_struct`) does
/// NOT trigger obstacle #2 — `returned_rows` matches `total_rows`. This
/// narrows the VCF bug to the Dictionary/List-encoded INFO subfields, not to
/// Struct columns in general. Keep it as a control alongside the VCF test.
#[tokio::test]
async fn test_get_output_synthetic_struct_column_baseline() {
    let vfs = hermetic_vfs();
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let csv_path =
        std::path::Path::new(&manifest_dir).join("../data-engine/test_datasets/insurance.csv");
    let csv_data = std::fs::read(csv_path).unwrap();
    put_fixture(&vfs, "/insurance.csv", &csv_data);

    let engine = DataEngine::builder()
        .register_opendal_fs(vfs.storage.clone())
        .unwrap()
        .with_vfs((*vfs.mounted).clone())
        .build();
    let (client, _handle) = spawn_with_engine(engine);
    let tools = data_engine_tools::registrations(Arc::new(client.clone()));
    let mut registry = agentik_core::tools::ToolRegistry::new();
    registry.register_all(tools).unwrap();
    let toolset = Toolset::from_registry(Arc::new(registry), None);

    let res = toolset
        .execute(
            &[build_tooluse(
                "b1",
                "add_node",
                json!({"id": "src", "kind": "file_to_dataframe", "spec": {"path": "/insurance.csv"}}),
            )],
            None,
        )
        .await
        .unwrap();
    check_ok(&res[0], "add_node source");

    // Struct column via named_struct — plain Struct(Float64, Float64), no
    // Dictionary/List encoding. Baseline that should always work.
    let res = toolset
        .execute(
            &[build_tooluse(
                "b2",
                "add_node",
                json!({
                    "id": "struct_node",
                    "kind": "sql",
                    "spec": {"sql_query": "SELECT age, named_struct('lo', 0.0, 'hi', charges) AS bounds FROM port_0 WHERE age > 30 LIMIT 5"}
                }),
            )],
            None,
        )
        .await
        .unwrap();
    check_ok(&res[0], "add_node sql (synthetic struct)");

    let res = toolset
        .execute(
            &[build_tooluse(
                "b3",
                "add_edge",
                json!({"from": "src", "from_port": 0, "to": "struct_node", "to_port": 0}),
            )],
            None,
        )
        .await
        .unwrap();
    check_ok(&res[0], "add_edge");

    let res = toolset
        .execute(&[build_tooluse("b4", "run_dag", json!({}))], None)
        .await
        .unwrap();
    check_ok(&res[0], "run_dag");

    let res = toolset
        .execute(
            &[build_tooluse(
                "b5",
                "get_output",
                json!({"id": "struct_node", "limit": 50}),
            )],
            None,
        )
        .await
        .unwrap();
    check_ok(&res[0], "get_output struct_node");

    let parsed = parse_tool_json(&res[0].content);
    let entry = &parsed["outputs"][0];
    let total_rows = entry["total_rows"].as_u64().unwrap() as usize;
    let returned_rows = entry["returned_rows"].as_u64().unwrap() as usize;
    assert!(total_rows > 0, "baseline: expected total_rows > 0");
    assert_eq!(
        total_rows, returned_rows,
        "baseline synthetic struct should never hit obstacle #2"
    );
}

/// `inspect_node` returns the live `(kind, spec)` stored on a node instance.
///
/// This is the instance-level counterpart of `get_node_spec` (which returns a
/// kind's parameter *schema*). The test verifies:
///   1. After `add_node`, `inspect_node` returns the exact spec that was passed.
///   2. After `update_node`, `inspect_node` reflects the updated spec.
///   3. `inspect_node` on a non-existent id returns an error hint (not a crash
///      and not a silent null).
#[tokio::test]
async fn test_inspect_node_returns_live_spec() {
    let vfs = hermetic_vfs();
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let csv_path =
        std::path::Path::new(&manifest_dir).join("../data-engine/test_datasets/insurance.csv");
    let csv_data = std::fs::read(csv_path).unwrap();
    put_fixture(&vfs, "/insurance.csv", &csv_data);

    let engine = DataEngine::builder()
        .register_opendal_fs(vfs.storage.clone())
        .unwrap()
        .with_vfs((*vfs.mounted).clone())
        .build();
    let (client, _handle) = spawn_with_engine(engine);

    let tools = data_engine_tools::registrations(Arc::new(client.clone()));
    let mut registry = agentik_core::tools::ToolRegistry::new();
    registry.register_all(tools).unwrap();
    let toolset = Toolset::from_registry(Arc::new(registry), None);

    // 1. Add a SQL node with a known spec.
    let original_query = "SELECT age, charges FROM port_0 WHERE age > 30 LIMIT 5";
    let results = toolset
        .execute(
            &[build_tooluse(
                "tc1",
                "add_node",
                json!({
                    "id": "sql",
                    "kind": "sql",
                    "spec": {"sql_query": original_query}
                }),
            )],
            None,
        )
        .await
        .unwrap();
    check_ok(&results[0], "add_node sql");

    // 2. inspect_node must return the exact kind + spec.
    let results = toolset
        .execute(
            &[build_tooluse("tc2", "inspect_node", json!({"id": "sql"}))],
            None,
        )
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    check_ok(&results[0], "inspect_node sql");
    let body = parse_tool_json(&results[0].content);
    assert_eq!(body["id"], "sql", "inspect_node echoed the queried id");
    assert_eq!(body["kind"], "sql", "inspect_node returned the node kind");
    assert_eq!(
        body["spec"]["sql_query"], original_query,
        "inspect_node returned the original spec before update"
    );

    // 3. Update the spec; inspect_node must reflect the change.
    let updated_query = "SELECT age FROM port_0 LIMIT 10";
    let results = toolset
        .execute(
            &[build_tooluse(
                "tc3",
                "update_node",
                json!({"id": "sql", "spec": {"sql_query": updated_query}}),
            )],
            None,
        )
        .await
        .unwrap();
    check_ok(&results[0], "update_node sql");

    let results = toolset
        .execute(
            &[build_tooluse("tc4", "inspect_node", json!({"id": "sql"}))],
            None,
        )
        .await
        .unwrap();
    check_ok(&results[0], "inspect_node sql after update");
    let body = parse_tool_json(&results[0].content);
    assert_eq!(
        body["spec"]["sql_query"], updated_query,
        "inspect_node reflects the updated spec, not the original"
    );
    assert_eq!(
        body["kind"], "sql",
        "inspect_node still reports the same kind after update"
    );

    // 4. inspect_node on a non-existent id returns an error hint, not a crash.
    let results = toolset
        .execute(
            &[build_tooluse(
                "tc5",
                "inspect_node",
                json!({"id": "does_not_exist"}),
            )],
            None,
        )
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert!(
        results[0].is_error.unwrap_or(false),
        "inspect_node on a missing id should return an error result, got: {:?}",
        results[0].content
    );
}

/// Normalize the untagged `ToolResultContent` enum (Text | Json | Blocks) to
/// a parsed `serde_json::Value`. `get_output` emits `success_json` (→ Json),
/// but we handle all variants so the helper is robust.
fn parse_tool_json(content: &agentik_sdk::types::tools::ToolResultContent) -> serde_json::Value {
    use agentik_sdk::types::tools::{ToolResultBlock, ToolResultContent};
    match content {
        ToolResultContent::Json(v) => v.clone(),
        ToolResultContent::Text(s) => serde_json::from_str(s)
            .unwrap_or_else(|e| panic!("tool Text content wasn't valid JSON: {e}; body: {s}")),
        ToolResultContent::Blocks(blocks) => {
            let text: String = blocks
                .iter()
                .filter_map(|b| match b {
                    ToolResultBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            serde_json::from_str(&text).unwrap_or_else(|e| {
                panic!("tool Blocks content wasn't valid JSON: {e}; body: {text}")
            })
        }
    }
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
            path: "/".into(),
            backend: "default".into(),
            source: mounted_root.path().to_string_lossy().to_string(),
            read_only: false,
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

    // Minimal pipeline: source -> sql (sequential calls, matching the
    // established e2e flow).
    let steps: Vec<(&str, &str, serde_json::Value)> = vec![
        (
            "a1",
            "add_node",
            json!({"id": "src", "kind": "file_to_dataframe", "spec": {"path": "/insurance.csv"}}),
        ),
        (
            "a2",
            "add_node",
            json!({"id": "sql", "kind": "sql", "spec": {"sql_query": "SELECT age FROM port_0 LIMIT 3"}}),
        ),
        (
            "a3",
            "add_edge",
            json!({"from": "src", "from_port": 0, "to": "sql", "to_port": 0}),
        ),
    ];
    for (call, name, input) in steps {
        let results = toolset
            .execute(&[build_tooluse(call, name, input)], None)
            .await
            .unwrap();
        check_ok(&results[0], name);
    }

    // Two executions through the tool — the audit path under test.
    for call in ["r1", "r2"] {
        let results = toolset
            .execute(
                &[build_tooluse(
                    call,
                    "run_dag",
                    json!({"commit_message": "audit e2e"}),
                )],
                None,
            )
            .await
            .unwrap();
        check_ok(&results[0], "run_dag");
        let report = result_json(&results[0]);
        assert_eq!(report["ok"], json!(true), "run failed: {report}");
    }

    // Both executions left a run row, attributed to the session that ran
    // them, against a single deduplicated snapshot.
    let runs = client.dag_runs_log(None, 10, None).await.unwrap();
    assert_eq!(runs.len(), 2, "every execution leaves a run row");
    assert_eq!(runs[0].trigger.as_deref(), Some("agent:default"));
    assert_eq!(runs[1].trigger.as_deref(), Some("agent:default"));
    assert!(runs[0].snapshot_id.is_some());
    assert_eq!(runs[0].snapshot_id, runs[1].snapshot_id);
    assert!(runs.iter().all(|run| run.ok));
    assert_eq!(runs[0].message.as_deref(), Some("audit e2e"));

    // The listing tool surfaces the audit trail as text.
    let results = toolset
        .execute(
            &[build_tooluse("l1", "dag_runs_log", json!({"limit": 5}))],
            None,
        )
        .await
        .unwrap();
    check_ok(&results[0], "dag_runs_log");

    // The detail view decodes the per-node report, including the input
    // bindings the scheduler captured for the sql node.
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
    let sql_node = detail["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == json!("sql"))
        .expect("sql node in run detail");
    assert_eq!(
        sql_node["inputs"][0]["from"],
        json!("src"),
        "scheduler-captured input binding is surfaced"
    );
}

#[tokio::test]
async fn test_dag_export_run_produces_evidence_crate() {
    // Same mounted-VFS arrangement as the audit-trail test, plus a file sink
    // so the run actually produces packaged artifacts.
    let mounted_root = tempfile::tempdir().unwrap();
    let data_root = tempfile::tempdir().unwrap();
    let manifest = VfsManifest {
        backend: vec![BackendDefinition {
            id: "default".into(),
            config: BackendConfig::local("/"),
        }],
        mount: vec![MountDefinition {
            path: "/".into(),
            backend: "default".into(),
            source: mounted_root.path().to_string_lossy().to_string(),
            read_only: false,
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

    let steps: Vec<(&str, &str, serde_json::Value)> = vec![
        (
            "x1",
            "add_node",
            json!({"id": "src", "kind": "file_to_dataframe", "spec": {"path": "/insurance.csv"}}),
        ),
        (
            "x2",
            "add_node",
            json!({"id": "sink", "kind": "dataframe_to_file",
                   "spec": {"path": "/exports/out.csv", "format": "csv", "mode": "overwrite"}}),
        ),
        (
            "x3",
            "add_edge",
            json!({"from": "src", "from_port": 0, "to": "sink", "to_port": 0}),
        ),
    ];
    for (call, name, input) in steps {
        let results = toolset
            .execute(&[build_tooluse(call, name, input)], None)
            .await
            .unwrap();
        check_ok(&results[0], name);
    }
    let results = toolset
        .execute(&[build_tooluse("xr", "run_dag", json!({}))], None)
        .await
        .unwrap();
    check_ok(&results[0], "run_dag");

    // Export the run through the agent tool. out_dir "/" is VFS-visible (the
    // mount covers "/"), so the crate is uploaded into the object store and
    // stays readable through the same VFS the agent sees.
    let results = toolset
        .execute(
            &[build_tooluse(
                "xe",
                "dag_export_run",
                json!({"run_id": "", "format": "crate", "out_dir": "/"}),
            )],
            None,
        )
        .await
        .unwrap();
    check_ok(&results[0], "dag_export_run");
    let summary = result_json(&results[0]);
    let exported: Vec<&str> = summary["files"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|file| file["exported_path"].as_str())
        .collect();
    assert!(
        exported
            .iter()
            .any(|path| path.ends_with("exports/out.csv")),
        "sink output packaged: {exported:?}"
    );
    assert!(
        exported
            .iter()
            .any(|path| path.ends_with("workflow/manifest.json")),
        "snapshot manifest packaged: {exported:?}"
    );
    // The crate manifest is readable through the VFS (the tempdir out path
    // sits under the test mount, so the export routes through the store).
    let length = file_storage
        .content_length("/ro-crate-metadata.json")
        .await
        .unwrap_or_default();
    assert!(length > 0, "crate manifest uploaded");
    let manifest_bytes = file_storage
        .read_range("/ro-crate-metadata.json", 0..length)
        .await
        .unwrap();
    let crate_doc: serde_json::Value = serde_json::from_slice(&manifest_bytes.to_vec()).unwrap();
    let actions = crate_doc["@graph"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["@type"] == json!("CreateAction"))
        .count();
    assert_eq!(actions, 2, "one CreateAction per node");
    // PROV export through the same tool.
    let results = toolset
        .execute(
            &[build_tooluse(
                "xp",
                "dag_export_run",
                json!({"run_id": "", "format": "prov", "out_dir": "/"}),
            )],
            None,
        )
        .await
        .unwrap();
    check_ok(&results[0], "dag_export_run prov");
    let prov_summary = result_json(&results[0]);
    let prov_uri = prov_summary["out"].as_str().unwrap();
    assert!(
        prov_uri.starts_with("vfs://"),
        "prov upload is vfs-addressed"
    );
    let vpath = prov_uri.strip_prefix("vfs://").unwrap();
    let length = file_storage.content_length(vpath).await.unwrap();
    let prov_bytes = file_storage.read_range(vpath, 0..length).await.unwrap();
    let prov: serde_json::Value = serde_json::from_slice(&prov_bytes.to_vec()).unwrap();
    assert!(prov["activity"].as_object().unwrap().len() >= 2);
    // PROV semantics pinned here too: no duplicate generations.
    let generated = prov["wasGeneratedBy"].as_array().unwrap();
    let mut seen = std::collections::BTreeSet::new();
    for entry in generated {
        let key = (
            entry["entity"].as_str().unwrap_or(""),
            entry["activity"].as_str().unwrap_or(""),
        );
        assert!(seen.insert(key), "duplicate wasGeneratedBy: {key:?}");
    }
}

#[tokio::test]
async fn test_dag_export_run_uploads_into_vfs() {
    // P2 regression: an agent-visible (mounted) out_dir must route through
    // the object store, not the host filesystem.
    let mounted_root = tempfile::tempdir().unwrap();
    let data_root = tempfile::tempdir().unwrap();
    let manifest = VfsManifest {
        backend: vec![BackendDefinition {
            id: "default".into(),
            config: BackendConfig::local("/"),
        }],
        mount: vec![MountDefinition {
            path: "/".into(),
            backend: "default".into(),
            source: mounted_root.path().to_string_lossy().to_string(),
            read_only: false,
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

    for (call, name, input) in [
        (
            "v1",
            "add_node",
            json!({"id": "src", "kind": "file_to_dataframe", "spec": {"path": "/insurance.csv"}}),
        ),
        (
            "v2",
            "add_node",
            json!({"id": "sink", "kind": "dataframe_to_file",
                   "spec": {"path": "/vfs-exports/out.csv", "format": "csv", "mode": "overwrite"}}),
        ),
        (
            "v3",
            "add_edge",
            json!({"from": "src", "from_port": 0, "to": "sink", "to_port": 0}),
        ),
    ] {
        let results = toolset
            .execute(&[build_tooluse(call, name, input)], None)
            .await
            .unwrap();
        check_ok(&results[0], name);
    }
    let results = toolset
        .execute(&[build_tooluse("vr", "run_dag", json!({}))], None)
        .await
        .unwrap();
    check_ok(&results[0], "run_dag");

    // `latest` + a mounted (agent-visible) out path.
    let results = toolset
        .execute(
            &[build_tooluse(
                "ve",
                "dag_export_run",
                json!({"run_id": "latest", "format": "crate", "out_dir": "/evidence/crate"}),
            )],
            None,
        )
        .await
        .unwrap();
    check_ok(&results[0], "dag_export_run");
    let summary = result_json(&results[0]);
    assert_eq!(summary["out"], json!("vfs:///evidence/crate"));
    let paths: Vec<&str> = summary["files"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|file| file["exported_path"].as_str())
        .collect();
    assert!(
        paths
            .iter()
            .all(|path| path.starts_with("vfs:///evidence/crate/")),
        "all uploaded paths are vfs-addressed: {paths:?}"
    );
    assert!(
        paths
            .iter()
            .any(|path| path.ends_with("ro-crate-metadata.json")),
        "crate manifest uploaded: {paths:?}"
    );
    // The crate is readable back through the same VFS the agent sees.
    let metadata = file_storage
        .content_length("/evidence/crate/ro-crate-metadata.json")
        .await
        .unwrap();
    assert!(metadata > 0);
}

/// End-to-end PROV export: the recorded run carries its node specs (from
/// the executed manifest), the wiring edges with ports, and the dispatch
/// order — read back through the VFS the agent sees.
#[tokio::test]
async fn test_dag_export_run_prov_carries_specs_edges_and_order() {
    let mounted_root = tempfile::tempdir().unwrap();
    let data_root = tempfile::tempdir().unwrap();
    let manifest = VfsManifest {
        backend: vec![BackendDefinition {
            id: "default".into(),
            config: BackendConfig::local("/"),
        }],
        mount: vec![MountDefinition {
            path: "/".into(),
            backend: "default".into(),
            source: mounted_root.path().to_string_lossy().to_string(),
            read_only: false,
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

    for (call, name, input) in [
        (
            "v1",
            "add_node",
            json!({"id": "src", "kind": "file_to_dataframe", "spec": {"path": "/insurance.csv"}}),
        ),
        (
            "v2",
            "add_node",
            json!({"id": "sink", "kind": "dataframe_to_file",
                   "spec": {"path": "/vfs-exports/out.csv", "format": "csv", "mode": "overwrite"}}),
        ),
        (
            "v3",
            "add_edge",
            json!({"from": "src", "from_port": 0, "to": "sink", "to_port": 0}),
        ),
    ] {
        let results = toolset
            .execute(&[build_tooluse(call, name, input)], None)
            .await
            .unwrap();
        check_ok(&results[0], name);
    }
    let results = toolset
        .execute(&[build_tooluse("vr", "run_dag", json!({}))], None)
        .await
        .unwrap();
    check_ok(&results[0], "run_dag");

    let results = toolset
        .execute(
            &[build_tooluse(
                "ve",
                "dag_export_run",
                json!({"run_id": "latest", "format": "prov", "out_dir": "/evidence/prov"}),
            )],
            None,
        )
        .await
        .unwrap();

    check_ok(&results[0], "dag_export_run");
    let summary = result_json(&results[0]);
    assert!(
        summary["out"]
            .as_str()
            .unwrap()
            .starts_with("vfs:///evidence/prov/prov-"),
        "single-file export lands at its vfs address: {}",
        summary["out"]
    );

    // Read the document back through the same VFS the agent sees.
    let prov_vpath = summary["files"][0]["exported_path"]
        .as_str()
        .unwrap()
        .strip_prefix("vfs://")
        .unwrap()
        .to_string();
    let length = file_storage.content_length(&prov_vpath).await.unwrap();
    let bytes = file_storage
        .read_range(&prov_vpath, 0..length)
        .await
        .unwrap()
        .to_bytes();
    let doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    let run_id = summary["run_id"].as_str().unwrap();
    let src_act = format!("urn:autonomics:run:{run_id}:node:src");
    let sink_act = format!("urn:autonomics:run:{run_id}:node:sink");

    // Specs from the executed manifest, as compact JSON literals.
    let src_spec: serde_json::Value = serde_json::from_str(
        doc["activity"][src_act.as_str()]["autonomics:spec"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(src_spec["path"], json!("/insurance.csv"));
    let sink_spec: serde_json::Value = serde_json::from_str(
        doc["activity"][sink_act.as_str()]["autonomics:spec"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(sink_spec["format"], json!("csv"));

    // Dispatch order recorded end-to-end: the source runs before its sink.
    let src_seq = doc["activity"][src_act.as_str()]["autonomics:dispatch_seq"]
        .as_u64()
        .unwrap();
    let sink_seq = doc["activity"][sink_act.as_str()]["autonomics:dispatch_seq"]
        .as_u64()
        .unwrap();
    assert!(src_seq < sink_seq);

    // Wiring edge with ports, at the node level.
    assert!(
        doc["wasInformedBy"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["activity"] == json!(sink_act)
                && r["informed"] == json!(src_act)
                && r["prov:role"] == json!("0>0")),
        "declared edge lands as wasInformedBy with its ports"
    );
}
