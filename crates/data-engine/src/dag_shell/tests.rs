use std::sync::Arc;
use std::time::{Duration, Instant};

use super::*;

fn sandbox() -> (Arc<NodeRegistry>, DagShellSnapshot) {
    let engine = crate::data_engine::DataEngine::builder().build();
    let registry = Arc::clone(engine.node_registry());
    let snapshot = DagShellSnapshot::from_dag(engine.dag());
    (registry, snapshot)
}

fn eval(script: &str) -> Result<ScriptExecution, DagShellError> {
    let (registry, snapshot) = sandbox();
    execute_script(script, Duration::from_millis(1000), &snapshot, &registry)
}

fn result_json(script: &str) -> serde_json::Value {
    let execution = eval(script).expect("script should evaluate");
    serde_json::from_str(&execution.result).expect("result should be JSON")
}

#[test]
fn file_channel_type_option_survives_shell_translation() {
    for source in ["from_path", "from_file_pairs"] {
        let script = format!("{source}('input', 'data/*', type='dir')\ncommit()\n");
        let execution = eval(&script).unwrap();
        let GraphEditOp::AddNode { spec, .. } = &execution.operations[0] else {
            panic!("expected source");
        };
        assert_eq!(spec["pathType"], "dir");
        let parsed: dag_engine::SourceSpec = serde_json::from_value(spec.clone()).unwrap();
        assert!(matches!(
            parsed,
            dag_engine::SourceSpec::Glob {
                path_type: dag_engine::model::PathType::Dir,
                ..
            } | dag_engine::SourceSpec::FilePairs {
                path_type: dag_engine::model::PathType::Dir,
                ..
            }
        ));
        let bad = format!("{source}('input', 'data/*', type='device')\ncommit()\n");
        assert!(eval(&bad).is_err());
    }
}

#[test]
fn channel_function_uses_captures_and_cannot_edit_graph() {
    let (registry, snapshot) = sandbox();
    let captures = serde_json::json!({"factor": 3})
        .as_object()
        .unwrap()
        .clone();
    let result = interp::run_pure_function(
        "def transform(item):\n    return item * factor\n",
        &captures,
        &[serde_json::json!(7)],
        &snapshot,
        &registry,
    )
    .unwrap();
    assert_eq!(result, serde_json::json!(21));
    let error = interp::run_pure_function(
        "def transform(item):\n    commit()\n    return item\n",
        &captures,
        &[serde_json::json!(7)],
        &snapshot,
        &registry,
    )
    .unwrap_err();
    assert!(error.message.contains("graph operations are not available"));
}

#[test]
fn channel_function_preserves_file_metadata_when_returned() {
    use dag_core::value::{FileFingerprint, FileRef, NodeValue};
    let (registry, snapshot) = sandbox();
    let input = NodeValue::File(FileRef {
        path: "/tmp/sample.fastq".into(),
        format: Some("fastq".into()),
        fingerprint: Some(FileFingerprint {
            size: 5,
            mtime_ns: 10,
            content_hash: Some("hash".into()),
            immutable_remote: false,
        }),
    });
    let output = interp::run_pure_node_function(
        "def identity(item):\n    return item\n",
        &serde_json::Map::new(),
        None,
        Some(&input),
        None,
        &snapshot,
        &registry,
    )
    .unwrap();
    let NodeValue::File(file) = output else {
        panic!("expected a file")
    };
    assert_eq!(file.path, "/tmp/sample.fastq");
    assert_eq!(file.format.as_deref(), Some("fastq"));
    assert_eq!(
        file.fingerprint.unwrap().content_hash.as_deref(),
        Some("hash")
    );
}

#[test]
fn channel_function_preserves_file_sets_and_their_fingerprints() {
    use dag_core::value::{FileFingerprint, FileRef, NodeValue};
    let (registry, snapshot) = sandbox();
    let fingerprint = FileFingerprint {
        size: 5,
        mtime_ns: 10,
        content_hash: Some("hash".into()),
        immutable_remote: false,
    };
    let input = NodeValue::FileSet(vec![FileRef {
        path: "/tmp/sample.fastq".into(),
        format: Some("fastq".into()),
        fingerprint: Some(fingerprint.clone()),
    }]);
    let output = interp::run_pure_node_function(
        "def copy(item):\n    return item[:]\n",
        &serde_json::Map::new(),
        None,
        Some(&input),
        None,
        &snapshot,
        &registry,
    )
    .unwrap();
    let NodeValue::FileSet(files) = output else {
        panic!("expected a file set")
    };
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].fingerprint.as_ref(), Some(&fingerprint));
}

#[test]
fn node_spec_persists_a_channel_function_and_its_captures() {
    let script = "factor = 3\ndef transform(item):\n    return item * factor\nnode('mapped', 'channel', {'operator': 'map', 'function': transform})\ncommit()\n";
    let execution = eval(script).unwrap();
    let GraphEditOp::AddNode { spec, .. } = &execution.operations[0] else {
        panic!("expected node")
    };
    let function = &spec["function"]["__dag_shell_function__"];
    assert_eq!(function["captures"]["factor"], 3);
    let (registry, snapshot) = sandbox();
    let result = interp::run_pure_function(
        function["source"].as_str().unwrap(),
        function["captures"].as_object().unwrap(),
        &[serde_json::json!(7)],
        &snapshot,
        &registry,
    )
    .unwrap();
    assert_eq!(result, serde_json::json!(21));
}

#[test]
fn timeout_is_normalized() {
    assert_eq!(
        normalize_timeout(None).unwrap(),
        Duration::from_millis(DEFAULT_TIMEOUT_MS)
    );
    assert_eq!(
        normalize_timeout(Some(0)).unwrap_err().code,
        "invalid_operation"
    );
    assert_eq!(
        normalize_timeout(Some(MAX_TIMEOUT_MS + 1))
            .unwrap_err()
            .code,
        "invalid_operation"
    );
}

#[test]
fn rejects_oversized_script_before_compilation() {
    let engine = crate::data_engine::DataEngine::builder().build();
    let registry = Arc::clone(engine.node_registry());
    let snapshot = DagShellSnapshot::from_dag(engine.dag());
    let script = "x".repeat(MAX_SCRIPT_BYTES + 1);
    let error =
        execute_script(&script, Duration::from_millis(10), &snapshot, &registry).unwrap_err();
    assert_eq!(error.code, "script_too_large");
}

#[test]
fn times_out_unbounded_scripts() {
    let (registry, snapshot) = sandbox();
    let started = Instant::now();
    let error = execute_script(
        "while True:\n    pass\n",
        Duration::ZERO,
        &snapshot,
        &registry,
    )
    .unwrap_err();
    assert_eq!(error.code, "timeout");
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn limits_deep_recursion() {
    let (registry, snapshot) = sandbox();
    let error = execute_script(
        r#"
def recurse(n):
    recurse(n + 1)

recurse(0)
"#,
        Duration::from_millis(100),
        &snapshot,
        &registry,
    )
    .unwrap_err();
    assert_eq!(error.code, "limit_exceeded");
    assert!(error.message.contains("recursion"));
}

#[test]
fn parse_errors_carry_source_position() {
    let (registry, snapshot) = sandbox();
    let error = execute_script(
        "x = = 1\ncommit()\n",
        Duration::from_millis(100),
        &snapshot,
        &registry,
    )
    .unwrap_err();
    assert_eq!(error.code, "parse_error");
    assert_eq!(error.line, Some(1));
    assert!(error.column.unwrap_or_default() > 0);
}

#[test]
fn unregistered_host_functions_are_unavailable() {
    let (registry, snapshot) = sandbox();
    let error = execute_script(
        r#"read_file("/etc/passwd")"#,
        Duration::from_millis(100),
        &snapshot,
        &registry,
    )
    .unwrap_err();
    assert_eq!(error.code, "runtime_error");
    assert!(error.message.contains("read_file"));
    assert!(error.message.contains("not defined"));
}

#[test]
fn module_imports_are_disabled() {
    let (registry, snapshot) = sandbox();
    let error = execute_script(
        "import anything as secret\n",
        Duration::from_millis(100),
        &snapshot,
        &registry,
    )
    .unwrap_err();
    assert_eq!(error.code, "parse_error");
    assert!(error.message.contains("import"));
}

#[test]
fn statement_whitelist_rejects_unreachable_constructs() {
    for script in [
        "if False:\n    import os\n",
        "class Wrapper:\n    pass\n",
        "try:\n    pass\nexcept Exception:\n    pass\n",
        "with open('/tmp/x') as handle:\n    pass\n",
        "assert False\n",
        "raise ValueError('x')\n",
        "x: int = 1\n",
    ] {
        let (registry, snapshot) = sandbox();
        let error =
            execute_script(script, Duration::from_millis(100), &snapshot, &registry).unwrap_err();
        assert_eq!(error.code, "parse_error", "script: {script}");
    }
}

#[test]
fn expression_depth_is_bounded() {
    let (registry, snapshot) = sandbox();
    let script = format!("{}1{}", "(".repeat(128), ")".repeat(128));
    let error =
        execute_script(&script, Duration::from_millis(100), &snapshot, &registry).unwrap_err();
    assert!(matches!(
        error.code.as_str(),
        "parse_error" | "limit_exceeded"
    ));
}

#[test]
fn array_size_is_bounded() {
    let (registry, snapshot) = sandbox();
    let error = execute_script(
        r#"
values = []
for value in range(10001):
    values.append(value)
values
"#,
        Duration::from_millis(5000),
        &snapshot,
        &registry,
    )
    .unwrap_err();
    assert_eq!(error.code, "limit_exceeded");
}

#[test]
fn map_size_is_bounded() {
    let (registry, snapshot) = sandbox();
    let error = execute_script(
        r#"
values = {}
for value in range(10002):
    values[str(value)] = value
values
"#,
        Duration::from_millis(1000),
        &snapshot,
        &registry,
    )
    .unwrap_err();
    assert_eq!(error.code, "limit_exceeded");
}

#[test]
fn oversized_maps_cannot_be_staged() {
    let (registry, snapshot) = sandbox();
    let error = execute_script(
        r#"
values = {}
for value in range(10002):
    values[str(value)] = value
node("oversized", "echo", values)
commit()
"#,
        Duration::from_millis(5000),
        &snapshot,
        &registry,
    )
    .unwrap_err();
    assert_eq!(error.code, "limit_exceeded");
}

#[test]
fn api_operation_count_is_bounded() {
    let (registry, snapshot) = sandbox();
    let error = execute_script(
        r#"
for value in range(1002):
    node("node_" + str(value), "echo", {})
"#,
        Duration::from_millis(1000),
        &snapshot,
        &registry,
    )
    .unwrap_err();
    assert_eq!(error.code, "limit_exceeded");
}

#[test]
fn queries_use_the_execution_start_snapshot() {
    let mut engine = crate::data_engine::DataEngine::builder().build();
    engine
        .add_node_from_registry("source", "echo", serde_json::json!({}))
        .unwrap();
    let registry = Arc::clone(engine.node_registry());
    let snapshot = DagShellSnapshot::from_dag(engine.dag());
    let execution = execute_script(
        r#"
observed = [has_node("source"), has_node("staged"), graph_summary()["node_count"]]
node("staged", "echo", {})
commit()
observed
"#,
        Duration::from_millis(100),
        &snapshot,
        &registry,
    )
    .unwrap();
    let observed: serde_json::Value = serde_json::from_str(&execution.result).unwrap();
    assert_eq!(observed, serde_json::json!([true, false, 1]));
}

// ── Interpreter semantics ──────────────────────────────────────────────────

#[test]
fn arithmetic_and_precedence() {
    assert_eq!(result_json("2 + 3 * 4"), serde_json::json!(14));
    assert_eq!(result_json("7 / 2"), serde_json::json!(3.5));
    assert_eq!(result_json("7 // 2"), serde_json::json!(3));
    assert_eq!(result_json("-7 % 3"), serde_json::json!(2));
    assert_eq!(result_json("2 ** 10"), serde_json::json!(1024));
    assert_eq!(result_json("(1 + 2) * 3"), serde_json::json!(9));
}

#[test]
fn strings_and_f_strings() {
    let script = r#"
parts = []
for i in range(3):
    parts.append(f"node_{i}")
"-".join(parts)
"#;
    assert_eq!(
        result_json(script),
        serde_json::json!("node_0-node_1-node_2")
    );
    assert_eq!(
        result_json(r#""a,b,c".split(",")"#),
        serde_json::json!(["a", "b", "c"])
    );
    assert_eq!(
        result_json(r#"f"{1 + 1}={bool(2)}""#),
        serde_json::json!("2=True")
    );
    assert_eq!(
        result_json(r#""Hello".lower() + '!'"#),
        serde_json::json!("hello!")
    );
}

#[test]
fn comprehensions_and_generators() {
    assert_eq!(
        result_json("[str(x) for x in range(3)]"),
        serde_json::json!(["0", "1", "2"])
    );
    assert_eq!(
        result_json(r#"{k: len(k) for k in ["a", "bb"]}"#),
        serde_json::json!({"a": 1, "bb": 2})
    );
    assert_eq!(
        result_json("sum(x for x in range(4))"),
        serde_json::json!(6)
    );
    assert_eq!(
        result_json("[x * y for x in range(2) for y in range(2)]"),
        serde_json::json!([0, 0, 0, 1])
    );
}

#[test]
fn functions_with_defaults_and_kwargs() {
    let script = r#"
def add(a, b=2):
    return a + b

[add(1), add(1, b=5), add(1, 10)]
"#;
    assert_eq!(result_json(script), serde_json::json!([3, 6, 11]));
}

#[test]
fn unpacking_indices_and_slices() {
    assert_eq!(
        result_json("a, b = [1, 2]\n[b, a]"),
        serde_json::json!([2, 1])
    );
    assert_eq!(result_json(r#""hello"[-1]"#), serde_json::json!("o"));
    assert_eq!(result_json("[1, 2, 3][::-1]"), serde_json::json!([3, 2, 1]));
    assert_eq!(result_json(r#""hello"[1:4]"#), serde_json::json!("ell"));
    assert_eq!(result_json("{}.get('x', 7)"), serde_json::json!(7));
}

#[test]
fn comparisons_and_logic() {
    assert_eq!(result_json("0 < 2 < 3"), serde_json::json!(true));
    assert_eq!(result_json("1 < 0 < 3"), serde_json::json!(false));
    assert_eq!(result_json(r#"None or "d""#), serde_json::json!("d"));
    assert_eq!(result_json("x = None\nx is None"), serde_json::json!(true));
    assert_eq!(result_json(r#""ell" in "hello""#), serde_json::json!(true));
    assert_eq!(result_json("2 in range(3)"), serde_json::json!(true));
    assert_eq!(result_json("not []"), serde_json::json!(true));
}

#[test]
fn walrus_and_tuple_iteration() {
    assert_eq!(result_json("(n := 5) + n"), serde_json::json!(10));
    assert_eq!(
        result_json(
            "total = 0\nfor key, value in [(\"a\", 1), (\"b\", 2)]:\n    total += value\ntotal"
        ),
        serde_json::json!(3)
    );
    assert_eq!(
        result_json("list(enumerate(['a', 'b'], start=1))"),
        serde_json::json!([[1, "a"], [2, "b"]])
    );
}

#[test]
fn unsupported_expressions_fail_cleanly() {
    for script in [
        "{1, 2}",
        "[1, 2] | [3]",
        "lambda x: x",
        "'%d' % 5",
        "f'{x:>8}'",
        "__import__('os')",
        "().__class__",
    ] {
        let error = eval(script).unwrap_err();
        assert!(
            matches!(error.code.as_str(), "runtime_error" | "limit_exceeded"),
            "script `{script}`: {}",
            error.message
        );
    }
}

#[test]
fn unhashable_dict_keys_are_rejected() {
    let error = eval("{[1]: 2}").unwrap_err();
    assert_eq!(error.code, "runtime_error");
    assert!(error.message.contains("list"));
}

#[test]
fn staged_loop_build_matches_previous_contract() {
    let (registry, snapshot) = sandbox();
    let execution = execute_script(
        r#"
for sample in ["a", "b"]:
    node("echo_" + sample, "echo", {})
edge("echo_a", 0, "echo_b", 0)
commit()
"done"
"#,
        Duration::from_millis(1000),
        &snapshot,
        &registry,
    )
    .unwrap();
    assert_eq!(execution.operations.len(), 3);
    assert!(execution.committed);
    assert_eq!(execution.result, "\"done\"");
}

#[test]
fn staging_after_commit_is_rejected() {
    let error = eval("commit()\nnode(\"x\", \"echo\", {})\n").unwrap_err();
    assert_eq!(error.code, "runtime_error");
    assert!(error.message.contains("commit() was already called"));
}

#[test]
fn kwargs_cascade_unifies_overloads() {
    let (registry, snapshot) = sandbox();
    let execution = execute_script(
        r#"
node("a", "echo", {})
node("b", "echo", {})
commit()
"#,
        Duration::from_millis(1000),
        &snapshot,
        &registry,
    )
    .unwrap();
    assert_eq!(execution.operations.len(), 2);

    let removal = execute_script(
        r#"
remove_node("a", cascade=False)
remove_node("b", cascade=True)
commit()
"#,
        Duration::from_millis(1000),
        &snapshot,
        &registry,
    )
    .unwrap();
    let traces = operation_traces(&removal.operations);
    assert_eq!(traces[0].cascade, None);
    assert_eq!(traces[1].cascade, Some(true));
}
