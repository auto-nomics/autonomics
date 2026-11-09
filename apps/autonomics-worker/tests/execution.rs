#![cfg(target_os = "linux")]

use datafusion::arrow::array::Int64Array;
use datafusion::arrow::ipc::reader::FileReader;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::process::{Command, Output};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_autonomics-worker")
}

fn identity() -> Value {
    let output = Command::new(binary()).arg("identity").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn dispatch(kind: &str, spec: Value, output_type: &str) -> Value {
    let identity = identity();
    json!({
        "spec": {
            "id":"worker-smoke", "kind":"process", "inputs":[],
            "outputs":[{"name":"result", "port":0, "payload":output_type}],
            "spec":{"typed_dispatch":{
                "protocol":1,
                "worker":{
                    "executable":binary(), "executable_sha256":identity["executable_sha256"],
                    "expected_source_revision":dag_core::source_revision()
                },
                "task":{
                    "id":"worker-smoke", "kind":kind, "spec":spec, "inputs":[],
                    "outputs":[{"name":"result", "port":0, "payload":output_type}]
                },
                "input_files":{}, "bundles":[], "plugin_identity":null
            }}
        },
        "resources":{}
    })
}

fn execute(workspace: &Path, dispatch: &Value, paths: &Value, artifacts: &Value) -> Output {
    let manifest = workspace.join("task.json");
    std::fs::write(&manifest, serde_json::to_vec(dispatch).unwrap()).unwrap();
    Command::new(binary())
        .args(["task-worker", "--dispatch"])
        .arg(&manifest)
        .env("AUTONOMICS_WORKSPACE", workspace)
        .env("AUTONOMICS_TASK_ID", "worker-smoke")
        .env("AUTONOMICS_INPUTS", paths.to_string())
        .env("AUTONOMICS_INPUT_ARTIFACTS", artifacts.to_string())
        .env_remove("SLURM_JOB_ID")
        .output()
        .unwrap()
}

#[test]
fn identity_matches_binary_and_rejects_incompatible_deployments() {
    let report = identity();
    let measured = format!("{:x}", Sha256::digest(std::fs::read(binary()).unwrap()));
    let mut identity: dag_core::dag::execution::typed_worker::WorkerIdentity =
        serde_json::from_value(report).unwrap();
    identity.validate(&measured).unwrap();
    identity.protocol += 1;
    assert!(identity.validate(&measured).is_err());
    identity.protocol = 1;
    identity.source_revision = "different-revision".into();
    assert!(identity.validate(&measured).is_err());
}

#[test]
fn standalone_worker_executes_sql_and_writes_complete_arrow_result() {
    let workspace = tempfile::tempdir().unwrap();
    let task = dispatch(
        "sql",
        json!({"sql_query":"SELECT CAST(42 AS BIGINT) AS answer"}),
        "dataframe",
    );
    let output = execute(workspace.path(), &task, &json!({}), &json!({}));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let file =
        std::fs::File::open(workspace.path().join("typed-outputs/port-0/value.arrow")).unwrap();
    let batch = FileReader::try_new(file, None)
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
    assert_eq!(
        batch
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap()
            .value(0),
        42
    );
    assert!(!workspace.path().join("history.db").exists());
}

#[test]
fn verified_file_input_roundtrips_and_tampered_input_is_rejected() {
    for tampered in [false, true] {
        let workspace = tempfile::tempdir().unwrap();
        let source = workspace.path().join("input.txt");
        let bytes = b"analysis input";
        std::fs::write(&source, bytes).unwrap();
        let artifacts = json!({"files":[{
            "path":source, "format":"txt", "fingerprint":{
                "size":bytes.len(), "mtime_ns":0, "immutable_remote":false,
                "content_hash":format!("{:x}", Sha256::digest(bytes))
            }
        }]});
        let mut task = dispatch("file_set_select", json!({"index":0}), "file");
        let binding = json!({
            "name":"files", "port":0, "payload":"file_set",
            "source":{"UpstreamPort":{"from":"source", "from_port":0}}
        });
        task["spec"]["inputs"] = json!([binding]);
        task["spec"]["spec"]["typed_dispatch"]["task"]["inputs"] = json!([binding]);
        task["spec"]["spec"]["_autonomics_input_artifacts"] = artifacts.clone();
        if tampered {
            std::fs::write(&source, b"analysis wrong").unwrap();
        }
        let output = execute(
            workspace.path(),
            &task,
            &json!({"files":[source]}),
            &artifacts,
        );
        if tampered {
            assert!(!output.status.success());
            assert!(String::from_utf8_lossy(&output.stderr).contains("SHA256"));
            assert!(!workspace.path().join("typed-outputs").exists());
        } else {
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                std::fs::read(workspace.path().join("typed-outputs/port-0/00000000.txt")).unwrap(),
                bytes
            );
        }
    }
}

#[test]
fn wrong_worker_hash_or_source_revision_fails_before_execution() {
    for key in ["executable_sha256", "expected_source_revision"] {
        let workspace = tempfile::tempdir().unwrap();
        let mut task = dispatch("sql", json!({"sql_query":"SELECT 1"}), "dataframe");
        task["spec"]["spec"]["typed_dispatch"]["worker"][key] = if key == "executable_sha256" {
            "0".repeat(64)
        } else {
            "other-source".into()
        }
        .into();
        let output = execute(workspace.path(), &task, &json!({}), &json!({}));
        assert!(!output.status.success());
        assert!(!workspace.path().join("typed-inputs").exists());
    }
}
