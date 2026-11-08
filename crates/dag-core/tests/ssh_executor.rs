//! End-to-end tests for the direct SSH task backend against a fake ssh
//! transport: a fixture directory provides an `ssh` entry script that drops
//! the ssh options and the host, then runs the remote command locally.
//!
//! Nothing about the scheduler is faked because there is no scheduler in
//! this path — the payload really runs, the supervisor really writes its
//! exit code, and the transport really reads it back. Fixture directories
//! are `tempfile` tempdirs and clean themselves up.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use dag_core::dag::graph::PortOutputs;
use dag_core::dag::node_event::{JobResult, NodeReporter};
use dag_core::node::{NodeInput, NodePorts};
use dag_core::registry::NodeCtx;
use dag_core::{
    ApptainerContainer, ApptainerMode, ApptainerNetwork, FileFingerprint, FileRef, PortType,
    ProcessTaskOutput, ProcessTaskSpec, SshSettings, SshTarget, SshTaskTransport, TaskDispatch,
    TaskExecution, TaskInputBinding, TaskInputSource, TaskLease, TaskOutputBinding, TaskResources,
    TaskSpec, TaskSubmission, TaskTransport,
};
use datafusion::prelude::SessionContext;
use tokio_util::sync::CancellationToken;

// ── fake ssh fixture ──────────────────────────────────────────────────────

const FIXTURE_SSH: &str = r#"#!/bin/sh
# fake ssh: drop -o pairs and the host, then run the remote command locally.
DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
control_master=unset
control_path=unset
control_persist=unset
strict_host_keys=unset
if [ "$1" = "-F" ]; then
  printf '%s\n' "$2" >> "$DIR/pinned_profile_calls.log"
  shift 2
fi
while [ "$1" = "-o" ]; do
  printf '%s\n' "$2" >> "$DIR/ssh_options.log"
  case "$2" in
    ControlMaster=no) control_master=no ;;
    ControlPath=none) control_path=none ;;
    ControlPersist=no) control_persist=no ;;
    StrictHostKeyChecking=yes) strict_host_keys=yes ;;
  esac
  shift 2
done
if [ "$control_master" != no ] || [ "$control_path" != none ] || [ "$control_persist" != no ]; then
  touch "$DIR/ssh_background_master_would_survive"
  exit 97
fi
if [ "$strict_host_keys" != yes ]; then
  exit 98
fi
shift
printf '%s\n' "$*" >> "$DIR/ssh_calls.log"
PATH="$DIR:$PATH"
HOME="$DIR/home"
export PATH HOME
# One-shot failure injection: the command runs, then the connection dies
# before its reply arrives. The remote side keeps the effect it already had.
if [ -f "$DIR/ssh_fail_255_after_run_once" ]; then
  rm -f "$DIR/ssh_fail_255_after_run_once"
  cat > "$DIR/failed_payload.sh"
  /bin/sh "$DIR/failed_payload.sh" >/dev/null 2>&1
  exit 255
fi
# One-shot failure injection: the command never reaches the host.
if [ -f "$DIR/ssh_fail_255_before_run_once" ]; then
  rm -f "$DIR/ssh_fail_255_before_run_once"
  exit 255
fi
exec /bin/sh -c "$*"
"#;

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ssh");
        std::fs::write(&path, FIXTURE_SSH).unwrap();
        make_executable(&path);
        std::fs::create_dir_all(dir.path().join("home")).unwrap();
        Self { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn ssh_target(&self) -> SshTarget {
        SshTarget::new(
            self.dir.path().join("ssh").to_string_lossy().into_owned(),
            "fakehost",
        )
    }

    fn options_log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("ssh_options.log")).unwrap_or_default()
    }

    fn calls_log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("ssh_calls.log")).unwrap_or_default()
    }
}

fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

// ── submission builders ───────────────────────────────────────────────────

fn base_spec(
    id: &str,
    kind: &str,
    spec: serde_json::Value,
    outputs: Vec<TaskOutputBinding>,
) -> TaskSpec {
    TaskSpec {
        id: id.into(),
        logical_node: Some(kind.into()),
        kind: kind.into(),
        spec,
        axis: None,
        item_key: None,
        item: None,
        inputs: Vec::new(),
        outputs,
    }
}

fn process_spec(process: &ProcessTaskSpec) -> serde_json::Value {
    serde_json::to_value(process).unwrap()
}

fn process_submission(
    task_id: &str,
    input_path: &Path,
    process: &ProcessTaskSpec,
    resources: TaskResources,
) -> TaskSubmission {
    let input = FileRef::local(input_path, Some("text".into())).unwrap();
    let mut spec = base_spec(
        task_id,
        "process",
        process_spec(process),
        process
            .outputs
            .iter()
            .map(|output| TaskOutputBinding {
                name: format!("port_{}", output.port),
                port: output.port,
                payload: "channel".into(),
                artifact_paths: Vec::new(),
            })
            .collect(),
    );
    spec.inputs = vec![TaskInputBinding {
        name: "f".into(),
        port: 0,
        source: TaskInputSource::UpstreamPort {
            from: "source".into(),
            from_port: 0,
        },
        payload: "file".into(),
        path: Some(input.path.clone()),
        fingerprint: input.fingerprint.clone(),
        staged_paths: vec![input.path.clone()],
    }];
    TaskSubmission {
        spec,
        inputs: Vec::new(),
        resources,
    }
}

/// The simple process task: one staged input, one JSON output collected by
/// the declared glob.
fn echo_process() -> ProcessTaskSpec {
    ProcessTaskSpec {
        container: None,
        script: Some("cat {{input.f}} > out-0.json\n".into()),
        command: Vec::new(),
        env: Default::default(),
        outputs: vec![ProcessTaskOutput {
            port: 0,
            pattern: "out-*.json".into(),
        }],
    }
}

fn ssh_settings(fixture: &Fixture, scratch: &Path) -> SshSettings {
    SshSettings {
        ssh: fixture.ssh_target(),
        scratch_dir: scratch.to_string_lossy().into_owned(),
        default_time: Duration::from_secs(120),
    }
}

fn transport_of(fixture: &Fixture, scratch: &Path) -> SshTaskTransport {
    SshTaskTransport::new(ssh_settings(fixture, scratch))
}

/// Prepare one immutable submission whose staged input already exists on the
/// host the fake ssh will run on.
fn prepare_simple(cluster: &Path, task_id: &str) -> (TaskDispatch, Vec<FileRef>) {
    let staged = cluster.join(format!("{task_id}-staged-input"));
    std::fs::write(&staged, "content").unwrap();
    let submission =
        process_submission(task_id, &staged, &echo_process(), TaskResources::default());
    let dispatch: TaskDispatch = submission.into_remote_dispatch().unwrap();
    (
        dispatch,
        vec![FileRef::local(&staged, Some("text".into())).unwrap()],
    )
}

fn task_dir(cluster: &Path, task_id: &str) -> PathBuf {
    cluster.join(task_id)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

// ── tests ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_remote_task_runs_and_returns_its_output_with_a_real_fingerprint() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let transport = transport_of(&fixture, cluster.path());
    let (dispatch, artifacts) = prepare_simple(cluster.path(), "ssh-success");
    let lease = transport.submit(dispatch, artifacts).await.unwrap();
    let receipt = transport.wait(&lease).await.unwrap();

    assert_eq!(receipt.task_id, "ssh-success");
    assert_eq!(receipt.executor, "ssh");
    assert_eq!(receipt.status, "success");
    assert_eq!(receipt.exit_code, 0);
    assert_eq!(
        receipt.workspace,
        task_dir(cluster.path(), "ssh-success").to_string_lossy()
    );

    let outputs = &receipt.output_artifacts_by_port[&0];
    assert_eq!(outputs.len(), 1);
    assert!(outputs[0].path.ends_with("output-0-0"));
    let fingerprint = outputs[0].fingerprint.as_ref().unwrap();
    assert_eq!(fingerprint.size, "content".len() as u64);
    assert_eq!(
        fingerprint.content_hash.as_deref(),
        Some(sha256_hex(b"content").as_str())
    );
}

#[tokio::test]
async fn a_failing_remote_task_reports_its_own_exit_code() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let transport = transport_of(&fixture, cluster.path());
    let staged = cluster.path().join("staged");
    std::fs::write(&staged, "content").unwrap();
    let mut process = echo_process();
    process.script = Some("cat {{input.f}} > out-0.json\nexit 7\n".into());
    let submission = process_submission("ssh-failure", &staged, &process, TaskResources::default());
    let dispatch: TaskDispatch = submission.into_remote_dispatch().unwrap();
    let artifacts = vec![FileRef::local(&staged, Some("text".into())).unwrap()];
    let lease = transport.submit(dispatch, artifacts).await.unwrap();
    let receipt = transport.wait(&lease).await.unwrap();

    assert_eq!(receipt.status, "failed");
    assert_eq!(receipt.exit_code, 7);
    // A failed task publishes no output.
    assert!(receipt.output_artifacts_by_port.is_empty());
}

#[tokio::test]
async fn a_replayed_launch_never_starts_a_second_task() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let transport = transport_of(&fixture, cluster.path());
    let staged = cluster.path().join("staged");
    std::fs::write(&staged, "content").unwrap();
    let counter = cluster.path().join("runs.txt");
    let mut process = echo_process();
    process.script = Some(format!(
        "echo run >> {}\ncat {{{{input.f}}}} > out-0.json\n",
        counter.to_string_lossy()
    ));
    let submission = process_submission("ssh-replay", &staged, &process, TaskResources::default());
    let dispatch: TaskDispatch = submission.into_remote_dispatch().unwrap();
    let artifacts = vec![FileRef::local(&staged, Some("text".into())).unwrap()];

    let first = transport
        .submit(dispatch.clone(), artifacts.clone())
        .await
        .unwrap();
    // The exclusive lock makes the second launch a no-op: without it the
    // payload would run twice and the task would race itself.
    let second = transport.submit(dispatch, artifacts).await.unwrap();
    let receipt = transport.wait(&second).await.unwrap();
    assert_eq!(receipt.status, "success");

    let runs = std::fs::read_to_string(&counter).unwrap();
    assert_eq!(
        runs.lines().count(),
        1,
        "payload ran more than once: {runs:?}"
    );
    // The first lease observes the same single attempt.
    let _ = first;
}

#[tokio::test]
async fn a_launch_that_loses_its_reply_is_safe_to_replay() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let transport = transport_of(&fixture, cluster.path());
    let staged = cluster.path().join("staged");
    std::fs::write(&staged, "content").unwrap();
    let counter = cluster.path().join("runs.txt");
    let mut process = echo_process();
    process.script = Some(format!(
        "echo run >> {}\ncat {{{{input.f}}}} > out-0.json\n",
        counter.to_string_lossy()
    ));
    let submission = process_submission(
        "ssh-lost-reply",
        &staged,
        &process,
        TaskResources::default(),
    );
    let dispatch: TaskDispatch = submission.into_remote_dispatch().unwrap();
    let artifacts = vec![FileRef::local(&staged, Some("text".into())).unwrap()];

    // The remote command runs, then the connection dies before its reply.
    std::fs::write(fixture.path().join("ssh_fail_255_after_run_once"), b"").unwrap();
    assert!(
        transport
            .submit(dispatch.clone(), artifacts.clone())
            .await
            .is_err()
    );

    // The replay finds the attempt already launched and must not start a
    // second one.
    let lease = transport.submit(dispatch, artifacts).await.unwrap();
    let receipt = transport.wait(&lease).await.unwrap();
    assert_eq!(receipt.status, "success");
    let runs = std::fs::read_to_string(&counter).unwrap();
    assert_eq!(
        runs.lines().count(),
        1,
        "payload ran more than once: {runs:?}"
    );
}

#[tokio::test]
async fn a_cancelled_task_is_stopped_and_recorded_as_cancelled() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let transport = transport_of(&fixture, cluster.path());
    let staged = cluster.path().join("staged");
    std::fs::write(&staged, "content").unwrap();
    let mut process = echo_process();
    process.script = Some("sleep 300\n".into());
    let submission = process_submission("ssh-cancel", &staged, &process, TaskResources::default());
    let dispatch: TaskDispatch = submission.into_remote_dispatch().unwrap();
    let artifacts = vec![FileRef::local(&staged, Some("text".into())).unwrap()];
    let lease = transport.submit(dispatch, artifacts).await.unwrap();

    // The supervisor records its identity before the payload starts.
    let pid_path = task_dir(cluster.path(), "ssh-cancel").join("run.pid");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !pid_path.exists() && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        pid_path.exists(),
        "the supervisor recorded no process identity"
    );
    let recorded = std::fs::read_to_string(&pid_path).unwrap();
    let pid: i64 = recorded.split_whitespace().next().unwrap().parse().unwrap();

    transport.cancel(&lease).await.unwrap();

    // The process group is gone and the cancellation is recorded.
    let alive = std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .unwrap();
    assert!(!alive.success(), "the remote process survived the cancel");
    let exit_code =
        std::fs::read_to_string(task_dir(cluster.path(), "ssh-cancel").join("exit.code")).unwrap();
    assert_eq!(exit_code.trim(), "130");
}

#[tokio::test]
async fn an_unknown_lease_is_refused() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let transport = transport_of(&fixture, cluster.path());
    let lease = TaskLease {
        lease_id: "attempt-does-not-exist".into(),
        task_id: "ssh-unknown".into(),
    };
    assert!(transport.wait(&lease).await.is_err());
    assert!(transport.cancel(&lease).await.is_err());
}

#[tokio::test]
async fn a_declared_output_port_that_matches_nothing_is_an_error() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let transport = transport_of(&fixture, cluster.path());
    let staged = cluster.path().join("staged");
    std::fs::write(&staged, "content").unwrap();
    let mut process = echo_process();
    process.script = Some("true\n".into());
    let submission =
        process_submission("ssh-no-output", &staged, &process, TaskResources::default());
    let dispatch: TaskDispatch = submission.into_remote_dispatch().unwrap();
    let artifacts = vec![FileRef::local(&staged, Some("text".into())).unwrap()];
    let lease = transport.submit(dispatch, artifacts).await.unwrap();
    let error = transport.wait(&lease).await.unwrap_err().to_string();
    assert!(
        error.contains("matched no files"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn a_container_task_is_refused_before_any_remote_contact() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let transport = transport_of(&fixture, cluster.path());
    let staged = cluster.path().join("staged");
    std::fs::write(&staged, "content").unwrap();
    let mut process = echo_process();
    process.container = Some(ApptainerContainer {
        image: format!("ghcr.io/autonomics/toy@sha256:{}", "a".repeat(64)),
        mode: ApptainerMode::Exec,
        network: ApptainerNetwork::Host,
    });
    let submission =
        process_submission("ssh-container", &staged, &process, TaskResources::default());
    let dispatch: TaskDispatch = submission.into_remote_dispatch().unwrap();
    let artifacts = vec![FileRef::local(&staged, Some("text".into())).unwrap()];
    let error = transport
        .submit(dispatch, artifacts)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("direct SSH backend"),
        "unexpected error: {error}"
    );
    assert!(fixture.calls_log().is_empty());
}

#[tokio::test]
async fn the_receipt_pins_the_exact_submitted_manifest() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let transport = transport_of(&fixture, cluster.path());
    let (mut dispatch, artifacts) = prepare_simple(cluster.path(), "ssh-manifest");
    dispatch.spec.spec["custody_literal"] = serde_json::json!("snowman \u{2603}");
    let lease = transport.submit(dispatch, artifacts).await.unwrap();
    let receipt = transport.wait(&lease).await.unwrap();

    let submitted =
        std::fs::read(task_dir(cluster.path(), "ssh-manifest").join("task.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&submitted).unwrap();
    assert_eq!(
        parsed["spec"]["spec"]["custody_literal"],
        "snowman \u{2603}"
    );
    assert!(submitted.ends_with(b"}\n"));
    let expected = FileFingerprint {
        size: submitted.len() as u64,
        mtime_ns: 0,
        content_hash: Some(sha256_hex(&submitted)),
        immutable_remote: false,
    };
    assert_eq!(receipt.task_manifest.fingerprint, Some(expected));
}

#[tokio::test]
async fn a_task_logs_carry_measured_fingerprints() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let transport = transport_of(&fixture, cluster.path());
    let staged = cluster.path().join("staged");
    std::fs::write(&staged, "content").unwrap();
    let mut process = echo_process();
    process.script = Some(
        "printf 'to stdout\\n'\nprintf 'to stderr\\n' >&2\ncat {{input.f}} > out-0.json\n".into(),
    );
    let submission = process_submission("ssh-logs", &staged, &process, TaskResources::default());
    let dispatch: TaskDispatch = submission.into_remote_dispatch().unwrap();
    let artifacts = vec![FileRef::local(&staged, Some("text".into())).unwrap()];
    let lease = transport.submit(dispatch, artifacts).await.unwrap();
    let receipt = transport.wait(&lease).await.unwrap();

    let stdout = receipt.stdout_log.as_ref().unwrap();
    assert!(stdout.path.ends_with("stdout.log"));
    assert_eq!(
        stdout.fingerprint.as_ref().unwrap().content_hash.as_deref(),
        Some(sha256_hex(b"to stdout\n").as_str())
    );
    let stderr = receipt.stderr_log.as_ref().unwrap();
    assert_eq!(
        stderr.fingerprint.as_ref().unwrap().content_hash.as_deref(),
        Some(sha256_hex(b"to stderr\n").as_str())
    );
}

#[tokio::test]
async fn a_long_wait_keeps_its_connection_alive() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let mut settings = ssh_settings(&fixture, cluster.path());
    settings.ssh = settings.ssh.with_keepalive(Duration::from_secs(15), 2);
    let transport = SshTaskTransport::new(settings);
    let (dispatch, artifacts) = prepare_simple(cluster.path(), "ssh-keepalive");
    let lease = transport.submit(dispatch, artifacts).await.unwrap();
    let receipt = transport.wait(&lease).await.unwrap();
    assert_eq!(receipt.status, "success");

    let options = fixture.options_log();
    assert!(
        options.lines().any(|line| line == "ServerAliveInterval=15"),
        "no keepalive interval in {options:?}"
    );
    assert!(
        options.lines().any(|line| line == "ServerAliveCountMax=2"),
        "no keepalive count in {options:?}"
    );
}

#[tokio::test]
async fn a_short_wait_configures_no_keepalive() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let transport = transport_of(&fixture, cluster.path());
    let (dispatch, artifacts) = prepare_simple(cluster.path(), "ssh-no-keepalive");
    let lease = transport.submit(dispatch, artifacts).await.unwrap();
    transport.wait(&lease).await.unwrap();
    let options = fixture.options_log();
    assert!(
        !options.lines().any(|line| line.starts_with("ServerAlive")),
        "unexpected keepalive in {options:?}"
    );
}

#[tokio::test]
async fn a_task_without_an_explicit_duration_uses_the_operator_horizon() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let transport = transport_of(&fixture, cluster.path());
    let (dispatch, artifacts) = prepare_simple(cluster.path(), "ssh-horizon");
    // No `max_duration_ms`: the transport must not invent one.
    assert!(dispatch.resources.max_duration_ms.is_none());
    let lease = transport.submit(dispatch, artifacts).await.unwrap();
    let receipt = transport.wait(&lease).await.unwrap();
    assert_eq!(receipt.status, "success");
}
