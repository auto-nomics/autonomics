//! Real-server smoke tests for the direct SSH task backend: these run real
//! payloads on a remote host over ssh, so they are `#[ignore]`d and must
//! only be run with explicit operator approval, e.g.
//!
//! ```text
//! cargo test -p dag-core --test ssh_smoke_real -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! The host defaults to the `ht` alias (operator-owned `~/.ssh/config`
//! entry with pinned port, user and identity) and can be overridden with
//! `AUTONOMICS_SSH_SMOKE_HOST`. Remote scratch lives under
//! `~/autonomics-ssh-smoke-<unix-stamp>`; each test prints its scratch
//! directory for the operator to clean up after triage.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dag_core::{
    FileRef, ProcessTaskOutput, ProcessTaskSpec, SshArtifactStore, SshSettings, SshTarget,
    SshTaskTransport, TaskArtifactStore, TaskDispatch, TaskInputBinding, TaskInputSource,
    TaskOutputBinding, TaskResources, TaskSpec, TaskSubmission, TaskTransport,
};

// ── smoke helpers ────────────────────────────────────────────────────────

fn smoke_env(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn unix_stamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Run one remote command with BatchMode and return its stdout, failing loud
/// on a nonzero exit — the same discipline the transport itself uses.
fn ssh_text(host: &str, remote: &str) -> String {
    let output = std::process::Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=15",
            host,
            remote,
        ])
        .output()
        .expect("spawn ssh");
    assert!(
        output.status.success(),
        "ssh `{remote}` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("ssh stdout utf-8")
}

fn process_submission(
    task_id: &str,
    input: &FileRef,
    process: &ProcessTaskSpec,
) -> TaskSubmission {
    let staged = input.path.clone();
    TaskSubmission {
        spec: TaskSpec {
            id: task_id.into(),
            logical_node: Some("process".into()),
            kind: "process".into(),
            spec: serde_json::to_value(process).unwrap(),
            axis: None,
            item_key: None,
            item: None,
            inputs: vec![TaskInputBinding {
                name: "f".into(),
                port: 0,
                source: TaskInputSource::UpstreamPort {
                    from: "source".into(),
                    from_port: 0,
                },
                payload: "file".into(),
                path: Some(input.path.clone()),
                fingerprint: input.fingerprint.clone(),
                staged_paths: vec![staged],
            }],
            outputs: process
                .outputs
                .iter()
                .map(|output| TaskOutputBinding {
                    name: format!("port_{}", output.port),
                    port: output.port,
                    payload: "file".into(),
                    artifact_paths: Vec::new(),
                })
                .collect(),
        },
        inputs: Vec::new(),
        resources: TaskResources::default(),
    }
}

fn settings(host: &str, scratch: String, _local_artifacts: &Path) -> SshSettings {
    SshSettings {
        ssh: SshTarget::new("ssh", host).with_keepalive(Duration::from_secs(15), 2),
        scratch_dir: scratch,
        default_time: Duration::from_secs(600),
    }
}

// ── tests ────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "runs real payloads on a remote host over ssh; run only with explicit operator approval"]
async fn real_host_runs_process_task_end_to_end() {
    let host = smoke_env("AUTONOMICS_SSH_SMOKE_HOST", "ht");
    let scratch = format!("~/autonomics-ssh-smoke-{}", unix_stamp());
    println!("scratch: {scratch}");
    let task_id = "ssh-smoke-ok";
    let local_artifacts = tempfile::tempdir().unwrap();
    let coordinator = tempfile::tempdir().unwrap();

    let input_path = coordinator.path().join("input.txt");
    std::fs::write(&input_path, "remote ssh smoke input").unwrap();
    let input = FileRef::local(&input_path, Some("text".into())).unwrap();

    let store = SshArtifactStore::new(
        SshTarget::new("ssh", &host),
        scratch.clone(),
        local_artifacts.path(),
    );
    let uploaded = store
        .upload(input, task_id, "input-0-0")
        .await
        .unwrap();
    let submission = process_submission(task_id, &uploaded, &ProcessTaskSpec {
        container: None,
        script: Some(
            "hostname > out-0.txt\ncat $AUTONOMICS_WORKSPACE/input-0-0 >> out-0.txt\necho stdout-observed\n"
                .into(),
        ),
        command: Vec::new(),
        env: Default::default(),
        outputs: vec![ProcessTaskOutput {
            port: 0,
            pattern: "out-*.txt".into(),
        }],
    });
    let dispatch: TaskDispatch = submission.into_remote_dispatch().unwrap();

    let transport = SshTaskTransport::new(settings(&host, scratch.clone(), local_artifacts.path()));
    let lease = transport.submit(dispatch, vec![uploaded]).await.unwrap();
    let receipt = transport.wait(&lease).await.unwrap();

    assert_eq!(receipt.status, "success", "status: {receipt:?}");
    assert_eq!(receipt.exit_code, 0);
    let outputs = &receipt.output_artifacts_by_port[&0];
    assert_eq!(outputs.len(), 1);
    let downloaded = store
        .download(outputs[0].clone(), task_id, "out-0.txt")
        .await
        .unwrap();
    let body = std::fs::read_to_string(&downloaded.path).unwrap();
    assert_eq!(body, "bioinfo10\nremote ssh smoke input");
    let stdout = receipt.stdout_log.as_ref().expect("stdout log");
    assert!(stdout.fingerprint.as_ref().unwrap().size > 0);
    println!("task {task_id} completed on {}", body.lines().next().unwrap());
}

#[tokio::test]
#[ignore = "runs real payloads on a remote host over ssh; run only with explicit operator approval"]
async fn real_host_reports_failure_without_outputs() {
    let host = smoke_env("AUTONOMICS_SSH_SMOKE_HOST", "ht");
    let scratch = format!("~/autonomics-ssh-smoke-{}", unix_stamp());
    println!("scratch: {scratch}");
    let task_id = "ssh-smoke-fail";
    let local_artifacts = tempfile::tempdir().unwrap();

    let input_path = local_artifacts.path().join("input.txt");
    std::fs::write(&input_path, "x").unwrap();
    let input = FileRef::local(&input_path, Some("text".into())).unwrap();
    let submission = process_submission(task_id, &input, &ProcessTaskSpec {
        container: None,
        script: Some("echo deliberate-failure >&2\nexit 9\n".into()),
        command: Vec::new(),
        env: Default::default(),
        outputs: vec![ProcessTaskOutput {
            port: 0,
            pattern: "out-*.txt".into(),
        }],
    });
    let dispatch: TaskDispatch = submission.into_remote_dispatch().unwrap();

    let transport = SshTaskTransport::new(settings(&host, scratch.clone(), local_artifacts.path()));
    let lease = transport
        .submit(dispatch, vec![input])
        .await
        .unwrap();
    let receipt = transport.wait(&lease).await.unwrap();

    assert_eq!(receipt.status, "failed");
    assert_eq!(receipt.exit_code, 9);
    assert!(receipt.output_artifacts_by_port.is_empty());
    let stderr = receipt.stderr_log.as_ref().expect("stderr log");
    let fingerprint = stderr.fingerprint.as_ref().unwrap();
    assert!(fingerprint.size > 0);
    assert!(fingerprint.content_hash.is_some());
    println!("failure surfaced with exit code {}", receipt.exit_code);
}

#[tokio::test]
#[ignore = "runs real payloads on a remote host over ssh; run only with explicit operator approval"]
async fn real_host_cancel_stops_a_running_task() {
    let host = smoke_env("AUTONOMICS_SSH_SMOKE_HOST", "ht");
    let stamp = unix_stamp();
    let scratch = format!("~/autonomics-ssh-smoke-{stamp}");
    println!("scratch: {scratch}");
    let task_id = "ssh-smoke-cancel";
    let local_artifacts = tempfile::tempdir().unwrap();

    let input_path = local_artifacts.path().join("input.txt");
    std::fs::write(&input_path, "x").unwrap();
    let input = FileRef::local(&input_path, Some("text".into())).unwrap();
    let submission = process_submission(task_id, &input, &ProcessTaskSpec {
        container: None,
        script: Some("sleep 600\n".into()),
        command: Vec::new(),
        env: Default::default(),
        outputs: vec![ProcessTaskOutput {
            port: 0,
            pattern: "out-*.txt".into(),
        }],
    });
    let dispatch: TaskDispatch = submission.into_remote_dispatch().unwrap();

    let transport = SshTaskTransport::new(settings(&host, scratch.clone(), local_artifacts.path()));
    let lease = transport
        .submit(dispatch, vec![input])
        .await
        .unwrap();

    // Wait until the supervisor published its identity, then cancel. The
    // task id is already a sanitized directory name, so the attempt
    // directory is predictable without listing the scratch root.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let pid = ssh_text(
            &host,
            &format!("cat {scratch}/{task_id}/run.pid 2>/dev/null || true"),
        );
        // run.pid records "PID PGID"; the pid is the first field.
        if pid
            .split_whitespace()
            .next()
            .and_then(|first| first.parse::<i64>().ok())
            .is_some()
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the supervisor never published its process identity"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    transport.cancel(&lease).await.unwrap();

    let recorded = ssh_text(
        &host,
        &format!("cat {scratch}/{task_id}/exit.code"),
    );
    assert_eq!(recorded.trim(), "130");
    // No supervisor from this attempt may survive the cancel. The bracketed
    // first character keeps the probe from matching its own command line.
    let survivors = ssh_text(
        &host,
        &format!("pgrep -af '[a]utonomics-ssh-smoke-{stamp}' || true"),
    );
    assert!(survivors.trim().is_empty(), "survivors: {survivors}");
    println!("cancel confirmed for {task_id}");
}
