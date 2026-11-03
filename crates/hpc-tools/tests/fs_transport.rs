//! Self-cleaning fake-SSH tests: no network, login commands, or account data.
use hpc_tools::{HpcClient, HpcConfig};
use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt;

fn fixture(script: &str) -> (tempfile::TempDir, HpcClient) {
    let directory = tempfile::tempdir().expect("toy directory");
    let executable = directory.path().join("fake-ssh");
    fs::write(&executable, format!("#!/usr/bin/env python3\n{script}\n")).expect("toy script");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).expect("toy permissions");
    let client = HpcClient::new(HpcConfig {
        ssh_host: "login-toy".into(),
        data_ssh_host: Some("data-toy".into()),
        pi_ssh_host: None,
        ssh_command: executable.to_string_lossy().into_owned(),
        remote_python: "python3".into(),
        state_dir: directory.path().join("state"),
        default_partition: "cpu".into(),
        default_account: None,
    })
    .expect("toy config");
    (directory, client)
}

#[tokio::test]
async fn payload_travels_only_on_stdin_with_explicit_data_endpoint() {
    let (_directory, client) = fixture(
        r#"import json,sys
envelope=json.load(sys.stdin)
assert 'BatchMode=yes' in sys.argv
assert 'StrictHostKeyChecking=yes' in sys.argv
assert 'ControlMaster=no' in sys.argv
assert 'ControlPath=none' in sys.argv
assert 'ControlPersist=no' in sys.argv
assert sys.argv[-2]=='data-toy'
assert 'toy-body-with-$(literal)' not in ' '.join(sys.argv)
exec(envelope['code'], {'REQUEST':envelope['input']})"#,
    );
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.run_json(
            "import json; print(json.dumps(REQUEST))",
            &json!({"body":"toy-body-with-$(literal)"}),
            true,
        ),
    )
    .await
    .expect("stdin EOF must be delivered promptly")
    .expect("transport");
    assert_eq!(result["body"], "toy-body-with-$(literal)");
}

#[tokio::test]
async fn requests_disable_alias_multiplexing_and_detached_master_persistence() {
    let (_directory, client) = fixture(
        r#"import json,sys
json.load(sys.stdin)
options=[sys.argv[i+1] for i,arg in enumerate(sys.argv[:-1]) if arg=='-o']
assert options.count('ControlMaster=no')==1
assert options.count('ControlPath=none')==1
assert options.count('ControlPersist=no')==1
assert not any(option.startswith('ControlMaster=') and option!='ControlMaster=no' for option in options)
assert not any(option.startswith('ControlPath=') and option!='ControlPath=none' for option in options)
assert not any(option.startswith('ControlPersist=') and option!='ControlPersist=no' for option in options)
assert not any(flag in sys.argv[1:-2] for flag in ('-M','-f','-N','-MNf'))
print(json.dumps({'fresh_connection_requested':True,'persistent_master_requested':False}))"#,
    );
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.run_json("", &json!({"scope":"toy"}), false),
    )
    .await
    .expect("bounded fake SSH request")
    .expect("transport");
    assert_eq!(response["fresh_connection_requested"], true);
    assert_eq!(response["persistent_master_requested"], false);
}

#[tokio::test]
async fn failures_do_not_copy_remote_stderr_into_agent_errors() {
    let (_directory, client) =
        fixture("import sys\nsys.stderr.write('TOY_PRIVATE_AUTH_DETAIL')\nsys.exit(7)");
    let error = client
        .run_json("", &json!({}), false)
        .await
        .expect_err("failure");
    assert!(!error.contains("TOY_PRIVATE_AUTH_DETAIL"));
    assert!(error.contains('7'));
}

#[tokio::test]
async fn invalid_json_and_oversized_responses_are_rejected() {
    let (_directory, client) = fixture("print('not a JSON envelope')");
    assert!(
        client
            .run_json("", &json!({}), false)
            .await
            .expect_err("invalid")
            .contains("invalid JSON")
    );
    let (_directory, client) = fixture("import sys\nsys.stdout.write('x'*(8*1024*1024+2))");
    assert!(
        client
            .run_json("", &json!({}), false)
            .await
            .expect_err("oversized")
            .contains("limit")
    );
}

#[tokio::test]
async fn output_limits_abort_without_waiting_for_a_child_that_keeps_running() {
    for (stream, length) in [("stdout", 8 * 1024 * 1024 + 1), ("stderr", 65_537)] {
        let (_directory, client) = fixture(&format!(
            "import json,sys,time\njson.load(sys.stdin)\nsys.{stream}.buffer.write(b'x'*{length})\nsys.{stream}.flush()\ntime.sleep(30)"
        ));
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.run_json("", &json!({}), false),
        )
        .await
        .expect("response limit must abort without waiting for child exit")
        .expect_err("oversized response");
        assert!(error.contains("limit"));
    }
}

#[tokio::test]
async fn data_endpoint_is_required_for_bulk_and_requests_are_bounded() {
    let (_directory, client) = fixture("raise RuntimeError('must not execute')");
    let mut config = client.config().clone();
    config.data_ssh_host = None;
    let client = HpcClient::new(config).expect("config");
    assert!(
        client
            .run_json("", &json!({}), true)
            .await
            .expect_err("missing data node")
            .contains("data SSH endpoint")
    );
    assert!(
        client
            .run_json("", &json!({"large":"x".repeat(8*1024*1024)}), false)
            .await
            .expect_err("oversized request")
            .contains("limit")
    );
}

#[test]
fn private_state_root_is_created_and_public_or_symlink_state_is_rejected() {
    let (directory, client) = fixture("print('{}')");
    let state = directory.path().join("state");
    assert_eq!(
        fs::metadata(&state).expect("state").permissions().mode() & 0o777,
        0o700
    );
    fs::set_permissions(&state, fs::Permissions::from_mode(0o755)).expect("public toy state");
    assert!(HpcClient::new(client.config().clone()).is_err());
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).expect("restore toy state");
    let mut config = client.config().clone();
    config.state_dir = directory.path().join("linked-state");
    std::os::unix::fs::symlink(&state, &config.state_dir).expect("toy link");
    assert!(HpcClient::new(config).is_err());
    let mut config = client.config().clone();
    config.state_dir = directory.path().join("missing-parent").join("state");
    assert!(HpcClient::new(config).is_err());
    assert!(!directory.path().join("missing-parent").exists());
}

#[tokio::test]
async fn file_tool_rejects_placeholder_guards_but_preserves_literal_content() {
    use agentik_core::tools::ToolFunction;
    use hpc_tools::files::HpcFilesTool;
    use std::sync::Arc;

    let (directory, client) =
        fixture("import json,sys\ne=json.load(sys.stdin)\nexec(e['code'], {'REQUEST':e['input']})");
    let tool = HpcFilesTool {
        client: Arc::new(client),
    };
    let path = directory.path().join("new.txt");
    for guard in ["null", ""] {
        let result = tool
            .execute(json!({"action":"write","path":path,
            "content":"null","expected_sha256":guard}))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        assert!(result.text_content().contains("JSON null"));
        assert!(!path.exists());
    }
    let result = tool
        .execute(json!({"action":"write","path":path,
        "content":"null","expected_sha256":null}))
        .await
        .unwrap();
    assert_ne!(result.is_error, Some(true), "{}", result.text_content());
    assert_eq!(fs::read_to_string(path).unwrap(), "null");
    let result = tool
        .execute(json!({"action":"upload_abort","upload_id":"a".repeat(32)}))
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    assert!(result.text_content().contains("original destination path"));
}
