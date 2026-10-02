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

fn result_json(result: &ToolResult) -> serde_json::Value {
    match &result.content {
        ToolResultContent::Json(value) => value.clone(),
        other => panic!("expected JSON tool result, got: {other:?}"),
    }
}

#[tokio::test]
async fn test_add_source_sql_run_dag() {
    // 1. Set up file storage and write test data
    let file_storage = Arc::new(OpendalFileStorage::new("/mnt/disk3/test"));
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let csv_path =
        std::path::Path::new(&manifest_dir).join("../data-engine/test_datasets/insurance.csv");
    let csv_data = std::fs::read(csv_path).unwrap();
    file_storage
        .op
        .write("/insurance.csv", csv_data)
        .await
        .unwrap();

    // 2. Build DataEngine and spawn server
    let engine = DataEngine::builder()
        .register_opendal_fs(file_storage)
        .unwrap()
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
    let file_storage = Arc::new(OpendalFileStorage::new("/mnt/disk3/test"));
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let vcf_path =
        std::path::Path::new(&manifest_dir).join("../data-engine/test_datasets/sample.vcf.gz");
    let vcf_data = std::fs::read(vcf_path).unwrap();
    file_storage
        .op
        .write("/sample.vcf.gz", vcf_data)
        .await
        .unwrap();

    let engine = DataEngine::builder()
        .register_opendal_fs(file_storage)
        .unwrap()
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

/// Obstacle #2 fix verification: when `collect()` of the limited plan errors
/// at runtime but `count()` (a separate, column-eliminated plan) succeeds,
/// `get_output` MUST surface the real error in-band (`collect_error`) instead
/// of the old behavior — silently swallowing it via `unwrap_or_default()` and
/// reporting the misleading `returned_rows: 0, total_rows: N`.
///
/// We force exactly that divergence with a runtime-erroring projection:
/// `SELECT cast(s AS int) ...` over a string column that holds non-numeric
/// values. `count(*)` eliminates the unused (and erroring) cast, so it
/// returns 5; the `SELECT *` collect materializes the cast and errors.
///
/// Before the fix this produced `total_rows=5, returned_rows=0` (silent
/// swallow). After the fix it produces `total_rows=5, returned_rows=null,
/// collect_error="<msg>"` — the agent sees the real failure.
#[tokio::test]
async fn test_get_output_surfaces_collect_error_instead_of_swallowing() {
    let file_storage = Arc::new(OpendalFileStorage::new("/mnt/disk3/test"));
    // 5 rows where `s` is non-numeric → cast(s as int) errors at execution.
    let csv = b"age,s\n1,abc\n2,def\n3,ghi\n4,jkl\n5,mno\n";
    file_storage
        .op
        .write("/badcast.csv", csv.to_vec())
        .await
        .unwrap();

    let engine = DataEngine::builder()
        .register_opendal_fs(file_storage)
        .unwrap()
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

    // Runtime-erroring projection. `count(*)` over this typically eliminates
    // the cast (column unused), so it returns 5; `SELECT *` must materialize
    // the cast and fails.
    let res = toolset
        .execute(
            &[build_tooluse(
                "e2",
                "add_node",
                json!({
                    "id": "badcast",
                    "kind": "sql",
                    "spec": {"sql_query": "SELECT cast(s as int) AS n FROM port_0"}
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
    // NOTE: get_output returns success_json (stable envelope) but with a
    // per-output `collect_error` field instead of the old silent 0 rows.
    let parsed = parse_tool_json(&res[0].content);
    let entry = &parsed["outputs"][0];
    let total_rows = entry["total_rows"].as_u64().unwrap() as usize;
    let collect_error = entry["collect_error"].as_str();
    let returned_rows = &entry["returned_rows"];
    eprintln!(
        "[badcast get_output] total_rows={total_rows} returned_rows={returned_rows} collect_error={collect_error:?}"
    );

    // total_rows still comes from COUNT(*) which eliminated the cast → 5.
    assert!(
        total_rows > 0,
        "count(*) should still return the row count (cast eliminated); got {total_rows}."
    );
    // The fix: the collect error is surfaced in-band, not swallowed.
    assert!(
        collect_error.is_some(),
        "FIX REGRESSION: get_output must surface the collect error in \
         `collect_error`; got none. entry: {entry}"
    );
    // And returned_rows is now `null` (not a misleading 0).
    assert!(
        returned_rows.is_null(),
        "returned_rows must be null when collect errors (was silently 0 \
         before the fix); got {returned_rows}"
    );
}

/// Synthetic baseline: a hand-built Struct column (via `named_struct`) does
/// NOT trigger obstacle #2 — `returned_rows` matches `total_rows`. This
/// narrows the VCF bug to the Dictionary/List-encoded INFO subfields, not to
/// Struct columns in general. Keep it as a control alongside the VCF test.
#[tokio::test]
async fn test_get_output_synthetic_struct_column_baseline() {
    let file_storage = Arc::new(OpendalFileStorage::new("/mnt/disk3/test"));
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let csv_path =
        std::path::Path::new(&manifest_dir).join("../data-engine/test_datasets/insurance.csv");
    let csv_data = std::fs::read(csv_path).unwrap();
    file_storage
        .op
        .write("/insurance.csv", csv_data)
        .await
        .unwrap();

    let engine = DataEngine::builder()
        .register_opendal_fs(file_storage)
        .unwrap()
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
    let file_storage = Arc::new(OpendalFileStorage::new("/mnt/disk3/test"));
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let csv_path =
        std::path::Path::new(&manifest_dir).join("../data-engine/test_datasets/insurance.csv");
    let csv_data = std::fs::read(csv_path).unwrap();
    file_storage
        .op
        .write("/insurance.csv", csv_data)
        .await
        .unwrap();

    let engine = DataEngine::builder()
        .register_opendal_fs(file_storage)
        .unwrap()
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

/// Evidence channel end-to-end: two `file_reference` evidence inputs merge
/// (identifier dedup + note fill-in) and `get_output` renders the citations
/// inline instead of a bare path. Local absolute paths keep this test
/// independent of VFS mount resolution.
#[tokio::test]
async fn test_evidence_channel_merge_and_inline_render() {
    let dir = std::env::temp_dir().join(format!("evidence-e2e-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path_a = dir.join("set_a.json");
    let path_b = dir.join("set_b.json");
    let merged_path = dir.join("merged.json");

    let set_a = json!({
        "schema_version": 1,
        "records": [
            {"citation": {"id": "1", "title": "Alpha study", "year": 2023,
                          "authors": [{"last_name": "Smith"}],
                          "identifiers": [{"kind": "doi", "value": "10.1/shared"}]},
             "origin": "pubmed"},
            {"citation": {"id": "2", "title": "Beta study", "year": 2024,
                          "authors": [{"last_name": "Jones"}],
                          "identifiers": []},
             "note": "why beta matters"}
        ]
    });
    // Same DOI (URL-prefixed, different casing) plus one unique record: the
    // merge must collapse the duplicate and carry the duplicate's note into
    // the surviving Alpha record.
    let set_b = json!({
        "schema_version": 1,
        "records": [
            {"citation": {"id": "3", "title": "Alpha study (alt view)", "year": 2023,
                          "authors": [{"last_name": "Smith"}],
                          "identifiers": [{"kind": "doi", "value": "https://doi.org/10.1/SHARED"}]},
             "note": "duplicate view", "origin": "openalex"},
            {"citation": {"id": "4", "title": "Gamma study", "year": 2025,
                          "authors": [{"last_name": "Lee"}],
                          "identifiers": [{"kind": "doi", "value": "10.1/gamma"}]},
             "origin": "crossref"}
        ]
    });
    std::fs::write(&path_a, serde_json::to_vec(&set_a).unwrap()).unwrap();
    std::fs::write(&path_b, serde_json::to_vec(&set_b).unwrap()).unwrap();

    // Engine without VFS: local absolute paths only.
    let engine = DataEngine::builder().build();
    let (client, _handle) = spawn_with_engine(engine);
    let tools = data_engine_tools::registrations(Arc::new(client.clone()));
    let mut registry = agentik_core::tools::ToolRegistry::new();
    registry.register_all(tools).unwrap();
    let toolset = Toolset::from_registry(Arc::new(registry), None);

    let mut steps = vec![
        (
            "e1",
            "add_node",
            json!({"id": "ref_a", "kind": "file_reference",
                   "spec": {"path": path_a.to_str().unwrap(), "format": "evidence"}}),
        ),
        (
            "e2",
            "add_node",
            json!({"id": "ref_b", "kind": "file_reference",
                   "spec": {"path": path_b.to_str().unwrap(), "format": "evidence"}}),
        ),
        (
            "e3",
            "add_node",
            json!({"id": "merge", "kind": "evidence_merge",
                   "spec": {"path": merged_path.to_str().unwrap()}}),
        ),
        (
            "e4",
            "add_edge",
            json!({"from": "ref_a", "from_port": 0, "to": "merge", "to_port": 0}),
        ),
        (
            "e5",
            "add_edge",
            json!({"from": "ref_b", "from_port": 0, "to": "merge", "to_port": 1}),
        ),
        ("e6", "run_dag", json!({})),
    ];
    // Sequential: later steps depend on earlier ones.
    for (id, name, input) in steps.drain(..) {
        let res = toolset
            .execute(&[build_tooluse(id, name, input)], None)
            .await
            .unwrap();
        check_ok(&res[0], name);
    }

    // Merged artifact: 3 records (Alpha + Beta + Gamma), Alpha carries the
    // duplicate's note and its own origin.
    let merged: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&merged_path).unwrap()).unwrap();
    assert_eq!(merged["records"].as_array().unwrap().len(), 3);
    assert_eq!(merged["records"][0]["citation"]["title"], json!("Alpha study"));
    assert_eq!(merged["records"][0]["note"], json!("duplicate view"));
    assert_eq!(merged["records"][0]["origin"], json!("pubmed"));

    // get_output renders evidence inline: citation list, not a bare file.
    let res = toolset
        .execute(
            &[build_tooluse("e7", "get_output", json!({"id": "merge"}))],
            None,
        )
        .await
        .unwrap();
    check_ok(&res[0], "get_output merge");
    let output = result_json(&res[0]);
    let entry = &output["outputs"][0];
    assert_eq!(entry["type"], json!("evidence"));
    assert_eq!(entry["total"], json!(3));
    assert_eq!(entry["returned"], json!(3));
    let records = entry["records"].as_array().unwrap();
    assert_eq!(records[0]["cite"], json!("Smith, 2023"));
    assert_eq!(records[0]["doi"], json!("10.1/shared"));

    // Non-evidence files on variadic ports carry no wiring-time contract
    // (undeclared ports skip edge validation — dag-core semantics shared
    // with container_command), so the merge node itself rejects them at run
    // time. Wire an existing file declared as csv into a third input port.
    let res = toolset
        .execute(
            &[build_tooluse(
                "e8",
                "add_node",
                json!({"id": "ref_csv", "kind": "file_reference",
                       "spec": {"path": path_a.to_str().unwrap(), "format": "csv"}}),
            )],
            None,
        )
        .await
        .unwrap();
    check_ok(&res[0], "add_node csv ref");
    let res = toolset
        .execute(
            &[build_tooluse(
                "e9",
                "add_edge",
                json!({"from": "ref_csv", "from_port": 0, "to": "merge", "to_port": 2}),
            )],
            None,
        )
        .await
        .unwrap();
    check_ok(&res[0], "add_edge csv -> merge (wiring is permissive)");
    let res = toolset
        .execute(&[build_tooluse("e10", "run_dag", json!({}))], None)
        .await
        .unwrap();
    check_ok(&res[0], "run_dag with bad input");
    let report = result_json(&res[0]);
    assert_eq!(report["ok"], json!(false), "run must fail with a csv input");
    let merge_node = report["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == json!("merge"))
        .expect("merge node in run report");
    assert_eq!(merge_node["status"], json!("failed"));
    let message = merge_node["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("expected `evidence`"),
        "run-time format rejection message, got: {message}"
    );

    std::fs::remove_dir_all(&dir).ok();
}
