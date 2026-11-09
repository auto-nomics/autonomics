//! End-to-end tests for the Slurm task backend against a fake ssh cluster:
//! a fixture directory provides an `ssh` entry script plus `sbatch` /
//! `squeue` / `sacct` / `scancel` shims driven by state files, so the real
//! transport, artifact store, remote executor, and routing executor all run
//! locally — only the cluster itself is faked. Fixture directories are
//! `tempfile` tempdirs and clean themselves up.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use dag_core::dag::graph::PortOutputs;
use dag_core::dag::node_event::{JobResult, NodeReporter};
use dag_core::node::{NodeInput, NodePorts};
use dag_core::registry::NodeCtx;
use dag_core::{
    FileFingerprint, FileRef, PortType, ProcessRoutingTaskExecutor, ProcessTaskOutput,
    ProcessTaskSpec, RemoteTaskExecutor, SlurmSettings, SlurmTaskTransport, SshArtifactStore,
    SshTarget, TaskArtifactStore, TaskAttemptReceipt, TaskDispatch, TaskExecution, TaskExecutor,
    TaskInputBinding, TaskInputSource, TaskLease, TaskOutputBinding, TaskResources, TaskSpec,
    TaskSubmission, TaskTransport, dag::DagError, local_task_executor, slurm_backed_executor,
};
use datafusion::prelude::SessionContext;
use tokio_util::sync::CancellationToken;

// ── fake cluster fixture ─────────────────────────────────────────────────

const FIXTURE_SSH: &str = r#"#!/bin/sh
# fake ssh: drop -o pairs and the host, then run the remote command locally
# with the fixture's shims first on PATH.
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
# Model an inherited ControlMaster=auto/ControlPersist=7d alias: both command
# paths must override it rather than leaving an independently managed master.
if [ "$control_master" != no ] || [ "$control_path" != none ] || [ "$control_persist" != no ]; then
  touch "$DIR/ssh_background_master_would_survive"
  exit 97
fi
if [ "$strict_host_keys" != yes ]; then
  exit 98
fi
shift
printf '%s\n' "$*" >> "$DIR/ssh_calls.log"
# The shims must be on PATH before the failure flags run anything: the
# "after" flag re-executes the submitted command, and if sbatch were not
# resolvable there, "sbatch ran but stdout was lost" would silently
# degrade into "nothing ever ran".
PATH="$DIR:$PATH"
HOME="$DIR/home"
export PATH HOME
# One-shot failure injection: "before" sends nothing at all (the command
# never reached the cluster); "after" runs the command but still fails, like
# a connection lost after sbatch committed. The flags fire on the FIRST ssh
# invocation after the file appears, then self-destruct.
if [ -f "$DIR/ssh_fail_255_before_once" ]; then
  rm -f "$DIR/ssh_fail_255_before_once"
  echo 'synthetic SSH connection rejected before bootstrap' >&2
  exit 255
fi
if [ -f "$DIR/ssh_fail_255_after_run_once" ]; then
  rm -f "$DIR/ssh_fail_255_after_run_once"
  if [ "$*" = "bash -s" ]; then
    IFS= read -r handshake
    /bin/sh -c "$handshake"
  fi
  /bin/sh -c "$*" >/dev/null 2>&1
  if [ -f "$DIR/discard_submission_records" ]; then
    while IFS= read -r record; do rm -f -- "$record"; done < "$DIR/discard_submission_records"
  fi
  exit 255
fi
case "$*" in
  *'cat > '*)
    if [ -f "$DIR/upload_auth_failure_once" ]; then
      rm -f "$DIR/upload_auth_failure_once"
      exit 255
    fi
    if [ -f "$DIR/upload_lost_reply_once" ]; then
      rm -f "$DIR/upload_lost_reply_once"
      /bin/sh -c "$*" >/dev/null 2>&1
      exit 255
    fi
    if [ -f "$DIR/upload_partial_once" ]; then
      rm -f "$DIR/upload_partial_once"
      head -c 2 | /bin/sh -c "$*"
      exit 255
    fi
    ;;
  'cat -- '*)
    if [ -f "$DIR/download_partial_transport_once" ]; then
      rm -f "$DIR/download_partial_transport_once"
      /bin/sh -c "$*" | head -c 1
      exit 255
    fi
    if [ -f "$DIR/download_non_transport_failure_once" ]; then
      rm -f "$DIR/download_non_transport_failure_once"
      exit 42
    fi
    ;;
esac
exec /bin/sh -c "$*"
"#;

const FIXTURE_SBATCH: &str = r#"#!/bin/sh
set -e
DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
printf '%s\n' "$*" >> "$DIR/calls.log"
# Record the job name being submitted so listing fixtures can refer to it as
# @LAST@ — the nonce is unpredictable to tests.
for arg in "$@"; do
  case "$arg" in
    *.sbatch)
      sed -n 's/^#SBATCH --job-name=//p' "$arg" > "$DIR/last_job_name" 2>/dev/null
      # Model completion using only deliberately staged toy output bytes.
      # Never execute the submitted payload: the transport has chosen the
      # attempt directory, so discover it from the actual manifest/script.
      # Legacy direct-transport fixtures retain their pre-staged directory
      # and pure-shell timing (including the 100 ms submission-timeout case).
      case "${PWD##*/}" in attempt-*)
      python3 - "$DIR" "$arg" <<'FIXTURE_PREPARE_OUTPUTS'
import json,pathlib,re,shutil,sys
fixture=pathlib.Path(sys.argv[1])
taskdir=pathlib.Path(sys.argv[2]).resolve().parent
manifest=json.loads((taskdir/'task.json').read_text())
logical=manifest['spec']['id']
namespace=manifest['spec']['spec'].get('_autonomics_attempt_key')
if namespace is not None:
 assert re.fullmatch(r'attempt-[0-9a-f]{64}',namespace)
 assert taskdir.name==namespace
 source=taskdir.parent/logical
 for output in source.glob('out-*.json'):
  shutil.copyfile(output,taskdir/output.name)
 job=(fixture/'job_id').read_text().strip() if (fixture/'job_id').exists() else '4242'
 for suffix in ('out','err'):
  log=fixture/('job-log.'+suffix)
  if log.exists():shutil.copyfile(log,taskdir/('slurm-'+job+'.'+suffix))
with (fixture/'submitted_task_dirs.jsonl').open('a') as stream:
 stream.write(json.dumps({'task_id':logical,'task_dir':str(taskdir)})+'\n')
FIXTURE_PREPARE_OUTPUTS
      ;; esac
      ;;
  esac
done
cat "$DIR/job_id" 2>/dev/null || echo 4242
"#;

const FIXTURE_SQUEUE: &str = r#"#!/bin/sh
DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
printf '%s\n' "$*" >> "$DIR/calls.log"
case "$*" in
  *"%i|%.256u|%.100k|%.8192Z|%.256j"*)
    if [ -f "$DIR/squeue_ownership_out" ]; then cat "$DIR/squeue_ownership_out"; fi
    ;;
  *"%i %j"*)
    # Full listing for adoption queries; @LAST@ expands to the job name of
    # the most recent sbatch call.
    if [ -f "$DIR/squeue_listing" ]; then
      last=$(cat "$DIR/last_job_name" 2>/dev/null)
      sed "s/@LAST@/$last/" "$DIR/squeue_listing"
    fi
    ;;
  *"-j "*)
    # Per-job state probe used by wait/cancel; file absent means the job is
    # not in the queue.
    if [ -f "$DIR/squeue_job_state" ]; then cat "$DIR/squeue_job_state"; elif [ -f "$DIR/queued" ]; then echo RUNNING; fi
    ;;
  *)
    if [ -f "$DIR/queued" ]; then echo RUNNING; fi
    ;;
esac
"#;

const FIXTURE_SACCT: &str = r#"#!/bin/sh
DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
printf '%s\n' "$*" >> "$DIR/calls.log"
case "$*" in
  *"--format=JobIDRaw,State%40,ExitCode,End,User%256,Comment%100,WorkDir%8192,JobName%256"*)
    if [ -f "$DIR/sacct_full_stop_out" ]; then cat "$DIR/sacct_full_stop_out"; fi
    ;;
  *"--format=JobIDRaw,User%256,Comment%100,WorkDir%8192,JobName%256"*)
    if [ -f "$DIR/sacct_ownership_out" ]; then cat "$DIR/sacct_ownership_out"; fi
    ;;
  *--name=*)
    # Adoption query by exact job name.
    if [ -f "$DIR/sacct_name_out" ]; then cat "$DIR/sacct_name_out"; fi
    ;;
  *--me*)
    echo 'sacct: unrecognized option --me' >&2
    exit 1
    ;;
  *"-u "*)
    # Accounting-liveness canary: absent file = accounting alive (one old
    # record); present-but-empty = accounting has no records.
    if [ -f "$DIR/sacct_canary_out" ]; then cat "$DIR/sacct_canary_out"; else echo "999"; fi
    ;;
  *)
    # Per-job State,ExitCode probe used by wait/cancel.
    if [ -f "$DIR/sacct_out" ]; then cat "$DIR/sacct_out"; fi
    ;;
esac
"#;

const FIXTURE_SCANCEL: &str = r#"#!/bin/sh
DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
printf '%s\n' "$*" >> "$DIR/calls.log"
touch "$DIR/cancelled"
if [ -f "$DIR/scancel_fail" ]; then
  echo "scancel: operation failed" >&2
  exit 1
fi
"#;

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        // The pinned SSH profile requires an owner-only parent directory;
        // tempfile uses the process umask for directory permissions.
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        // Child-only HOME: never mutate the Rust process environment, and
        // never let a fake `cd && pwd` resolve to a real account home.
        let home = dir.path().join("home");
        std::fs::create_dir(&home).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        for (name, contents) in [
            ("ssh", FIXTURE_SSH),
            ("sbatch", FIXTURE_SBATCH),
            ("squeue", FIXTURE_SQUEUE),
            ("sacct", FIXTURE_SACCT),
            ("scancel", FIXTURE_SCANCEL),
        ] {
            let path = dir.path().join(name);
            std::fs::write(&path, contents).unwrap();
            make_executable(&path);
        }
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

    fn calls_log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("calls.log")).unwrap_or_default()
    }

    fn ssh_calls_log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("ssh_calls.log")).unwrap_or_default()
    }

    fn submitted_task_dirs(&self, cluster: &Path, task_id: &str) -> Vec<PathBuf> {
        std::fs::read_to_string(self.path().join("submitted_task_dirs.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .filter(|record| record["task_id"] == task_id)
            .map(|record| {
                let directory = PathBuf::from(record["task_dir"].as_str().unwrap());
                assert_eq!(directory.parent(), Some(cluster));
                let manifest: TaskDispatch =
                    serde_json::from_slice(&std::fs::read(directory.join("task.json")).unwrap())
                        .unwrap();
                assert_eq!(manifest.spec.id, task_id);
                if let Some(namespace) = manifest.spec.spec.get("_autonomics_attempt_key") {
                    let namespace = namespace.as_str().unwrap();
                    assert_eq!(namespace.len(), 72);
                    assert!(namespace.starts_with("attempt-"));
                    assert!(
                        namespace[8..]
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                    );
                    assert_eq!(directory.file_name().unwrap(), namespace);
                }
                directory
            })
            .collect()
    }

    fn submitted_task_dir(&self, cluster: &Path, task_id: &str) -> PathBuf {
        let directories = self.submitted_task_dirs(cluster, task_id);
        assert_eq!(
            directories.len(),
            1,
            "expected exactly one submitted attempt"
        );
        directories.into_iter().next().unwrap()
    }
}

fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

// ── submission builders ──────────────────────────────────────────────────

fn noop_node() -> Box<dyn dag_core::DagNode> {
    #[derive(Clone)]
    struct NoopNode {
        ports: NodePorts,
    }

    impl Default for NoopNode {
        fn default() -> Self {
            Self {
                ports: NodePorts::new().add_output_port(None),
            }
        }
    }

    #[async_trait::async_trait]
    impl dag_core::DagNode for NoopNode {
        fn ports(&self) -> &NodePorts {
            &self.ports
        }

        async fn execute(
            &mut self,
            _ctx: &NodeCtx,
            _inputs: &[NodeInput],
            _reporter: &NodeReporter,
        ) -> Result<PortOutputs, DagError> {
            Ok(PortOutputs::new())
        }

        fn clone_box(&self) -> Box<dyn dag_core::DagNode> {
            Box::new(self.clone())
        }

        fn kind(&self) -> &'static str {
            "noop"
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    Box::new(NoopNode::default())
}

fn test_ctx() -> Arc<NodeCtx> {
    Arc::new(NodeCtx::new(SessionContext::new().runtime_env(), None))
}

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
        staged_paths: Vec::new(),
    }];
    TaskSubmission {
        spec,
        inputs: vec![NodeInput::file(0, input)],
        resources,
    }
}

fn noop_submission(task_id: &str) -> TaskSubmission {
    TaskSubmission {
        spec: base_spec(
            task_id,
            "noop",
            serde_json::json!({"mode": "local"}),
            vec![TaskOutputBinding {
                name: "port_0".into(),
                port: 0,
                payload: PortType::Channel.to_string(),
                artifact_paths: Vec::new(),
            }],
        ),
        inputs: Vec::new(),
        resources: TaskResources::default(),
    }
}

fn execution(submission: TaskSubmission) -> TaskExecution {
    TaskExecution {
        submission,
        node: noop_node(),
        engine_ctx: test_ctx(),
        reporter: NodeReporter::noop(),
        cancellation: CancellationToken::new(),
    }
}

/// The simple process task every full-stack test submits: one staged input,
/// one JSON output collected by the declared glob.
fn echo_process() -> ProcessTaskSpec {
    ProcessTaskSpec {
        container: None,
        script: Some("cat {{input.f}} > out-0.json\n".into()),
        command: Vec::new(),
        env: Default::default(),
        outputs: vec![ProcessTaskOutput {
            port: 0,
            pattern: "out-*.json".into(),
            optional: false,
        }],
    }
}

fn slurm_settings(fixture: &Fixture, cluster: &Path, local_artifacts: &Path) -> SlurmSettings {
    SlurmSettings {
        apptainer: None,
        ssh: fixture.ssh_target(),
        scratch_dir: cluster.to_string_lossy().into_owned(),
        local_artifact_root: local_artifacts.to_path_buf(),
        partition: "debug64c512g".into(),
        account: None,
        gpus: None,
        qos: None,
        mem_per_cpu: "8G".into(),
        default_time: "01:00:00".into(),
        poll_interval: Duration::from_millis(5),
    }
}

#[test]
fn frozen_compute_and_shared_endpoints_are_part_of_the_submission_binding() {
    let fixture = Fixture::new();
    let base = slurm_settings(&fixture, fixture.path(), fixture.path());
    let binding = |settings| {
        SlurmTaskTransport::new(settings)
            .configured_submission_binding()
            .unwrap()
    };
    let legacy = binding(base.clone());
    let mut empty = base.clone();
    empty.ssh = empty
        .ssh
        .with_shared_endpoints(std::collections::BTreeMap::new());
    assert_eq!(binding(empty), legacy);
    let endpoint = vfs::SshEndpoint {
        hostname: "original.invalid".into(),
        user: "synthetic".into(),
        port: 22,
        host_key_alias: "original.invalid".into(),
    };
    let mut pinned = base.clone();
    pinned.ssh = pinned.ssh.with_endpoint(endpoint.clone());
    let original = binding(pinned);
    assert_ne!(original, legacy);
    for field in ["hostname", "user", "port", "host_key_alias"] {
        let mut changed = endpoint.clone();
        match field {
            "hostname" => changed.hostname = "replacement.invalid".into(),
            "user" => changed.user = "other".into(),
            "port" => changed.port = 2222,
            _ => changed.host_key_alias = "replacement.invalid".into(),
        }
        let mut settings = base.clone();
        settings.ssh = settings.ssh.with_endpoint(changed);
        assert_ne!(binding(settings), original, "{field}");
    }
    let shared = |ep| {
        let mut settings = base.clone();
        settings.ssh = settings
            .ssh
            .with_shared_endpoints(std::collections::BTreeMap::from([("data".into(), ep)]));
        binding(settings)
    };
    let old = shared(endpoint.clone());
    assert_ne!(old, legacy);
    let mut changed = endpoint;
    changed.user = "other".into();
    assert_ne!(shared(changed), old);
}

#[tokio::test]
async fn invalid_frozen_endpoints_are_refused_before_a_binding_or_ssh_process() {
    let fixture = Fixture::new();
    for shared in [false, true] {
        let endpoint = vfs::SshEndpoint {
            hostname: "-oProxyCommand=anything".into(),
            user: "synthetic".into(),
            port: 22,
            host_key_alias: "synthetic.invalid".into(),
        };
        let ssh = if shared {
            fixture
                .ssh_target()
                .with_shared_endpoints(std::collections::BTreeMap::from([(
                    "data".into(),
                    endpoint,
                )]))
        } else {
            fixture.ssh_target().with_endpoint(endpoint)
        };
        assert!(ssh.run("true").await.is_err());
        let mut settings = slurm_settings(&fixture, fixture.path(), fixture.path());
        settings.ssh = ssh;
        assert!(
            SlurmTaskTransport::new(settings)
                .configured_submission_binding()
                .is_err()
        );
    }
    assert!(!fixture.path().join("ssh_options.log").exists());
    assert!(!fixture.path().join("ssh_calls.log").exists());
}

#[tokio::test]
async fn apptainer_requires_operator_manifest_and_never_falls_back_to_host() {
    let fixture = Fixture::new();
    let root = tempfile::tempdir().unwrap();
    let mut process = echo_process();
    process.container = Some(dag_core::ApptainerContainer {
        image: format!("ghcr.io/autonomics/toy@sha256:{}", "a".repeat(64)),
        mode: dag_core::ApptainerMode::Exec,
        network: dag_core::ApptainerNetwork::Host,
    });
    std::fs::write(fixture.path().join("input"), "synthetic input").unwrap();
    let submission = process_submission(
        "container-required",
        &fixture.path().join("input"),
        &process,
        TaskResources::default(),
    );
    let dispatch = TaskDispatch {
        spec: submission.spec.clone(),
        resources: submission.resources.clone(),
    };
    let settings = slurm_settings(&fixture, root.path(), root.path());
    let transport = SlurmTaskTransport::new(settings);
    assert!(
        transport
            .submit(dispatch.clone(), vec![])
            .await
            .unwrap_err()
            .to_string()
            .contains("operator SIF manifest")
    );
    assert!(fixture.ssh_calls_log().is_empty());
    let host = dag_core::ProcessTaskTransport::new(root.path().join("host"));
    assert!(
        host.submit(dispatch, vec![])
            .await
            .unwrap_err()
            .to_string()
            .contains("host execution refused")
    );
    assert!(!root.path().join("host").exists());
    let routing = ProcessRoutingTaskExecutor::new(local_task_executor(), None);
    assert!(matches!(
        routing.run(execution(submission.clone())).await,
        JobResult::Failed { .. }
    ));
    assert!(matches!(
        local_task_executor().run(execution(submission)).await,
        JobResult::Failed { .. }
    ));
}

#[tokio::test]
async fn apptainer_changed_operator_sif_binding_cannot_reuse_an_existing_submission() {
    let fixture = Fixture::new();
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("input");
    std::fs::write(&input, "synthetic").unwrap();
    let image = format!("ghcr.io/autonomics/toy@sha256:{}", "a".repeat(64));
    let mut process = echo_process();
    process.container = Some(dag_core::ApptainerContainer {
        image: image.clone(),
        mode: dag_core::ApptainerMode::Exec,
        network: dag_core::ApptainerNetwork::Host,
    });
    let submission = process_submission("sif-binding", &input, &process, TaskResources::default());
    let dispatch = TaskDispatch {
        spec: submission.spec,
        resources: submission.resources,
    };
    let mut settings = slurm_settings(
        &fixture,
        &root.path().join("cluster"),
        &root.path().join("local"),
    );
    settings.apptainer = Some(dag_core::ApptainerSettings {
        program: "/usr/bin/apptainer".into(),
        images: std::collections::BTreeMap::from([(
            image,
            dag_core::SifImage {
                path: "/site/images/toy.sif".into(),
                sha256: "b".repeat(64),
            },
        )]),
    });
    SlurmTaskTransport::new(settings.clone())
        .submit(dispatch.clone(), vec![])
        .await
        .unwrap();
    let path = root.path().join("cluster/sif-binding/task.json");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(
        manifest["apptainer_execution"]["sif"]["sha256"],
        "b".repeat(64)
    );
    settings
        .apptainer
        .as_mut()
        .unwrap()
        .images
        .values_mut()
        .next()
        .unwrap()
        .sha256 = "c".repeat(64);
    assert!(
        SlurmTaskTransport::new(settings)
            .submit(dispatch, vec![])
            .await
            .is_err()
    );
    assert_eq!(
        fixture
            .calls_log()
            .lines()
            .filter(|line| line.contains("--parsable"))
            .count(),
        1
    );
}

fn slurm_stack(
    fixture: &Fixture,
    cluster: &Path,
    local_artifacts: &Path,
    coordinator: &Path,
) -> Arc<dyn TaskExecutor> {
    slurm_backed_executor(
        slurm_settings(fixture, cluster, local_artifacts),
        coordinator,
        local_task_executor(),
    )
}

fn task_dir(cluster: &Path, task_id: &str) -> PathBuf {
    cluster.join(task_id)
}

fn stage_output_file(cluster: &Path, task_id: &str) {
    let dir = task_dir(cluster, task_id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("out-0.json"), br#"[{"value":42}]"#).unwrap();
}

/// Bind the rendered directive and the cluster's observed name to the durable
/// attempt identity, including the nonce used by exact-name adoption queries.
fn assert_attempt_job_name(
    fixture: &Fixture,
    local_artifacts: &Path,
    task_id: &str,
    script: &str,
    expected_prefix: &str,
) -> String {
    let intents: Vec<_> = std::fs::read_dir(local_artifacts.join(task_id))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".intent-")
        })
        .collect();
    assert_eq!(intents.len(), 1);
    let intent: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&intents[0]).unwrap()).unwrap();
    assert_eq!(intent["task_id"], task_id);
    let nonce = intent["nonce"].as_str().unwrap();
    assert_eq!(nonce.len(), 16);
    assert!(
        nonce
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    );
    assert_eq!(
        intents[0].file_name().unwrap().to_str().unwrap(),
        format!(".intent-{nonce}.json")
    );
    let expected_name = format!("{expected_prefix}{nonce}");
    assert_eq!(intent["job_name"], expected_name);
    let names: Vec<_> = script
        .lines()
        .filter_map(|line| line.strip_prefix("#SBATCH --job-name="))
        .collect();
    assert_eq!(names, [expected_name.as_str()]);
    assert_eq!(
        std::fs::read_to_string(fixture.path().join("last_job_name")).unwrap(),
        format!("{expected_name}\n")
    );
    expected_name
}

// ── tests ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn slurm_stack_runs_process_task_end_to_end() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let local_artifacts = tempfile::tempdir().unwrap();
    let coordinator = tempfile::tempdir().unwrap();
    let input_path = coordinator.path().join("input.txt");
    std::fs::write(&input_path, "remote input").unwrap();

    std::fs::write(fixture.path().join("sacct_out"), "4242|COMPLETED|0:0\n").unwrap();
    let task_id = "slurm-e2e";
    // The fake sbatch does not execute job scripts, so pre-stage the output
    // file the declared glob will collect.
    stage_output_file(cluster.path(), task_id);
    let submission = process_submission(
        task_id,
        &input_path,
        &echo_process(),
        TaskResources::default(),
    );
    let executor = slurm_stack(
        &fixture,
        cluster.path(),
        local_artifacts.path(),
        coordinator.path(),
    );

    let result = executor.run(execution(submission)).await;
    let JobResult::Success {
        outputs, details, ..
    } = result
    else {
        panic!("slurm execution should succeed: {result:?}");
    };

    // Output restored into a channel value, exactly like the local backend.
    let channel = outputs.get(&0).unwrap().as_channel().unwrap();
    assert_eq!(channel.items.len(), 1);

    let details = details.unwrap();
    assert_eq!(details.exit_code, Some(0));
    let downloaded = &details.output_artifacts_by_port[&0][0];
    assert!(downloaded.path.contains("output-0-0"));
    assert_eq!(
        std::fs::read_to_string(&downloaded.path).unwrap(),
        r#"[{"value":42}]"#
    );

    // The staged input was uploaded to the (fake) cluster scratch.
    let remote_dir = fixture.submitted_task_dir(cluster.path(), task_id);
    assert_ne!(remote_dir, task_dir(cluster.path(), task_id));
    let remote_input = remote_dir.join("input-0-0");
    assert_eq!(
        std::fs::read_to_string(&remote_input).unwrap(),
        "remote input"
    );

    // The generated job script carries the sbatch directives and the
    // single-quoted environment contract.
    let script = std::fs::read_to_string(remote_dir.join("job.sbatch")).unwrap();
    let expected_dir = remote_dir.to_string_lossy().into_owned();
    assert!(script.contains("#!/bin/bash\n"));
    assert_attempt_job_name(
        &fixture,
        local_artifacts.path(),
        task_id,
        &script,
        "auto-slurm-e2e-",
    );
    assert!(script.contains("#SBATCH --partition=debug64c512g\n"));
    assert!(script.contains("#SBATCH --cpus-per-task=1\n"));
    assert!(script.contains("#SBATCH --mem-per-cpu=8G\n"));
    assert!(script.contains("#SBATCH --time=01:00:00\n"));
    assert!(script.contains(&format!("#SBATCH --output={expected_dir}/slurm-%j.out\n")));
    assert!(script.contains(&format!("#SBATCH --error={expected_dir}/slurm-%j.err\n")));
    assert!(script.contains(&format!("export AUTONOMICS_TASK_ID='{task_id}'\n")));

    // The manifest was written remotely and downloaded for custody.
    let namespace = remote_dir.file_name().unwrap();
    let manifest = local_artifacts.path().join(namespace).join("task.json");
    assert_eq!(
        details.task_manifest.as_ref().unwrap().path,
        manifest.to_string_lossy()
    );
    assert!(Path::new(&downloaded.path).starts_with(local_artifacts.path().join(namespace)));
    assert!(manifest.is_file());
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(manifest).unwrap()).unwrap();
    assert_eq!(manifest["spec"]["id"], task_id);
    assert_eq!(
        manifest["spec"]["spec"]["_autonomics_attempt_key"],
        namespace.to_str().unwrap()
    );
    let admitted = &manifest["spec"]["spec"]["_autonomics_input_artifacts"]["f"][0];
    assert_eq!(admitted["path"], remote_input.to_str().unwrap());
    assert_eq!(admitted["fingerprint"]["size"], "remote input".len() as u64);
    assert_eq!(
        admitted["fingerprint"]["content_hash"],
        sha256_hex(b"remote input")
    );

    // sbatch got the generated script and produced a numeric job id
    // (shim call logs carry only the arguments, no argv[0]).
    assert!(
        fixture
            .calls_log()
            .lines()
            .any(|line| line.starts_with("--parsable ")),
        "calls.log: {}",
        fixture.calls_log()
    );
}

#[tokio::test]
async fn repeated_logical_task_uses_distinct_remote_and_local_attempt_namespaces() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let local_artifacts = tempfile::tempdir().unwrap();
    let coordinator = tempfile::tempdir().unwrap();
    let executor = slurm_stack(
        &fixture,
        cluster.path(),
        local_artifacts.path(),
        coordinator.path(),
    );
    let task_id = "repeated-logical-task";
    let input = coordinator.path().join("input.txt");
    let mut downloaded = Vec::new();
    for index in 0..2 {
        let job_id = 4242 + index;
        std::fs::write(fixture.path().join("job_id"), format!("{job_id}\n")).unwrap();
        std::fs::write(
            fixture.path().join("sacct_out"),
            format!("{job_id}|COMPLETED|0:0\n"),
        )
        .unwrap();
        let bytes = format!("[{{\"value\":{}}}]", 42 + index);
        stage_output_file(cluster.path(), task_id);
        std::fs::write(task_dir(cluster.path(), task_id).join("out-0.json"), &bytes).unwrap();
        std::fs::write(&input, format!("input attempt {index}")).unwrap();
        let result = executor
            .run(execution(process_submission(
                task_id,
                &input,
                &echo_process(),
                TaskResources::default(),
            )))
            .await;
        let JobResult::Success {
            outputs,
            details: Some(details),
            ..
        } = result
        else {
            panic!("repeat attempt {index} should succeed: {result:?}");
        };
        assert_eq!(
            outputs[&0].as_channel().unwrap().items,
            vec![serde_json::json!({"value":42 + index})]
        );
        let file = details.output_artifacts_by_port[&0][0].clone();
        assert_eq!(std::fs::read_to_string(&file.path).unwrap(), bytes);
        downloaded.push(file.path);
    }
    let attempts = fixture.submitted_task_dirs(cluster.path(), task_id);
    assert_eq!(attempts.len(), 2);
    assert_ne!(attempts[0], attempts[1]);
    assert_ne!(downloaded[0], downloaded[1]);
    for (index, attempt) in attempts.iter().enumerate() {
        assert_ne!(attempt, &task_dir(cluster.path(), task_id));
        assert_eq!(
            std::fs::read_to_string(attempt.join("input-0-0")).unwrap(),
            format!("input attempt {index}")
        );
        assert!(
            Path::new(&downloaded[index])
                .starts_with(local_artifacts.path().join(attempt.file_name().unwrap()))
        );
        // Later attempts cannot replace the first attempt's downloaded result.
        assert_eq!(
            std::fs::read_to_string(&downloaded[index]).unwrap(),
            format!("[{{\"value\":{}}}]", 42 + index)
        );
    }
}

#[tokio::test]
async fn sbatch_script_reflects_resources_and_settings() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let transport =
        SlurmTaskTransport::new(slurm_settings(&fixture, cluster.path(), cluster.path()));

    let input_path = cluster.path().join("input.txt");
    std::fs::write(&input_path, "x").unwrap();
    let staged = cluster.path().join("staged-input");
    std::fs::write(&staged, "x").unwrap();
    let mut process = echo_process();
    process.command = vec!["true".into()];
    process.script = None;

    let mut submission = process_submission(
        "slurm-directives",
        &input_path,
        &process,
        TaskResources {
            cpus: Some(2),
            memory_bytes: Some(5 * 1024 * 1024 + 1),
            max_duration_ms: Some(90_000),
        },
    );
    // Transport-level call: stage the input path the way the remote
    // executor would after uploading.
    submission.spec.inputs[0].staged_paths = vec![staged.to_string_lossy().into_owned()];
    let dispatch: TaskDispatch = submission.into_remote_dispatch().unwrap();

    transport
        .submit(
            dispatch,
            vec![FileRef::local(&staged, Some("text".into())).unwrap()],
        )
        .await
        .unwrap();

    let script =
        std::fs::read_to_string(task_dir(cluster.path(), "slurm-directives").join("job.sbatch"))
            .unwrap();
    assert_attempt_job_name(
        &fixture,
        cluster.path(),
        "slurm-directives",
        &script,
        "auto-slurm-directives-",
    );
    assert!(script.contains("#SBATCH --cpus-per-task=2\n"));
    assert!(!script.contains("#SBATCH --gres="));
    // 5 MiB + 1 byte rounds up to 6 MiB.
    assert!(script.contains("#SBATCH --mem=6M\n"));
    assert!(script.contains("#SBATCH --time=00:01:30\n"));
    // command tasks render as single-quoted argv.
    assert!(script.contains("'true'"));
    assert!(!script.contains("#SBATCH --mem-per-cpu"));
}

#[tokio::test]
async fn gpu_allocation_is_rendered_and_bound_to_manifest() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let mut settings = slurm_settings(&fixture, cluster.path(), cluster.path());
    settings.partition = "a100".into();
    settings.account = Some("operator-account".into());
    settings.gpus = Some(2);
    settings.qos = Some("debug".into());
    let transport = SlurmTaskTransport::new(settings.clone());
    let input = cluster.path().join("staged");
    std::fs::write(&input, "synthetic").unwrap();
    let process = echo_process();
    let mut submission =
        process_submission("gpu-binding", &input, &process, TaskResources::default());
    submission.spec.inputs[0].staged_paths = vec![input.to_string_lossy().into_owned()];
    let dispatch = submission.into_remote_dispatch().unwrap();
    let artifacts = vec![FileRef::local(&input, None).unwrap()];
    transport
        .submit(dispatch.clone(), artifacts.clone())
        .await
        .unwrap();
    let directory = task_dir(cluster.path(), "gpu-binding");
    let script = std::fs::read_to_string(directory.join("job.sbatch")).unwrap();
    assert!(script.contains("#SBATCH --partition=a100\n"));
    assert!(script.contains("#SBATCH --account=operator-account\n"));
    assert!(script.contains("#SBATCH --gres=gpu:2\n"));
    assert!(script.contains("#SBATCH --nodes=1\n#SBATCH --ntasks=1\n"));
    assert!(script.contains("#SBATCH --qos=debug\n"));
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.join("task.json")).unwrap()).unwrap();
    assert_eq!(
        manifest["slurm_gpu_allocation"],
        serde_json::json!({
            "partition": "a100", "account": "operator-account", "gpus": 2, "qos": "debug"
        })
    );
    // A changed GPU count cannot adopt an existing attempt's manifest.
    settings.gpus = Some(1);
    assert!(
        SlurmTaskTransport::new(settings.clone())
            .submit(dispatch.clone(), artifacts.clone())
            .await
            .is_err()
    );
    // Zero must be rejected before any SSH/submission work.
    let calls_before = fixture.calls_log();
    settings.gpus = Some(0);
    assert!(
        SlurmTaskTransport::new(settings)
            .submit(dispatch, artifacts)
            .await
            .is_err()
    );
    assert_eq!(fixture.calls_log(), calls_before);
}

#[tokio::test]
async fn sbatch_job_name_preserves_distinct_attempt_nonce_at_length_limit() {
    let fixture = Fixture::new();
    let task_id = "x".repeat(80);
    // 63 bytes minus "auto-", the separator, and the full 16-byte hex tag.
    let expected_prefix = format!("auto-{}-", "x".repeat(41));
    let mut names = Vec::new();
    for _ in 0..2 {
        // Separate workspaces represent independent attempts of the same task;
        // duplicate submission in one workspace must still retain its old job.
        let cluster = tempfile::tempdir().unwrap();
        let transport = transport_of(&fixture, cluster.path());
        let (dispatch, artifacts) = prepare_simple(cluster.path(), &task_id);
        transport.submit(dispatch, artifacts).await.unwrap();
        let script =
            std::fs::read_to_string(task_dir(cluster.path(), &task_id).join("job.sbatch")).unwrap();
        let name = assert_attempt_job_name(
            &fixture,
            cluster.path(),
            &task_id,
            &script,
            &expected_prefix,
        );
        assert_eq!(name.len(), 63);
        names.push(name);
    }
    assert_ne!(names[0], names[1]);
}

#[tokio::test]
async fn wait_maps_failed_state() {
    // Accounting already normalizes signal exits to 128+signal. Preserve that
    // value (including when rc is also nonzero); a zero FAILED exit becomes 1.
    for (record, expected_exit) in [
        ("4242|FAILED|2:0", 2),
        ("4242|FAILED|0:9", 137),
        ("4242|FAILED|2:15", 143),
        ("4242|FAILED|0:0", 1),
    ] {
        let receipt = wait_with_sacct(record, "slurm-failed").await.unwrap();
        assert_eq!(receipt.status, "failed", "record {record}");
        assert_eq!(receipt.exit_code, expected_exit, "record {record}");
        assert!(receipt.output_artifacts_by_port.is_empty());
    }
}

#[tokio::test]
async fn failed_slurm_logs_survive_transport_and_are_exposed_in_failure_details() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let artifacts = tempfile::tempdir().unwrap();
    let coordinator = tempfile::tempdir().unwrap();
    std::fs::write(fixture.path().join("sacct_out"), "4242|FAILED|17:0\n").unwrap();
    std::fs::write(
        fixture.path().join("job-log.err"),
        "file_reference: input exists only on coordinator\n",
    )
    .unwrap();
    std::fs::write(fixture.path().join("job-log.out"), "worker started\n").unwrap();
    let source = coordinator.path().join("input.txt");
    std::fs::write(&source, "input").unwrap();
    let executor = slurm_stack(
        &fixture,
        cluster.path(),
        artifacts.path(),
        coordinator.path(),
    );
    let result = executor
        .run(execution(process_submission(
            "logs-failed",
            &source,
            &echo_process(),
            TaskResources::default(),
        )))
        .await;
    let JobResult::Failed {
        error,
        details: Some(details),
        ..
    } = result
    else {
        panic!("expected failure")
    };
    assert_eq!(details.exit_code, Some(17));
    assert!(
        error
            .to_string()
            .contains("input exists only on coordinator")
    );
    let stderr = details.stderr_log.as_ref().unwrap();
    assert_eq!(
        std::fs::read_to_string(&stderr.path).unwrap(),
        "file_reference: input exists only on coordinator\n"
    );
    assert!(stderr.fingerprint.as_ref().unwrap().content_hash.is_some());
    assert_eq!(
        std::fs::read_to_string(&details.stdout_log.unwrap().path).unwrap(),
        "worker started\n"
    );
    let attempt: TaskAttemptReceipt = serde_json::from_slice(
        &std::fs::read(Path::new(details.workspace.as_ref().unwrap()).join("attempt.json"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(attempt.stderr_log.unwrap().path, stderr.path);
    let raw = fixture
        .submitted_task_dir(cluster.path(), "logs-failed")
        .join("slurm-4242.err");
    assert!(raw.is_file(), "raw scheduler logs remain retained");
}

#[tokio::test]
async fn slurm_log_capture_failure_keeps_the_exit_and_reports_the_cause() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let artifacts = tempfile::tempdir().unwrap();
    let coordinator = tempfile::tempdir().unwrap();
    std::fs::write(fixture.path().join("sacct_out"), "4242|FAILED|17:0\n").unwrap();
    let python = fixture.path().join("python3");
    std::fs::write(
        &python,
        r#"#!/bin/sh
case "$*" in
  *'invalid Slurm log identity'*) echo 'synthetic log capture interpreter failure' >&2; exit 1;;
esac
exec /usr/bin/python3 "$@"
"#,
    )
    .unwrap();
    make_executable(&python);
    let source = coordinator.path().join("input.txt");
    std::fs::write(&source, "input").unwrap();
    let result = slurm_stack(
        &fixture,
        cluster.path(),
        artifacts.path(),
        coordinator.path(),
    )
    .run(execution(process_submission(
        "logs-unavailable",
        &source,
        &echo_process(),
        TaskResources::default(),
    )))
    .await;
    let JobResult::Failed {
        error,
        details: Some(details),
        ..
    } = result
    else {
        panic!("expected failure")
    };
    assert_eq!(details.exit_code, Some(17));
    assert!(
        error
            .to_string()
            .contains("synthetic log capture interpreter failure"),
        "{error}"
    );
    let log = details.stderr_log.unwrap();
    assert!(
        std::fs::read_to_string(&log.path)
            .unwrap()
            .contains("remote log collection failed")
    );
}

#[tokio::test]
async fn slurm_log_capture_is_bounded_and_does_not_change_a_signal_exit() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    std::fs::write(fixture.path().join("sacct_out"), "4242|FAILED|0:9\n").unwrap();
    let transport = transport_of(&fixture, cluster.path());
    let lease = submit_simple(&transport, cluster.path(), "large-logs").await;
    let raw = task_dir(cluster.path(), "large-logs").join("slurm-4242.err");
    let text = format!("{}final failure\n", "x".repeat(200_000));
    std::fs::write(&raw, &text).unwrap();
    let receipt = transport.wait(&lease).await.unwrap();
    assert_eq!(receipt.exit_code, 137);
    let snapshot = std::fs::read_to_string(receipt.stderr_log.unwrap().path).unwrap();
    assert!(snapshot.starts_with("[truncated:"));
    assert!(snapshot.ends_with("final failure\n"));
    assert!(snapshot.len() < 66 * 1024);
    assert_eq!(std::fs::read_to_string(raw).unwrap(), text);
}

#[tokio::test]
async fn wait_maps_timeout_state() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    std::fs::write(fixture.path().join("sacct_out"), "4242|TIMEOUT|0:0\n").unwrap();
    let transport = transport_of(&fixture, cluster.path());
    let lease = submit_simple(&transport, cluster.path(), "slurm-timeout").await;
    let receipt = transport.wait(&lease).await.unwrap();
    assert_eq!(receipt.status, "timeout");
    assert_eq!(receipt.exit_code, 124);
}

#[tokio::test]
async fn wait_maps_cancelled_state() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    std::fs::write(
        fixture.path().join("sacct_out"),
        "4242|CANCELLED by 12345|0:0\n",
    )
    .unwrap();
    let transport = transport_of(&fixture, cluster.path());
    let lease = submit_simple(&transport, cluster.path(), "slurm-cancelled").await;
    let receipt = transport.wait(&lease).await.unwrap();
    assert_eq!(receipt.status, "cancelled");
    assert_eq!(receipt.exit_code, 130);
}

#[tokio::test]
async fn cancel_marks_lease_and_runs_scancel() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let transport = transport_of(&fixture, cluster.path());
    let lease = submit_simple(&transport, cluster.path(), "slurm-scancel").await;

    std::fs::write(fixture.path().join("sacct_out"), "4242|CANCELLED|0:0\n").unwrap();
    transport.cancel(&lease).await.unwrap();
    assert!(fixture.path().join("cancelled").is_file());
    // Shim call logs carry only the arguments (no argv[0]): the scancel
    // line is the bare job id.
    assert!(
        fixture.calls_log().lines().any(|line| line == "4242"),
        "calls.log: {}",
        fixture.calls_log()
    );

    let error = transport.wait(&lease).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unknown, cancelled, or already consumed"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn wait_rejects_unknown_lease() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let error = transport_of(&fixture, cluster.path())
        .wait(&TaskLease {
            lease_id: "999".into(),
            task_id: "ghost".into(),
        })
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unknown, cancelled, or already consumed")
    );
}

#[tokio::test]
async fn wait_polls_squeue_until_job_leaves_queue() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    std::fs::write(fixture.path().join("queued"), "").unwrap();
    std::fs::write(fixture.path().join("sacct_out"), "4242|COMPLETED|0:0\n").unwrap();
    stage_output_file(cluster.path(), "slurm-polling");

    let transport = transport_of(&fixture, cluster.path());
    let lease = submit_simple(&transport, cluster.path(), "slurm-polling").await;
    // The job leaves the queue shortly after submission.
    let queued = fixture.path().join("queued");
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        let _ = std::fs::remove_file(&queued);
    });

    let receipt = transport.wait(&lease).await.unwrap();
    assert_eq!(receipt.status, "success");
    assert_eq!(receipt.exit_code, 0);
    assert_eq!(
        receipt.output_artifacts_by_port[&0][0].format.as_deref(),
        Some("json")
    );
    assert!(
        receipt.output_artifacts_by_port[&0][0]
            .path
            .ends_with("output-0-0")
    );

    // Shim call logs carry only the arguments (no argv[0]).
    let polls = fixture
        .calls_log()
        .lines()
        .filter(|line| line.starts_with("-h -j 4242"))
        .count();
    assert!(polls >= 2, "expected repeated squeue polls, got {polls}");
}

#[tokio::test]
async fn receipt_pins_exact_submitted_manifest_including_heredoc_newline() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    std::fs::write(fixture.path().join("sacct_out"), "4242|COMPLETED|0:0\n").unwrap();
    let transport = transport_of(&fixture, cluster.path());
    let (mut dispatch, artifacts) = prepare_simple(cluster.path(), "manifest-custody");
    // Escaped NUL/newlines remain data in JSON. A literal batch delimiter in
    // the script also exercises the bootstrap's dynamic quoted delimiter.
    let literal = "snowman \u{2603}\nAUTONOMICS_MANIFEST_EOF\n$HOME `literal`\0";
    dispatch.spec.spec["custody_literal"] = serde_json::json!(literal);
    dispatch.spec.spec["script"] =
        serde_json::json!("# synthetic script, never run by fake sbatch\nAUTONOMICS_SBATCH_EOF\n");
    let lease = transport.submit(dispatch, artifacts).await.unwrap();
    let manifest_path = task_dir(cluster.path(), "manifest-custody").join("task.json");
    let submitted = std::fs::read(&manifest_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&submitted).unwrap();
    assert_eq!(parsed["spec"]["spec"]["custody_literal"], literal);
    assert!(submitted.ends_with(b"}\n"));
    assert!(!submitted.contains(&0), "NUL must remain JSON escaped");
    let receipt = transport.wait(&lease).await.unwrap();
    let expected = FileFingerprint {
        size: submitted.len() as u64,
        mtime_ns: 0,
        content_hash: Some(sha256_hex(&submitted)),
        immutable_remote: false,
    };
    assert_eq!(receipt.task_manifest.fingerprint, Some(expected.clone()));
    let store = SshArtifactStore::new(
        fixture.ssh_target(),
        cluster.path().to_string_lossy(),
        local.path(),
    );
    let downloaded = store
        .download(receipt.task_manifest, "manifest-custody", "task.json")
        .await
        .unwrap();
    assert_eq!(downloaded.fingerprint, Some(expected));
    assert_eq!(std::fs::read(downloaded.path).unwrap(), submitted);
}

#[tokio::test]
async fn changed_manifest_cannot_replace_the_pre_submit_pin_on_success_or_failure() {
    for (state, expected_status) in [
        ("4242|COMPLETED|0:0\n", "success"),
        ("4242|FAILED|1:0\n", "failed"),
    ] {
        let fixture = Fixture::new();
        let cluster = tempfile::tempdir().unwrap();
        let local = tempfile::tempdir().unwrap();
        std::fs::write(fixture.path().join("sacct_out"), state).unwrap();
        let transport = transport_of(&fixture, cluster.path());
        let lease = submit_simple(&transport, cluster.path(), "manifest-original").await;
        let path = task_dir(cluster.path(), "manifest-original").join("task.json");
        let submitted = std::fs::read(&path).unwrap();
        // Same-size, still-valid JSON mutation before wait: neither receipt
        // generation nor download may bless the new bytes as the submission.
        let changed = String::from_utf8(submitted.clone())
            .unwrap()
            .replace("manifest-original", "manifest-modified")
            .into_bytes();
        assert_ne!(submitted, changed);
        assert_eq!(submitted.len(), changed.len());
        std::fs::write(&path, &changed).unwrap();
        let receipt = transport.wait(&lease).await.unwrap();
        assert_eq!(receipt.status, expected_status);
        let fingerprint = receipt.task_manifest.fingerprint.as_ref().unwrap();
        assert_eq!(fingerprint.size, submitted.len() as u64);
        assert_eq!(
            fingerprint.content_hash.as_deref(),
            Some(sha256_hex(&submitted).as_str())
        );
        assert!(!fingerprint.immutable_remote);
        let store = SshArtifactStore::new(
            fixture.ssh_target(),
            cluster.path().to_string_lossy(),
            local.path(),
        );
        let error = store
            .download(receipt.task_manifest, "verified-manifest", "task.json")
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("artifact fingerprint mismatch"),
            "{error}"
        );
        let destination = local.path().join("verified-manifest");
        assert!(!destination.join("task.json").exists());
        assert!(std::fs::read_dir(destination).unwrap().next().is_none());
        assert_eq!(
            std::fs::read(path).unwrap(),
            changed,
            "never repair evidence"
        );
    }
}

#[tokio::test]
async fn missing_output_port_is_a_hard_error() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    std::fs::write(fixture.path().join("sacct_out"), "4242|COMPLETED|0:0\n").unwrap();

    let transport = transport_of(&fixture, cluster.path());
    let lease = submit_simple(&transport, cluster.path(), "slurm-missing").await;
    // The job "ran" but produced nothing: remove the staged file so the
    // declared port matches nothing at collect time.
    let _ = std::fs::remove_file(task_dir(cluster.path(), "slurm-missing").join("out-0.json"));
    let error = transport.wait(&lease).await.unwrap_err();
    assert!(
        error.to_string().contains("output port 0 matched no files"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn routing_executor_sends_process_tasks_to_slurm_and_keeps_typed_local() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let local_artifacts = tempfile::tempdir().unwrap();
    let coordinator = tempfile::tempdir().unwrap();
    std::fs::write(fixture.path().join("sacct_out"), "4242|COMPLETED|0:0\n").unwrap();
    let executor = slurm_stack(
        &fixture,
        cluster.path(),
        local_artifacts.path(),
        coordinator.path(),
    );

    // A process task routes to the Slurm stack.
    let input_path = coordinator.path().join("input.txt");
    std::fs::write(&input_path, "routed").unwrap();
    stage_output_file(cluster.path(), "routed-process");
    let result = executor
        .run(execution(process_submission(
            "routed-process",
            &input_path,
            &echo_process(),
            TaskResources::default(),
        )))
        .await;
    assert!(matches!(result, JobResult::Success { .. }), "{result:?}");
    let remote_dir = fixture.submitted_task_dir(cluster.path(), "routed-process");
    assert_ne!(remote_dir, task_dir(cluster.path(), "routed-process"));

    // A typed task stays on the local executor (remote path ignores the
    // node payload, so it must never see one).
    let result = executor.run(execution(noop_submission("local-noop"))).await;
    assert!(matches!(result, JobResult::Success { .. }), "{result:?}");
    assert!(!cluster.path().join("local-noop").exists());
}

#[tokio::test]
async fn upload_resolves_tilde_scratch_to_absolute_paths() {
    let fixture = Fixture::new();
    let store = SshArtifactStore::new(
        fixture.ssh_target(),
        "~/slurm-store-fix",
        fixture.path().join("local-root"),
    );
    // `cd && pwd` runs in a per-fixture child-only HOME. The TempDir owns
    // all uploads, including failed assertions; no real HOME is touched.
    let home = fixture.path().join("home");

    let payload = fixture.path().join("payload.txt");
    std::fs::write(&payload, "tilde fix").unwrap();

    // A tilde scratch must never leak into the returned artifact path:
    // single-quoted ssh commands and sbatch directives do not expand it.
    let first = store
        .upload(
            FileRef::local(&payload, Some("text".into())).unwrap(),
            "task-a",
            "input-0-0",
        )
        .await
        .unwrap();
    assert!(
        !first.path.starts_with('~'),
        "uploaded path must be absolute: {}",
        first.path
    );
    assert_eq!(
        Path::new(&first.path),
        home.join("slurm-store-fix/task-a/input-0-0")
    );
    assert_eq!(std::fs::read_to_string(&first.path).unwrap(), "tilde fix");

    // The home resolution is cached: a second upload must not re-run it.
    let second = store
        .upload(
            FileRef::local(&payload, Some("text".into())).unwrap(),
            "task-a",
            "input-0-1",
        )
        .await
        .unwrap();
    assert!(
        second.path.ends_with("/task-a/input-0-1"),
        "{}",
        second.path
    );
    let home_resolutions = fixture
        .ssh_calls_log()
        .lines()
        .filter(|line| *line == "cd && pwd")
        .count();
    assert_eq!(
        home_resolutions,
        1,
        "ssh_calls.log: {}",
        fixture.ssh_calls_log()
    );

    // Another fixture may upload the same task/path safely: its child HOME
    // is different, without changing global HOME or relaxing no-clobber.
    let independent = Fixture::new();
    let other_store = SshArtifactStore::new(
        independent.ssh_target(),
        "~/slurm-store-fix",
        independent.path().join("local-root"),
    );
    let other = other_store
        .upload(
            FileRef::local(&payload, Some("text".into())).unwrap(),
            "task-a",
            "input-0-0",
        )
        .await
        .unwrap();
    assert_eq!(
        Path::new(&other.path),
        independent
            .path()
            .join("home/slurm-store-fix/task-a/input-0-0")
    );
    assert_ne!(first.path, other.path);
    assert_eq!(std::fs::read_to_string(&first.path).unwrap(), "tilde fix");
    assert_eq!(std::fs::read_to_string(&other.path).unwrap(), "tilde fix");
}

// ── helpers over the transport directly ──────────────────────────────────

fn transport_of(fixture: &Fixture, cluster: &Path) -> SlurmTaskTransport {
    SlurmTaskTransport::new(slurm_settings(fixture, cluster, cluster))
}

/// Prepare one immutable fixture submission. Reusing a work order must not
/// rewrite its inputs: FileRef::local intentionally binds metadata identity.
fn prepare_simple(cluster: &Path, task_id: &str) -> (TaskDispatch, Vec<FileRef>) {
    stage_output_file(cluster, task_id);
    let input_path = cluster.join(format!("{task_id}-source.txt"));
    std::fs::write(&input_path, "content").unwrap();

    let mut submission = process_submission(
        task_id,
        &input_path,
        &echo_process(),
        TaskResources::default(),
    );
    let staged = cluster.join(format!("{task_id}-staged-input"));
    std::fs::write(&staged, "content").unwrap();
    submission.spec.inputs[0].staged_paths = vec![staged.to_string_lossy().into_owned()];
    let dispatch: TaskDispatch = submission.into_remote_dispatch().unwrap();

    (
        dispatch,
        vec![FileRef::local(&staged, Some("text".into())).unwrap()],
    )
}

/// Submit one fixture work order. Accounting/queue state stays test-controlled.
async fn submit_simple(transport: &SlurmTaskTransport, cluster: &Path, task_id: &str) -> TaskLease {
    let (dispatch, artifacts) = prepare_simple(cluster, task_id);
    transport.submit(dispatch, artifacts).await.unwrap()
}

#[tokio::test]
async fn duplicate_submission_recovers_original_without_new_sbatch() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let transport = transport_of(&fixture, cluster.path());
    let (dispatch, artifacts) = prepare_simple(cluster.path(), "duplicate");
    let first = transport
        .submit(dispatch.clone(), artifacts.clone())
        .await
        .unwrap();
    let directory = task_dir(cluster.path(), "duplicate");
    let originals: Vec<_> = [
        "task.json",
        "job.sbatch",
        "submission.identity",
        "submission.job-id",
    ]
    .into_iter()
    .map(|name| (name, std::fs::read(directory.join(name)).unwrap()))
    .collect();
    let second = transport
        .submit(dispatch.clone(), artifacts.clone())
        .await
        .unwrap();
    assert_eq!(first.lease_id, second.lease_id);
    // The repair does not make a different request look like the original.
    // A changed resource contract must not reuse the old allocation or submit
    // another job, and the existing identity/manifest/script must remain exact.
    let mut different = dispatch;
    different.resources.cpus = Some(2);
    let error = transport.submit(different, artifacts).await.unwrap_err();
    assert!(error.to_string().contains("identity mismatch"), "{error}");
    for (name, raw) in originals {
        assert_eq!(std::fs::read(directory.join(name)).unwrap(), raw);
    }
    assert_eq!(
        fixture
            .calls_log()
            .lines()
            .filter(|s| s.starts_with("--parsable "))
            .count(),
        1
    );
}

fn recovery_intent(local: &Path, task: &str) -> dag_core::SlurmSubmissionIntent {
    let files: Vec<_> = std::fs::read_dir(local.join(task))
        .unwrap()
        .map(|v| v.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".intent-")
        })
        .collect();
    assert_eq!(files.len(), 1);
    dag_core::SlurmSubmissionIntent::read(&files[0]).unwrap()
}

fn fixture_ownership(fixture: &Fixture, intent: &dag_core::SlurmSubmissionIntent) {
    let user = std::process::Command::new("/usr/bin/id")
        .arg("-un")
        .output()
        .unwrap();
    assert!(user.status.success());
    let user = String::from_utf8(user.stdout).unwrap();
    std::fs::write(
        fixture.path().join("sacct_ownership_out"),
        format!(
            "4242|{}|autonomics-{}|{}|{}\n",
            user.trim(),
            intent.submission_identity,
            intent.remote_task_dir,
            intent.job_name
        ),
    )
    .unwrap();
}

#[tokio::test]
async fn recovery_observes_original_after_transport_restart_without_submission() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    let settings = slurm_settings(&fixture, cluster.path(), local.path());
    let transport = SlurmTaskTransport::new(settings.clone());
    let original = submit_simple(&transport, cluster.path(), "restart-observe").await;
    let intent = recovery_intent(local.path(), "restart-observe");
    fixture_ownership(&fixture, &intent);
    // The actual site does not retain scheduler comments in accounting.
    // Recovery must use the nonce name recorded before submission, never a
    // name learned from the scheduler response being validated.
    let ownership = fixture.path().join("sacct_ownership_out");
    let row = std::fs::read_to_string(&ownership).unwrap();
    std::fs::write(
        &ownership,
        row.replace(&format!("autonomics-{}", intent.submission_identity), ""),
    )
    .unwrap();
    drop(transport);
    let restarted = SlurmTaskTransport::new(settings);
    let observed = restarted.inspect_submission_intent(&intent).await.unwrap();
    assert_eq!(observed.state, "IDENTITY_VERIFIED_JOB_FOUND");
    assert_eq!(observed.job_id, Some(original.lease_id));
    assert!(!observed.resubmission_authorized);
    assert_eq!(
        fixture
            .calls_log()
            .lines()
            .filter(|l| l.starts_with("--parsable "))
            .count(),
        1
    );
    assert!(!fixture.path().join("cancelled").exists());
}

#[tokio::test]
async fn recovery_missing_job_response_uses_original_identity_without_replay() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    let transport = SlurmTaskTransport::new(slurm_settings(&fixture, cluster.path(), local.path()));
    let original = submit_simple(&transport, cluster.path(), "lost-response-observe").await;
    let intent = recovery_intent(local.path(), "lost-response-observe");
    fixture_ownership(&fixture, &intent);
    let response = Path::new(&intent.remote_task_dir).join("submission.job-id");
    std::fs::remove_file(&response).unwrap();
    let observed = transport.inspect_submission_intent(&intent).await.unwrap();
    assert_eq!(observed.job_id, Some(original.lease_id));
    assert!(!observed.resubmission_authorized);
    assert!(!response.exists());
    assert_eq!(
        fixture
            .calls_log()
            .lines()
            .filter(|l| l.starts_with("--parsable "))
            .count(),
        1
    );
}

#[tokio::test]
async fn recovery_changed_settings_or_manifest_cannot_authorize_reuse() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    let settings = slurm_settings(&fixture, cluster.path(), local.path());
    let transport = SlurmTaskTransport::new(settings.clone());
    submit_simple(&transport, cluster.path(), "changed-observe").await;
    let intent = recovery_intent(local.path(), "changed-observe");
    fixture_ownership(&fixture, &intent);
    let before = fixture.ssh_calls_log();
    let mut changed = settings;
    changed.partition = "other".into();
    assert!(
        SlurmTaskTransport::new(changed)
            .inspect_submission_intent(&intent)
            .await
            .is_err()
    );
    assert_eq!(fixture.ssh_calls_log(), before);
    std::fs::write(
        Path::new(&intent.remote_task_dir).join("task.json"),
        b"changed",
    )
    .unwrap();
    assert!(transport.inspect_submission_intent(&intent).await.is_err());
    assert_eq!(
        fixture
            .calls_log()
            .lines()
            .filter(|l| l.starts_with("--parsable "))
            .count(),
        1
    );
    assert!(!fixture.path().join("cancelled").exists());
}

#[tokio::test]
async fn recovery_empty_scheduler_evidence_never_resubmits_or_discards_intent() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    let transport = SlurmTaskTransport::new(slurm_settings(&fixture, cluster.path(), local.path()));
    submit_simple(&transport, cluster.path(), "unknown-observe").await;
    let intent = recovery_intent(local.path(), "unknown-observe");
    let result = transport.inspect_submission_intent(&intent).await.unwrap();
    assert_eq!(result.state, "UNRESOLVED");
    assert!(result.job_id.is_none());
    assert!(!result.resubmission_authorized);
    assert_eq!(
        fixture
            .calls_log()
            .lines()
            .filter(|l| l.starts_with("--parsable "))
            .count(),
        1
    );
    assert!(
        Path::new(&intent.remote_task_dir)
            .join("submission.job-id")
            .exists()
    );
    assert_eq!(
        recovery_intent(local.path(), "unknown-observe").submission_identity,
        intent.submission_identity
    );
}

#[tokio::test]
async fn cancel_failure_retains_lease_and_workspace() {
    let fixture = Fixture::new();
    std::fs::write(fixture.path().join("scancel"), "#!/bin/sh\nexit 1\n").unwrap();
    let cluster = tempfile::tempdir().unwrap();
    let transport = transport_of(&fixture, cluster.path());
    let lease = submit_simple(&transport, cluster.path(), "cancel-failed").await;
    assert!(transport.cancel(&lease).await.is_err());
    assert!(task_dir(cluster.path(), "cancel-failed").exists());
    std::fs::write(fixture.path().join("sacct_out"), "4242|FAILED|2:0\n").unwrap();
    assert_eq!(transport.wait(&lease).await.unwrap().status, "failed");
}

#[tokio::test]
async fn malformed_accounting_never_succeeds() {
    for record in [
        "4242|COMPLETED|",
        "4242|COMPLETED|0",
        "4242|COMPLETED|x:0",
        "4242|COMPLETED|0:-1",
        "4242|COMPLETED|0:999",
        "4242|COMPLETED+|0:0",
        "4242|RUNNING|0:0",
        "4242|COMPLETED|0:0|extra",
        "4242|COMPLETED|0:0\n4242|FAILED|1:0",
    ] {
        let fixture = Fixture::new();
        let cluster = tempfile::tempdir().unwrap();
        std::fs::write(fixture.path().join("sacct_out"), record).unwrap();
        let transport = transport_of(&fixture, cluster.path());
        let lease = submit_simple(&transport, cluster.path(), "malformed").await;
        assert!(transport.wait(&lease).await.is_err(), "accepted {record}");
    }
}

#[tokio::test]
async fn nonzero_exit_or_signal_cannot_be_success() {
    for record in ["4242|COMPLETED|2:0", "4242|COMPLETED|0:9"] {
        let fixture = Fixture::new();
        let cluster = tempfile::tempdir().unwrap();
        std::fs::write(fixture.path().join("sacct_out"), record).unwrap();
        let transport = transport_of(&fixture, cluster.path());
        let lease = submit_simple(&transport, cluster.path(), "signal").await;
        assert_eq!(transport.wait(&lease).await.unwrap().status, "failed");
    }
}

#[tokio::test]
async fn interrupted_or_corrupt_download_preserves_destination() {
    for response in ["printf x; exit 255", "printf bad", "printf abcd"] {
        let fixture = Fixture::new();
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("download")).unwrap();
        let destination = root.path().join("download/output");
        std::fs::write(&destination, "previous").unwrap();
        // Metadata describes 'abc'; transfer is interrupted or same-size corruption.
        let shim = format!(
            "#!/bin/sh\nDIR=$(dirname -- \"$0\")\ncase \"$*\" in *'stat -c'*) printf '3\\nba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\\n';; *) printf '%s\\n' \"$$\" > \"$DIR/transfer.pid\"; {response};; esac\n"
        );
        std::fs::write(fixture.path().join("ssh"), shim).unwrap();
        let store = SshArtifactStore::new(fixture.ssh_target(), "/synthetic", root.path());
        let artifact = FileRef {
            domain: Default::default(),
            path: "/synthetic/file".into(),
            format: None,
            fingerprint: None,
        };
        assert!(
            store
                .download(artifact, "download", "output")
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(destination).unwrap(), "previous");
        assert_eq!(
            std::fs::read_dir(root.path().join("download"))
                .unwrap()
                .count(),
            1
        );
        #[cfg(target_os = "linux")]
        {
            let pid = std::fs::read_to_string(fixture.path().join("transfer.pid")).unwrap();
            assert!(pid.trim().parse::<u32>().unwrap() > 0);
            assert!(
                !Path::new("/proc").join(pid.trim()).exists(),
                "handled failed transfer returned before the local SSH process was reaped"
            );
        }
    }
}

#[tokio::test]
async fn disconnected_ssh_recovers_committed_job_without_replay() {
    let fixture = Fixture::new();
    let shim = FIXTURE_SSH.replace("exec /bin/sh -c \"$*\"", "if [ \"$*\" = \"bash -s\" ]; then IFS= read -r handshake; /bin/sh -c \"$handshake\"; /bin/sh -c \"$*\" >/dev/null; exit 255; fi\nexec /bin/sh -c \"$*\"");
    std::fs::write(fixture.path().join("ssh"), shim).unwrap();
    let cluster = tempfile::tempdir().unwrap();
    let transport = transport_of(&fixture, cluster.path());
    let lease = submit_simple(&transport, cluster.path(), "disconnect").await;
    assert_eq!(lease.lease_id, "4242");
    assert_eq!(
        fixture
            .calls_log()
            .lines()
            .filter(|s| s.starts_with("--parsable "))
            .count(),
        1
    );
}

#[tokio::test]
async fn queued_job_is_not_confirmed_cancelled() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let transport = transport_of(&fixture, cluster.path());
    let lease = submit_simple(&transport, cluster.path(), "still-running").await;
    std::fs::write(fixture.path().join("queued"), "").unwrap();
    assert!(transport.cancel(&lease).await.is_err());
    assert!(task_dir(cluster.path(), "still-running").exists());
}

#[tokio::test]
async fn missing_accounting_cannot_confirm_cancelled() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let transport = transport_of(&fixture, cluster.path());
    let lease = submit_simple(&transport, cluster.path(), "missing-accounting").await;
    assert!(transport.cancel(&lease).await.is_err());
    assert!(task_dir(cluster.path(), "missing-accounting").exists());
}

#[tokio::test]
async fn accounting_must_name_exact_allocation_for_wait_and_cancel() {
    for id in ["4243", "4242.0", "4242_1", "04242", "4242;other"] {
        let fixture = Fixture::new();
        let cluster = tempfile::tempdir().unwrap();
        let transport = transport_of(&fixture, cluster.path());
        let lease = submit_simple(&transport, cluster.path(), "wrong-accounting-id").await;
        std::fs::write(
            fixture.path().join("sacct_out"),
            format!("{id}|COMPLETED|0:0\n"),
        )
        .unwrap();
        assert!(transport.wait(&lease).await.is_err(), "wait accepted {id}");
        assert!(
            transport.cancel(&lease).await.is_err(),
            "cancel accepted {id}"
        );
        // Failure retains the original pending lease; an exact record can
        // still reconcile it, rather than inventing a cancellation receipt.
        std::fs::write(fixture.path().join("sacct_out"), "4242|FAILED|2:0\n").unwrap();
        assert_eq!(transport.wait(&lease).await.unwrap().status, "failed");
        assert!(
            fixture
                .ssh_calls_log()
                .contains("JobIDRaw,State%40,ExitCode")
        );
    }
}

#[cfg(target_os = "linux")]
fn successful_download_fixture(fixture: &Fixture) {
    std::fs::write(fixture.path().join("ssh"), "#!/bin/sh\ncase \"$*\" in *'stat -c'*) printf '3\\nba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\\n';; *) printf abc;; esac\n").unwrap();
}

#[cfg(target_os = "linux")]
fn synthetic_remote_file() -> FileRef {
    FileRef {
        domain: Default::default(),
        path: "/synthetic/file".into(),
        format: None,
        fingerprint: None,
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn successful_download_cannot_replace_existing_destination_or_foreign_partial() {
    let fixture = Fixture::new();
    successful_download_fixture(&fixture);
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("download");
    std::fs::create_dir(&directory).unwrap();
    let destination = directory.join("output");
    let foreign = directory.join(format!("output.part-{}-0", std::process::id()));
    std::fs::write(&destination, b"previous").unwrap();
    std::fs::write(&foreign, b"foreign").unwrap();
    let store = SshArtifactStore::new(fixture.ssh_target(), "/synthetic", root.path());
    assert!(
        store
            .download(synthetic_remote_file(), "download", "output")
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(destination).unwrap(), b"previous");
    assert_eq!(std::fs::read(foreign).unwrap(), b"foreign");
    assert_eq!(std::fs::read_dir(directory).unwrap().count(), 2);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn competing_successful_downloads_publish_exactly_one_held_inode() {
    let fixture = Fixture::new();
    successful_download_fixture(&fixture);
    let root = tempfile::tempdir().unwrap();
    let store = SshArtifactStore::new(fixture.ssh_target(), "/synthetic", root.path());
    let (first, second) = tokio::join!(
        store.download(synthetic_remote_file(), "download", "output"),
        store.download(synthetic_remote_file(), "download", "output")
    );
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    assert_eq!(
        std::fs::read(root.path().join("download/output")).unwrap(),
        b"abc"
    );
    assert_eq!(
        std::fs::read_dir(root.path().join("download"))
            .unwrap()
            .count(),
        1
    );
}

#[tokio::test]
async fn ssh_timeout_includes_nonreading_script_stdin_and_reaps_local_shim() {
    let fixture = Fixture::new();
    let pid_path = fixture.path().join("blocked-ssh.pid");
    std::fs::write(
        fixture.path().join("ssh"),
        format!(
            "#!/bin/sh\nprintf '%s' \"$$\" > '{}'\nexec python3 -c 'import time;time.sleep(10)'\n",
            pid_path.display()
        ),
    )
    .unwrap();
    let target = fixture
        .ssh_target()
        .with_command_timeout(Duration::from_millis(100));
    let script = "x".repeat(2 * 1024 * 1024);
    let result = tokio::time::timeout(Duration::from_secs(2), target.run_script(&script)).await;
    let error = result
        .expect("stdin write escaped command deadline")
        .unwrap_err();
    assert!(error.to_string().contains("remote outcome UNKNOWN"));
    let pid = std::fs::read_to_string(&pid_path).unwrap();
    for _ in 0..100 {
        if !Path::new("/proc").join(pid.trim()).exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("local synthetic SSH process was not reaped after timeout");
}

#[tokio::test]
async fn accepted_submission_timeout_recovers_without_replaying_sbatch() {
    let fixture = Fixture::new();
    let shim = FIXTURE_SSH.replace("exec /bin/sh -c \"$*\"", "if [ \"$*\" = \"bash -s\" ]; then /bin/sh -c \"$*\"; exec python3 -c 'import time;time.sleep(10)'; fi\nexec /bin/sh -c \"$*\"");
    std::fs::write(fixture.path().join("ssh"), shim).unwrap();
    let cluster = tempfile::tempdir().unwrap();
    let mut settings = slurm_settings(&fixture, cluster.path(), cluster.path());
    // This case needs the synthetic sbatch to ACCEPT before its SSH shim
    // withholds stdout for ten seconds. Allow fixture creation on a shared
    // filesystem to finish; the separate 100 ms stdin/reaping regression
    // above still exercises the short command deadline itself.
    settings.ssh = fixture
        .ssh_target()
        .with_command_timeout(Duration::from_secs(2));
    let transport = SlurmTaskTransport::new(settings);
    let lease = submit_simple(&transport, cluster.path(), "accepted-timeout").await;
    assert_eq!(lease.lease_id, "4242");
    assert_eq!(
        fixture
            .calls_log()
            .lines()
            .filter(|s| s.starts_with("--parsable "))
            .count(),
        1
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn cancelled_download_has_no_named_staging_to_remove_or_substitute() {
    let fixture = Fixture::new();
    let pid_path = fixture.path().join("stream-ssh.pid");
    std::fs::write(fixture.path().join("ssh"), format!(
        "#!/bin/sh\ncase \"$*\" in *'stat -c'*) printf '3\\nba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\\n';; *) printf '%s' \"$$\" > '{}'; exec python3 -c 'import sys,time;sys.stdout.write(\"a\");sys.stdout.flush();time.sleep(10)';; esac\n", pid_path.display()
    )).unwrap();
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("download");
    std::fs::create_dir(&directory).unwrap();
    let foreign = directory.join("output.part-foreign");
    std::fs::write(&foreign, "retain foreign body").unwrap();
    let store = SshArtifactStore::new(fixture.ssh_target(), "/synthetic", root.path());
    let result = tokio::time::timeout(
        Duration::from_millis(250),
        store.download(synthetic_remote_file(), "download", "output"),
    )
    .await;
    assert!(
        result.is_err(),
        "outer scope did not cancel an in-progress stream"
    );
    assert!(!directory.join("output").exists());
    assert_eq!(
        std::fs::read_to_string(&foreign).unwrap(),
        "retain foreign body"
    );
    assert_eq!(std::fs::read_dir(directory).unwrap().count(), 1);
    let pid = std::fs::read_to_string(pid_path).unwrap();
    for _ in 0..100 {
        if !Path::new("/proc").join(pid.trim()).exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("synthetic stream SSH process was not reaped after scope cancellation");
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "bounded subprocess helper, explicitly invoked by file_upload_special_sources_rejected_before_ssh"]
async fn fifo_file_admission_subprocess() {
    let root = PathBuf::from(std::env::var_os("AUTONOMICS_SLURM_FIFO_CHILD_ROOT").unwrap());
    std::fs::write(
        root.join("fifo-child-entered"),
        b"entered real upload caller",
    )
    .unwrap();
    let target = SshTarget::new(root.join("ssh").to_string_lossy(), "fakehost")
        .with_transfer_timeout(Duration::from_millis(100));
    let result = target
        .run_with_file_stdin("unused", &root.join("fifo"))
        .await;
    assert!(
        result.is_err(),
        "FIFO reached SSH instead of regular-file admission rejection"
    );
    assert!(!root.join("ssh-started").exists());
    let store = SshArtifactStore::new(target, "~/forbidden-remote-preparation", &root);
    let artifact = FileRef {
        domain: Default::default(),
        path: root.join("fifo").to_string_lossy().into_owned(),
        format: None,
        fingerprint: None,
    };
    assert!(store.upload(artifact, "invalid", "input").await.is_err());
    assert!(
        !root.join("ssh-started").exists(),
        "artifact upload started SSH for FIFO"
    );
    std::fs::write(
        root.join("fifo-child-admission-rejected"),
        b"rejected before SSH",
    )
    .unwrap();
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn file_upload_special_sources_rejected_before_ssh() {
    use std::os::unix::{fs::symlink, net::UnixListener};
    let fixture = Fixture::new();
    std::fs::write(fixture.path().join("ssh"), "#!/bin/sh\nDIR=$(CDPATH= cd -- \"$(dirname -- \"$0\")\" && pwd)\nprintf started > \"$DIR/ssh-started\"\nexit 0\n").unwrap();
    let root = fixture.path();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(root.join("fifo"))
            .status()
            .unwrap()
            .success()
    );
    // The OLD defect blocks the Tokio thread in File::open, so an async
    // timeout in that same process is insufficient. Isolate it in a child
    // harness and always kill+wait on the parent's independent real deadline.
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "fifo_file_admission_subprocess",
            "--nocapture",
        ])
        .env("AUTONOMICS_SLURM_FIFO_CHILD_ROOT", root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            let result = child.wait_with_output().unwrap();
            assert!(
                result.status.success(),
                "FIFO child failed: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            break;
        }
        if started.elapsed() > Duration::from_secs(3) {
            let kill_result = child.kill();
            let result = child.wait_with_output().unwrap();
            panic!(
                "FIFO admission blocked; isolated child kill={kill_result:?}, reaped: {}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        std::fs::read(root.join("fifo-child-entered")).unwrap(),
        b"entered real upload caller"
    );
    assert_eq!(
        std::fs::read(root.join("fifo-child-admission-rejected")).unwrap(),
        b"rejected before SSH"
    );
    // The managed TMPDIR can exceed AF_UNIX's short sockaddr path limit.
    // Bind through our held directory FD; the admitted source is root/socket.
    use std::os::fd::AsRawFd;
    let directory_fd = std::fs::File::open(root).unwrap();
    let _listener =
        UnixListener::bind(format!("/proc/self/fd/{}/socket", directory_fd.as_raw_fd())).unwrap();
    std::fs::create_dir(root.join("directory")).unwrap();
    std::fs::write(root.join("regular"), b"ordinary body").unwrap();
    symlink(root.join("regular"), root.join("symlink")).unwrap();
    let target = fixture.ssh_target();
    let store = SshArtifactStore::new(target.clone(), "~/forbidden-remote-preparation", root);
    for name in ["socket", "directory", "symlink", "missing"] {
        assert!(
            target
                .run_with_file_stdin("unused", &root.join(name))
                .await
                .is_err(),
            "accepted {name}"
        );
        let artifact = FileRef {
            domain: Default::default(),
            path: root.join(name).to_string_lossy().into_owned(),
            format: None,
            fingerprint: None,
        };
        assert!(
            store.upload(artifact, "invalid", "input").await.is_err(),
            "store accepted {name}"
        );
    }
    assert!(
        !root.join("ssh-started").exists(),
        "invalid source started SSH"
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn upload_streams_held_regular_inode_after_path_replacement() {
    let fixture = Fixture::new();
    let source = fixture.path().join("source");
    std::fs::write(&source, b"original held bytes").unwrap();
    std::fs::write(fixture.path().join("ssh"), "#!/bin/sh\nDIR=$(CDPATH= cd -- \"$(dirname -- \"$0\")\" && pwd)\nmv \"$DIR/source\" \"$DIR/moved-original\"\nprintf substituted > \"$DIR/source\"\nexec cat\n").unwrap();
    let result = fixture
        .ssh_target()
        .run_with_file_stdin("unused", &source)
        .await
        .unwrap();
    assert!(result.status.success());
    assert_eq!(result.stdout, b"original held bytes");
    assert_eq!(std::fs::read(&source).unwrap(), b"substituted");
    assert_eq!(
        std::fs::read(fixture.path().join("moved-original")).unwrap(),
        b"original held bytes"
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn artifact_upload_pins_source_before_first_remote_preparation() {
    let fixture = Fixture::new();
    let source = fixture.path().join("source");
    std::fs::write(&source, b"original store-held bytes").unwrap();
    let replace = "if [ ! -f \"$DIR/source-replaced\" ]; then mv \"$DIR/source\" \"$DIR/moved-original\"; printf substituted > \"$DIR/source\"; touch \"$DIR/source-replaced\"; fi\nexec /bin/sh -c \"$*\"";
    std::fs::write(
        fixture.path().join("ssh"),
        FIXTURE_SSH.replace("exec /bin/sh -c \"$*\"", replace),
    )
    .unwrap();
    let remote = fixture.path().join("remote");
    let store = SshArtifactStore::new(
        fixture.ssh_target(),
        remote.to_string_lossy(),
        fixture.path().join("local"),
    );
    let artifact = FileRef {
        domain: Default::default(),
        path: source.to_string_lossy().into_owned(),
        format: None,
        fingerprint: None,
    };
    let uploaded = store
        .upload(artifact, "held-source", "input")
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(&uploaded.path).unwrap(),
        b"original store-held bytes"
    );
    assert_eq!(std::fs::read(source).unwrap(), b"substituted");
    let fingerprint = uploaded.fingerprint.as_ref().unwrap();
    assert_eq!(fingerprint.size, b"original store-held bytes".len() as u64);
    assert_eq!(
        fingerprint.content_hash.as_deref(),
        Some(sha256_hex(b"original store-held bytes").as_str())
    );
    assert_eq!(
        std::fs::read(fixture.path().join("moved-original")).unwrap(),
        b"original store-held bytes"
    );
}

#[tokio::test]
async fn submit_adopts_job_after_lost_stdout() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    // Remove only the toy durable response to exercise name-based recovery.
    std::fs::write(
        fixture.path().join("discard_submission_records"),
        format!(
            "{}\n",
            task_dir(cluster.path(), "slurm-adopt-lost")
                .join("submission.job-id")
                .display()
        ),
    )
    .unwrap();
    // sbatch commits, then the connection dies before the job id comes
    // back: the one-shot flag runs the bootstrap but still exits 255.
    std::fs::write(fixture.path().join("ssh_fail_255_after_run_once"), "").unwrap();
    // The full queue listing shows exactly one job under the submitted name.
    std::fs::write(fixture.path().join("squeue_listing"), "4242 @LAST@\n").unwrap();

    let transport = transport_of(&fixture, cluster.path());
    let (dispatch, artifacts) = simple_dispatch(cluster.path(), "slurm-adopt-lost");
    let lease = transport.submit(dispatch, artifacts).await.unwrap();
    assert_eq!(lease.lease_id, "4242");
    assert_eq!(lease.task_id, "slurm-adopt-lost");

    // Adoption took over the already-submitted job: exactly one real
    // sbatch happened, and the lost attempt was not repeated.
    let sbatch_calls = fixture
        .calls_log()
        .lines()
        .filter(|line| line.starts_with("--parsable "))
        .count();
    assert_eq!(sbatch_calls, 1, "calls.log: {}", fixture.calls_log());

    // The adopted job behaves like any other lease afterwards.
    std::fs::write(fixture.path().join("sacct_out"), "4242|COMPLETED|0:0\n").unwrap();
    let receipt = transport.wait(&lease).await.unwrap();
    assert_eq!(receipt.status, "success");
    assert_eq!(receipt.exit_code, 0);
}

#[tokio::test]
async fn submit_adopts_via_sacct_name_when_squeue_is_empty() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    // Remove only the toy durable response to exercise name-based recovery.
    std::fs::write(
        fixture.path().join("discard_submission_records"),
        format!(
            "{}\n",
            task_dir(cluster.path(), "slurm-adopt-sacct")
                .join("submission.job-id")
                .display()
        ),
    )
    .unwrap();
    std::fs::write(fixture.path().join("ssh_fail_255_after_run_once"), "").unwrap();
    // The queue listing has already dropped the job, but accounting still
    // knows it by exact name.
    std::fs::write(fixture.path().join("sacct_name_out"), "4242|RUNNING\n").unwrap();

    let transport = transport_of(&fixture, cluster.path());
    let (dispatch, artifacts) = simple_dispatch(cluster.path(), "slurm-adopt-sacct");
    let lease = transport.submit(dispatch, artifacts).await.unwrap();
    assert_eq!(lease.lease_id, "4242");
    assert_eq!(lease.task_id, "slurm-adopt-sacct");
}

#[tokio::test]
async fn submission_authentication_failure_delivers_no_payload_or_intent() {
    fn fixture_tree(path: &Path) -> serde_json::Value {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::symlink_metadata(path).unwrap();
        assert!(metadata.is_dir() || metadata.is_file());
        let mut entries = std::collections::BTreeMap::new();
        if metadata.is_dir() {
            for entry in std::fs::read_dir(path).unwrap() {
                let entry = entry.unwrap();
                entries.insert(
                    entry.file_name().into_string().unwrap(),
                    fixture_tree(&entry.path()),
                );
            }
        }
        serde_json::json!({
            "device": metadata.dev(), "inode": metadata.ino(),
            "mode": metadata.mode(), "links": metadata.nlink(), "size": metadata.len(),
            "mtime": [metadata.mtime(), metadata.mtime_nsec()],
            "ctime": [metadata.ctime(), metadata.ctime_nsec()],
            "contents": if metadata.is_file() { std::fs::read(path).unwrap() } else { vec![] },
            "entries": entries,
        })
    }
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    std::fs::write(fixture.path().join("ssh_fail_255_before_once"), "").unwrap();
    let transport = transport_of(&fixture, cluster.path());
    let (dispatch, artifacts) = simple_dispatch(cluster.path(), "slurm-auth-denied");
    // The fixture deliberately pre-stages out-0.json here. Authentication
    // failure must preserve the entire original tree, not make it disappear.
    let before = fixture_tree(cluster.path());
    let error = transport.submit(dispatch, artifacts).await.unwrap_err();
    assert!(error.to_string().contains("payload NOT_SENT"), "{error}");
    assert!(
        error
            .to_string()
            .contains("synthetic SSH connection rejected")
    );
    assert!(matches!(
        &error,
        DagError::SubmissionNotSent {
            local_ssh_stopped: true,
            ..
        }
    ));
    assert_eq!(error.to_report().kind, "submission_not_sent");
    assert!(!fixture.calls_log().contains("--parsable"));
    assert_eq!(fixture_tree(cluster.path()), before);
    assert!(
        !task_dir(cluster.path(), "slurm-auth-denied")
            .join("submission.identity")
            .exists()
    );
}

#[tokio::test]
async fn submission_handshake_orders_durable_admission_before_payload() {
    let fixture = Fixture::new();
    let marker = fixture.path().join("durable-admission");
    let output = fixture
        .ssh_target()
        .run_submission_once(
            &format!("test -f '{}' && printf submitted\n", marker.display()),
            || {
                std::fs::write(&marker, "admitted").unwrap();
                Ok(())
            },
        )
        .await
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"submitted");
}

#[tokio::test]
async fn lost_submission_reply_with_empty_listings_retains_identity_without_replay() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let task = "slurm-lost-reply";
    std::fs::write(
        fixture.path().join("discard_submission_records"),
        format!(
            "{}\n",
            task_dir(cluster.path(), task)
                .join("submission.job-id")
                .display(),
        ),
    )
    .unwrap();
    std::fs::write(fixture.path().join("ssh_fail_255_after_run_once"), "").unwrap();
    let transport = transport_of(&fixture, cluster.path());
    let (dispatch, artifacts) = simple_dispatch(cluster.path(), task);
    let error = transport.submit(dispatch, artifacts).await.unwrap_err();
    assert!(!matches!(&error, DagError::SubmissionNotSent { .. }));
    assert!(
        error
            .to_string()
            .contains("empty listings do not authorize replay"),
        "{error}"
    );
    let calls = fixture.calls_log();
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.starts_with("--parsable "))
            .count(),
        1
    );
    assert!(
        calls
            .lines()
            .any(|line| line.contains("-u ") && line.contains("-o JobID"))
    );
    assert!(!calls.contains("--me"));
    let intent_dir = cluster.path().join(task);
    let intents: Vec<_> = std::fs::read_dir(&intent_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".intent-")
        })
        .collect();
    assert_eq!(intents.len(), 1);
    let intent: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&intents[0]).unwrap()).unwrap();
    let diagnostic = intent_dir.join(format!(
        ".submission-response-{}.json",
        intent["nonce"].as_str().unwrap()
    ));
    let response: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&diagnostic).unwrap()).unwrap();
    for field in ["nonce", "submission_identity", "manifest_sha256"] {
        assert_eq!(response[field], intent[field]);
    }
    assert_eq!(response["outcome"]["exit_code"], 255);
    assert_eq!(response["resubmission_authorized"], false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&diagnostic).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    for field in [
        "attempted_at_unix_ms",
        "dispatch_sha256",
        "job_name",
        "transport_binding",
    ] {
        assert!(intent.get(field).is_some());
    }
}

#[tokio::test]
async fn submission_handshake_refuses_payload_when_admission_fails() {
    let fixture = Fixture::new();
    let marker = fixture.path().join("payload-must-not-run");
    let error = fixture
        .ssh_target()
        .run_submission_once(&format!("touch '{}'\n", marker.display()), || {
            Err(DagError::Schedule("synthetic admission rejected".into()))
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("payload NOT_SENT"));
    assert!(!marker.exists());
}

#[tokio::test]
async fn submission_handshake_rejects_stdout_pollution_without_admission() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.path().join("ssh"),
        "#!/bin/sh\nprintf 'unexpected server banner\\n'; exit 255\n",
    )
    .unwrap();
    let mut admitted = false;
    let error = fixture
        .ssh_target()
        .run_submission_once("echo forbidden\n", || {
            admitted = true;
            Ok(())
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("payload NOT_SENT"));
    assert!(!admitted);
}

#[tokio::test]
async fn submission_handshake_deadline_reaps_child_without_admitting_payload() {
    let fixture = Fixture::new();
    let pid_path = fixture.path().join("blocked-ssh.pid");
    std::fs::write(
        fixture.path().join("ssh"),
        format!(
            "#!/bin/sh\nprintf '%s' \"$$\" > '{}'\nexec python3 -c 'import time;time.sleep(10)'\n",
            pid_path.display(),
        ),
    )
    .unwrap();
    let target = fixture
        .ssh_target()
        .with_command_timeout(Duration::from_millis(100));
    let mut admitted = false;
    let error = target
        .run_submission_once("echo forbidden\n", || {
            admitted = true;
            Ok(())
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("payload NOT_SENT"));
    assert!(!admitted);
    let pid = std::fs::read_to_string(pid_path).unwrap();
    assert!(!Path::new("/proc").join(pid.trim()).exists());
}

#[tokio::test]
async fn submit_reports_unknown_when_accounting_cannot_confirm() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    // Remove only the toy durable response to exercise name-based recovery.
    std::fs::write(
        fixture.path().join("discard_submission_records"),
        format!(
            "{}\n",
            task_dir(cluster.path(), "slurm-unknown")
                .join("submission.job-id")
                .display()
        ),
    )
    .unwrap();
    // sbatch committed, but accounting is up with zero records: the empty
    // canary means nothing can confirm or deny the submit, so the outcome
    // must stay unknown instead of being guessed.
    std::fs::write(fixture.path().join("ssh_fail_255_after_run_once"), "").unwrap();
    std::fs::write(fixture.path().join("sacct_canary_out"), "").unwrap();

    let transport = transport_of(&fixture, cluster.path());
    let (dispatch, artifacts) = simple_dispatch(cluster.path(), "slurm-unknown");
    let error = transport.submit(dispatch, artifacts).await.unwrap_err();
    assert!(
        error.to_string().contains("submission status unknown"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn submit_cancels_ambiguous_candidates_before_erroring() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    // Remove only the toy durable response to exercise name-based recovery.
    std::fs::write(
        fixture.path().join("discard_submission_records"),
        format!(
            "{}\n",
            task_dir(cluster.path(), "slurm-ambiguous")
                .join("submission.job-id")
                .display()
        ),
    )
    .unwrap();
    // Two queue entries carry the submitted name: adopting either would be
    // a coin flip, so both candidates get a best-effort scancel before the
    // transport reports the outcome as unknown.
    std::fs::write(fixture.path().join("ssh_fail_255_after_run_once"), "").unwrap();
    std::fs::write(
        fixture.path().join("squeue_listing"),
        "4242 @LAST@\n4243 @LAST@\n",
    )
    .unwrap();

    let transport = transport_of(&fixture, cluster.path());
    let (dispatch, artifacts) = simple_dispatch(cluster.path(), "slurm-ambiguous");
    let error = transport.submit(dispatch, artifacts).await.unwrap_err();
    assert!(
        error.to_string().contains("submission status unknown"),
        "unexpected error: {error}"
    );

    // Every candidate was cancelled by its bare numeric job id (shim call
    // logs carry only the arguments, no argv[0]).
    let calls = fixture.calls_log();
    assert!(
        calls.lines().any(|line| line == "4242"),
        "calls.log: {calls}"
    );
    assert!(
        calls.lines().any(|line| line == "4243"),
        "calls.log: {calls}"
    );
    // Ambiguity is reported, never retried.
    let sbatch_calls = calls
        .lines()
        .filter(|line| line.starts_with("--parsable "))
        .count();
    assert_eq!(sbatch_calls, 1, "calls.log: {calls}");
}

#[tokio::test]
async fn cancel_confirmed_by_sacct_terminal_state() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let transport = transport_of(&fixture, cluster.path());
    let lease = submit_simple(&transport, cluster.path(), "slurm-cancel-sacct").await;

    // Accounting already shows a terminal state: the first poll confirms
    // the stop without needing the queue to drain.
    std::fs::write(
        fixture.path().join("sacct_out"),
        "4242|CANCELLED by 12345|0:15\n",
    )
    .unwrap();
    transport.cancel(&lease).await.unwrap();
    assert!(fixture.path().join("cancelled").is_file());
}

#[tokio::test]
async fn cancel_ok_when_scancel_fails_but_job_already_terminal() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let transport = transport_of(&fixture, cluster.path());
    let lease = submit_simple(&transport, cluster.path(), "slurm-cancel-late").await;

    // The job already finished, so scancel exits nonzero — the terminal
    // accounting record still proves the stop happened.
    std::fs::write(fixture.path().join("scancel_fail"), "").unwrap();
    std::fs::write(fixture.path().join("sacct_out"), "4242|COMPLETED|0:0\n").unwrap();
    transport.cancel(&lease).await.unwrap();
}

#[tokio::test]
async fn cancel_fails_when_stop_cannot_be_confirmed() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let transport = transport_of(&fixture, cluster.path());
    let lease = submit_simple(&transport, cluster.path(), "slurm-cancel-stuck").await;

    // Nothing ever turns terminal: accounting says RUNNING and the job
    // never leaves the queue, so the confirmation budget must run out.
    std::fs::write(fixture.path().join("scancel_fail"), "").unwrap();
    std::fs::write(fixture.path().join("sacct_out"), "4242|RUNNING|0:0\n").unwrap();
    std::fs::write(fixture.path().join("squeue_job_state"), "RUNNING\n").unwrap();
    let error = transport.cancel(&lease).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("cancel requested but stop unconfirmed"),
        "unexpected error: {error}"
    );

    // Unknown cancellation retains its lease for exact later reconciliation.
    std::fs::remove_file(fixture.path().join("squeue_job_state")).unwrap();
    std::fs::write(fixture.path().join("sacct_out"), "4242|FAILED|2:0\n").unwrap();
    assert_eq!(transport.wait(&lease).await.unwrap().status, "failed");
}

#[tokio::test]
async fn wait_rejects_malformed_exit_code() {
    // `rc:signal` must be all-digit on both sides of the colon.
    let error = wait_with_sacct("4242|COMPLETED|garbage", "slurm-garbage")
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("invalid or nonterminal sacct record"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn wait_refuses_completed_with_contradictory_exit() {
    // COMPLETED only counts with a clean `0:0`; anything else falls back to
    // the recorded exit: rc itself, or 128+signal when a signal killed it.
    for (record, expected_exit) in [("4242|COMPLETED|1:0", 1), ("4242|COMPLETED|0:9", 137)] {
        let receipt = wait_with_sacct(record, "slurm-contradictory")
            .await
            .unwrap();
        assert_eq!(receipt.status, "failed", "record {record}");
        assert_eq!(receipt.exit_code, expected_exit, "record {record}");
    }
}

#[tokio::test]
async fn wait_success_requires_every_record_to_be_clean() {
    for records in [
        "4242|COMPLETED|0:0\n4242|FAILED|3:0\n",
        "4242|COMPLETED|0:0\n4242|COMPLETED|0:0\n",
    ] {
        assert!(wait_with_sacct(records, "slurm-multi").await.is_err());
    }
}

#[tokio::test]
async fn ssh_commands_and_streams_disable_persistent_multiplexing() {
    let fixture = Fixture::new();
    let ssh = fixture.ssh_target();
    let command = ssh.run("printf command-ok").await.unwrap();
    assert!(command.status.success());
    assert_eq!(command.stdout, b"command-ok");

    let staging = fixture.path().join("single-call-stream.bin");
    let stream = ssh
        .stream_remote_to_file("printf '3\\nabc'", &staging)
        .await
        .unwrap();
    assert_eq!(stream.content_bytes, 3);
    assert_eq!(std::fs::read(&staging).unwrap(), b"abc");
    let options = std::fs::read_to_string(fixture.path().join("ssh_options.log")).unwrap();
    for required in [
        "BatchMode=yes",
        "StrictHostKeyChecking=yes",
        "ControlMaster=no",
        "ControlPath=none",
        "ControlPersist=no",
    ] {
        assert_eq!(
            options.lines().filter(|option| *option == required).count(),
            2,
            "both execute and stream paths must pass {required}"
        );
    }

    // The artifact store has a separate anonymous-inode download path. Its
    // metadata query and payload transfer must both override inherited SSH
    // multiplexing too; the generic stream test above does not exercise it.
    let cluster = tempfile::tempdir().unwrap();
    let local_root = tempfile::tempdir().unwrap();
    let source = cluster.path().join("source.txt");
    std::fs::write(&source, b"artifact download policy").unwrap();
    let store = SshArtifactStore::new(
        ssh,
        cluster.path().to_string_lossy().into_owned(),
        local_root.path(),
    );
    let uploaded = store
        .upload(
            FileRef::local(&source, None).unwrap(),
            "ssh-policy",
            "artifact.txt",
        )
        .await
        .unwrap();
    std::fs::write(fixture.path().join("ssh_options.log"), b"").unwrap();
    let downloaded = store
        .download(uploaded, "ssh-policy", "artifact.txt")
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(&downloaded.path).unwrap(),
        b"artifact download policy"
    );
    let options = std::fs::read_to_string(fixture.path().join("ssh_options.log")).unwrap();
    for required in [
        "BatchMode=yes",
        "StrictHostKeyChecking=yes",
        "ControlMaster=no",
        "ControlPath=none",
        "ControlPersist=no",
    ] {
        assert_eq!(
            options.lines().filter(|option| *option == required).count(),
            2,
            "artifact metadata and payload paths must both pass {required}"
        );
    }
    assert!(
        !fixture
            .path()
            .join("ssh_background_master_would_survive")
            .exists()
    );
}

#[tokio::test]
async fn ssh_stream_enforces_declared_size() {
    let fixture = Fixture::new();
    let ssh = fixture.ssh_target();

    let staging = fixture.path().join("stream-ok.bin");
    let stream = ssh
        .stream_remote_to_file("echo 3; printf abc", &staging)
        .await
        .unwrap();
    assert_eq!(stream.declared_size, 3);
    assert_eq!(stream.content_bytes, 3);
    assert_eq!(
        stream.content_sha256,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(std::fs::read(&staging).unwrap(), &b"abc"[..]);

    // A short stream, a missing header, and a remote failure after the
    // bytes must all error — and never leave staging behind.
    for (name, command) in [
        ("size-mismatch", "echo 5; printf abc"),
        ("no-header", "printf abc"),
        ("remote-exit", "echo 5; printf abcde; exit 3"),
    ] {
        let staging = fixture.path().join(format!("stream-{name}.bin"));
        let result = ssh.stream_remote_to_file(command, &staging).await;
        assert!(result.is_err(), "{name}: expected an error");
        assert!(
            !staging.exists(),
            "{name}: failed stream left staging behind"
        );
    }

    // Exit 255 with nothing received is the one retried failure: the flag
    // self-destructs, so the second attempt goes through cleanly.
    std::fs::write(fixture.path().join("ssh_fail_255_before_once"), "").unwrap();
    let staging = fixture.path().join("stream-retry.bin");
    let stream = ssh
        .stream_remote_to_file("echo 3; printf abc", &staging)
        .await
        .unwrap();
    assert_eq!(stream.content_bytes, 3);
}

#[tokio::test]
async fn download_attaches_fingerprint_and_leaves_no_part_files() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let local_root = tempfile::tempdir().unwrap();
    let store = SshArtifactStore::new(
        fixture.ssh_target(),
        cluster.path().to_string_lossy().into_owned(),
        local_root.path(),
    );

    let source = cluster.path().join("payload.txt");
    std::fs::write(&source, "fingerprint me").unwrap();
    let uploaded = store
        .upload(
            FileRef::local(&source, Some("text".into())).unwrap(),
            "dl-fingerprint",
            "artifact.bin",
        )
        .await
        .unwrap();

    // The upload carries no content hash, so download computes and attaches
    // one from the bytes it actually received.
    let downloaded = store
        .download(uploaded, "dl-fingerprint", "artifact.bin")
        .await
        .unwrap();
    let fingerprint = downloaded
        .fingerprint
        .as_ref()
        .expect("downloaded artifact carries a fingerprint");
    assert_eq!(fingerprint.size, "fingerprint me".len() as u64);
    assert_eq!(
        fingerprint.content_hash.as_deref(),
        Some(sha256_hex(b"fingerprint me").as_str())
    );
    assert_eq!(
        std::fs::read_to_string(&downloaded.path).unwrap(),
        "fingerprint me"
    );

    // Streaming writes through a `.part` file: none may survive a success.
    let leftovers: Vec<String> = std::fs::read_dir(local_root.path().join("dl-fingerprint"))
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        leftovers.iter().all(|name| !name.contains(".part")),
        "part files left behind: {leftovers:?}"
    );
}

#[tokio::test]
async fn download_rejects_declared_fingerprint_mismatch() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let local_root = tempfile::tempdir().unwrap();
    let store = SshArtifactStore::new(
        fixture.ssh_target(),
        cluster.path().to_string_lossy().into_owned(),
        local_root.path(),
    );

    let payload = b"declared content".to_vec();
    let source = cluster.path().join("payload.txt");
    std::fs::write(&source, &payload).unwrap();
    let remote_path = store
        .upload(
            FileRef::local(&source, None).unwrap(),
            "dl-mismatch",
            "artifact.bin",
        )
        .await
        .unwrap()
        .path;

    // A receipt-borne fingerprint that disagrees with the actual bytes must
    // abort the download instead of trusting the declaration.
    let mismatching = FileRef {
        domain: Default::default(),
        path: remote_path.clone(),
        format: None,
        fingerprint: Some(FileFingerprint {
            size: 999,
            mtime_ns: 0,
            content_hash: Some("deadbeef".into()),
            immutable_remote: true,
        }),
    };
    let error = store
        .download(mismatching, "dl-mismatch", "artifact.bin")
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("artifact fingerprint mismatch"),
        "unexpected error: {error}"
    );
    let destination_dir = local_root.path().join("dl-mismatch");
    assert!(!destination_dir.join("artifact.bin").exists());
    let leftovers: Vec<String> = std::fs::read_dir(&destination_dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        leftovers.iter().all(|name| !name.contains(".part")),
        "part files left behind: {leftovers:?}"
    );

    // The matching fingerprint downloads cleanly and is echoed back as-is.
    let verified = FileFingerprint {
        size: payload.len() as u64,
        mtime_ns: 0,
        content_hash: Some(sha256_hex(&payload)),
        immutable_remote: true,
    };
    let downloaded = store
        .download(
            FileRef {
                domain: Default::default(),
                path: remote_path,
                format: None,
                fingerprint: Some(verified.clone()),
            },
            "dl-matched",
            "artifact.bin",
        )
        .await
        .unwrap();
    assert_eq!(downloaded.fingerprint, Some(verified));
    assert_eq!(std::fs::read(&downloaded.path).unwrap(), payload);
}

#[tokio::test]
async fn download_streams_multi_megabyte_payload() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let local_root = tempfile::tempdir().unwrap();
    let store = SshArtifactStore::new(
        fixture.ssh_target(),
        cluster.path().to_string_lossy().into_owned(),
        local_root.path(),
    );

    // 3 MiB of deterministic bytes: an alphabet-rich 64 KiB pattern per
    // block, shifted per block so the stream is not one repeated line a
    // lazy copy could fake.
    const PATTERN_LEN: usize = 64 * 1024;
    let pattern: Vec<u8> = (0..PATTERN_LEN).map(|index| (index % 251) as u8).collect();
    let mut payload = Vec::with_capacity(3 * 1024 * 1024);
    for block in 0..(3 * 1024 * 1024 / PATTERN_LEN) {
        let offset = (block % 251) as u8;
        payload.extend(pattern.iter().map(|byte| byte.wrapping_add(offset)));
    }
    assert_eq!(payload.len(), 3 * 1024 * 1024);

    let source = cluster.path().join("payload.bin");
    std::fs::write(&source, &payload).unwrap();
    let uploaded = store
        .upload(
            FileRef::local(&source, None).unwrap(),
            "dl-bulk",
            "payload.bin",
        )
        .await
        .unwrap();
    let downloaded = store
        .download(uploaded, "dl-bulk", "payload.bin")
        .await
        .unwrap();

    assert_eq!(std::fs::read(&downloaded.path).unwrap(), payload);
    let fingerprint = downloaded
        .fingerprint
        .as_ref()
        .expect("bulk download carries a fingerprint");
    assert_eq!(fingerprint.size, payload.len() as u64);
    assert_eq!(
        fingerprint.content_hash.as_deref(),
        Some(sha256_hex(&payload).as_str())
    );
}

async fn wait_with_sacct(sacct_out: &str, task_id: &str) -> Result<TaskAttemptReceipt, DagError> {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    std::fs::write(fixture.path().join("sacct_out"), sacct_out).unwrap();
    let transport = transport_of(&fixture, cluster.path());
    let lease = submit_simple(&transport, cluster.path(), task_id).await;
    transport.wait(&lease).await
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn simple_dispatch(cluster: &Path, task_id: &str) -> (TaskDispatch, Vec<FileRef>) {
    prepare_simple(cluster, task_id)
}

#[tokio::test]
async fn artifact_upload_recovers_absence_or_complete_readback_without_a_job_retry() {
    for (flag, expected_writes) in [
        ("upload_auth_failure_once", 2),
        ("upload_lost_reply_once", 1),
    ] {
        let fixture = Fixture::new();
        let cluster = tempfile::tempdir().unwrap();
        let local = tempfile::tempdir().unwrap();
        let store = SshArtifactStore::new(
            fixture.ssh_target(),
            cluster.path().to_string_lossy(),
            local.path(),
        );
        let source = fixture.path().join("source");
        std::fs::write(&source, b"authenticated original bytes").unwrap();
        std::fs::write(fixture.path().join(flag), b"").unwrap();
        let file = store
            .upload(
                FileRef::local(&source, None).unwrap(),
                "safe-upload",
                "input",
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(&file.path).unwrap(),
            b"authenticated original bytes"
        );
        assert_eq!(
            fixture
                .ssh_calls_log()
                .lines()
                .filter(|s| s.contains("cat > "))
                .count(),
            expected_writes
        );
        assert!(!fixture.path().join("last_job_name").exists());
    }
}

#[tokio::test]
async fn artifact_upload_partial_target_is_retained_and_never_overwritten() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    let store = SshArtifactStore::new(
        fixture.ssh_target(),
        cluster.path().to_string_lossy(),
        local.path(),
    );
    let source = fixture.path().join("source");
    std::fs::write(&source, b"abcde").unwrap();
    std::fs::write(fixture.path().join("upload_partial_once"), b"").unwrap();
    let error = store
        .upload(FileRef::local(&source, None).unwrap(), "partial", "input")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("partial or foreign"));
    assert_eq!(
        std::fs::read(cluster.path().join("partial/input")).unwrap(),
        b"ab"
    );
    assert_eq!(std::fs::read(source).unwrap(), b"abcde");
    assert_eq!(
        fixture
            .ssh_calls_log()
            .lines()
            .filter(|s| s.contains("cat > "))
            .count(),
        1
    );
}

#[tokio::test]
async fn artifact_upload_uncertain_reply_never_adopts_a_foreign_symlink() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    let store = SshArtifactStore::new(
        fixture.ssh_target(),
        cluster.path().to_string_lossy(),
        local.path(),
    );
    let source = fixture.path().join("source");
    std::fs::write(&source, b"same bytes").unwrap();
    std::fs::create_dir(cluster.path().join("foreign")).unwrap();
    symlink(&source, cluster.path().join("foreign/input")).unwrap();
    std::fs::write(fixture.path().join("upload_auth_failure_once"), b"").unwrap();
    let error = store
        .upload(FileRef::local(&source, None).unwrap(), "foreign", "input")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("partial or foreign"));
    assert!(cluster.path().join("foreign/input").is_symlink());
    assert_eq!(std::fs::read(source).unwrap(), b"same bytes");
}

#[tokio::test]
async fn artifact_download_restarts_a_partial_ssh_stream_and_verifies_the_entire_payload() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    let store = SshArtifactStore::new(
        fixture.ssh_target(),
        cluster.path().to_string_lossy(),
        local.path(),
    );
    let source = cluster.path().join("source");
    std::fs::write(&source, b"whole authenticated payload").unwrap();
    std::fs::write(fixture.path().join("download_partial_transport_once"), b"").unwrap();
    let file = store
        .download(FileRef::local(&source, None).unwrap(), "download", "output")
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(file.path).unwrap(),
        b"whole authenticated payload"
    );
    assert_eq!(
        fixture
            .ssh_calls_log()
            .lines()
            .filter(|s| s.starts_with("cat -- "))
            .count(),
        2
    );
    assert_eq!(
        std::fs::read_dir(local.path().join("download"))
            .unwrap()
            .count(),
        1
    );
}

#[tokio::test]
async fn artifact_download_does_not_retry_a_non_transport_failure() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    let store = SshArtifactStore::new(
        fixture.ssh_target(),
        cluster.path().to_string_lossy(),
        local.path(),
    );
    let source = cluster.path().join("source");
    std::fs::write(&source, b"whole authenticated payload").unwrap();
    std::fs::write(
        fixture.path().join("download_non_transport_failure_once"),
        b"",
    )
    .unwrap();
    let error = store
        .download(FileRef::local(&source, None).unwrap(), "download", "output")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("exit 42"));
    assert!(!local.path().join("download/output").exists());
    assert_eq!(
        fixture
            .ssh_calls_log()
            .lines()
            .filter(|s| s.starts_with("cat -- "))
            .count(),
        1
    );
}

// The actual submit/wait/cancel transport path, with only SSH/Slurm faked.
#[derive(Debug, Default)]
struct RecorderState {
    begun: std::sync::Mutex<Vec<dag_core::SlurmSubmissionIntent>>,
    not_ready: std::sync::atomic::AtomicBool,
    rejected: std::sync::atomic::AtomicBool,
    reject_stop: std::sync::atomic::AtomicBool,
    stops: std::sync::atomic::AtomicUsize,
    unresolved: std::sync::atomic::AtomicUsize,
}
#[derive(Debug)]
struct RecorderSpy {
    state: Arc<RecorderState>,
    root: PathBuf,
}
#[derive(Debug)]
struct RecordSpy(Arc<RecorderState>);
impl dag_core::SlurmSubmissionRecorder for RecorderSpy {
    fn ensure_ready(&self) -> Result<(), DagError> {
        if self
            .state
            .not_ready
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            Err(DagError::Schedule(
                "SYNTHETIC running context is unbound".into(),
            ))
        } else {
            Ok(())
        }
    }

    fn before_submit(
        &self,
        intent: &dag_core::SlurmSubmissionIntent,
    ) -> Result<Arc<dyn dag_core::SlurmRecordedSubmission>, DagError> {
        let file = self
            .root
            .join(&intent.task_id)
            .join(format!(".intent-{}.json", intent.nonce));
        let saved = dag_core::SlurmSubmissionIntent::read(&file)?;
        assert_eq!(saved.submission_identity, intent.submission_identity);
        self.state.begun.lock().unwrap().push(intent.clone());
        if self
            .state
            .rejected
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(DagError::Schedule("SYNTHETIC ledger failure".into()));
        }
        Ok(Arc::new(RecordSpy(Arc::clone(&self.state))))
    }
}
impl dag_core::SlurmRecordedSubmission for RecordSpy {
    fn stopped(&self, raw: &serde_json::Value) -> Result<(), DagError> {
        assert_eq!(raw["schema"], "SLURM_OWNED_STOP_PROOF_V1");
        assert_eq!(raw["job_id"], "4242");
        assert!(
            raw["accounting"]["stdout"]
                .as_str()
                .unwrap()
                .contains("4242.extern|")
        );
        self.0
            .stops
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.0.reject_stop.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(DagError::Schedule("SYNTHETIC stop ledger failure".into()));
        }
        Ok(())
    }
    fn unresolved(&self) {
        self.0
            .unresolved
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}
fn recorded_transport(
    fixture: &Fixture,
    cluster: &Path,
) -> (SlurmTaskTransport, Arc<RecorderState>) {
    let state = Arc::new(RecorderState::default());
    let settings = slurm_settings(fixture, cluster, cluster);
    let transport =
        SlurmTaskTransport::new(settings).with_submission_recorder(Arc::new(RecorderSpy {
            state: Arc::clone(&state),
            root: cluster.to_owned(),
        }));
    (transport, state)
}
fn record_terminal_fixture(fixture: &Fixture, state: &RecorderState) {
    let intents = state.begun.lock().unwrap();
    let intent = intents.last().unwrap();
    let user = std::process::Command::new("id")
        .arg("-un")
        .output()
        .unwrap();
    assert!(user.status.success());
    let user = String::from_utf8(user.stdout).unwrap();
    let user = user.trim();
    let dir = &intent.remote_task_dir;
    let name = &intent.job_name;
    std::fs::write(
        fixture.path().join("sacct_ownership_out"),
        format!("4242|{user}||{dir}|{name}\n"),
    )
    .unwrap();
    std::fs::write(fixture.path().join("sacct_full_stop_out"), format!("4242|COMPLETED|0:0|2026-10-27T00:00:00|{user}||{dir}|{name}\n4242.batch|COMPLETED|0:0|2026-10-27T00:00:00||||\n4242.extern|COMPLETED|0:0|2026-10-27T00:00:00||||\n")).unwrap();
    std::fs::write(fixture.path().join("sacct_out"), "4242|COMPLETED|0:0\n").unwrap();
    let control = fixture.path().join("scontrol");
    std::fs::write(
        &control,
        "#!/bin/sh\nprintf 'slurm_load_jobs error: Invalid job id specified\\n' >&2\nexit 1\n",
    )
    .unwrap();
    make_executable(&control);
}
#[tokio::test]
async fn recorder_readiness_rejection_precedes_submission_ssh_and_intent_registration() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let (transport, state) = recorded_transport(&fixture, cluster.path());
    state
        .not_ready
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let (dispatch, artifacts) = prepare_simple(cluster.path(), "not-ready");
    let error = transport.submit(dispatch, artifacts).await.unwrap_err();
    assert!(error.to_string().contains("context is unbound"));
    assert!(state.begun.lock().unwrap().is_empty());
    assert!(fixture.calls_log().is_empty());
    assert!(fixture.ssh_calls_log().is_empty());
    assert!(!cluster.path().join("not-ready/submission.intent").exists());
    // The full coordinator also has to refuse *before* its artifact store
    // can resolve/download/upload inputs, not just before transport.submit.
    let input = cluster.path().join("input.txt");
    std::fs::write(&input, b"SYNTHETIC INPUT MUST NOT BE UPLOADED").unwrap();
    let coordinator = cluster.path().join("coordinator");
    let settings = slurm_settings(&fixture, cluster.path(), cluster.path());
    let store = SshArtifactStore::new(
        settings.ssh,
        settings.scratch_dir,
        cluster.path().join("downloads"),
    );
    let executor =
        RemoteTaskExecutor::new(Arc::new(transport), Arc::new(store), coordinator.clone());
    let result = executor
        .run(execution(process_submission(
            "not-ready-executor",
            &input,
            &echo_process(),
            TaskResources::default(),
        )))
        .await;
    assert!(
        matches!(result, JobResult::Failed { error, .. } if error.to_string().contains("context is unbound"))
    );
    assert!(!coordinator.exists());
    assert!(fixture.ssh_calls_log().is_empty());
}

#[tokio::test]
async fn formal_recorder_rejection_prevents_payload_after_authenticated_handshake() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let (transport, state) = recorded_transport(&fixture, cluster.path());
    state
        .rejected
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let (dispatch, artifacts) = prepare_simple(cluster.path(), "record-reject");
    let error = transport.submit(dispatch, artifacts).await.unwrap_err();
    assert!(error.to_string().contains("ledger failure"));
    assert_eq!(state.begun.lock().unwrap().len(), 1);
    assert!(fixture.calls_log().is_empty());
    assert_eq!(fixture.ssh_calls_log(), "bash -s\n");
    assert!(error.to_string().contains("payload NOT_SENT"));
    assert!(
        !cluster
            .path()
            .join("record-reject/submission.intent")
            .exists()
    );
}
#[tokio::test]
async fn formal_recorder_unconfirmed_stop_keeps_lease_and_does_not_collect_outputs() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let (transport, state) = recorded_transport(&fixture, cluster.path());
    let lease = submit_simple(&transport, cluster.path(), "record-wait").await;
    record_terminal_fixture(&fixture, &state);
    state
        .reject_stop
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(
        transport
            .wait(&lease)
            .await
            .unwrap_err()
            .to_string()
            .contains("stop ledger failure")
    );
    assert!(!cluster.path().join("record-wait/output-0-0").exists());
    assert_eq!(
        state.unresolved.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    state
        .reject_stop
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let receipt = transport.wait(&lease).await.unwrap();
    assert_eq!(receipt.exit_code, 0);
    assert_eq!(state.stops.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(
        state.unresolved.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
}
#[tokio::test]
async fn formal_recorder_cancellation_requires_owned_job_then_records_full_stop() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    let (transport, state) = recorded_transport(&fixture, cluster.path());
    let lease = submit_simple(&transport, cluster.path(), "record-cancel").await;
    assert!(
        transport
            .cancel(&lease)
            .await
            .unwrap_err()
            .to_string()
            .contains("ownership remains unknown")
    );
    assert!(!fixture.path().join("cancelled").exists());
    record_terminal_fixture(&fixture, &state);
    transport.cancel(&lease).await.unwrap();
    assert!(fixture.path().join("cancelled").exists());
    assert_eq!(state.stops.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn formal_recorder_ambiguous_submission_does_not_cancel_by_name_alone() {
    let fixture = Fixture::new();
    let cluster = tempfile::tempdir().unwrap();
    std::fs::write(
        fixture.path().join("discard_submission_records"),
        format!(
            "{}\n",
            task_dir(cluster.path(), "formal-ambiguous")
                .join("submission.job-id")
                .display()
        ),
    )
    .unwrap();
    std::fs::write(fixture.path().join("ssh_fail_255_after_run_once"), "").unwrap();
    std::fs::write(
        fixture.path().join("squeue_listing"),
        "4242 @LAST@\n4243 @LAST@\n",
    )
    .unwrap();
    let (transport, state) = recorded_transport(&fixture, cluster.path());
    let (dispatch, artifacts) = simple_dispatch(cluster.path(), "formal-ambiguous");
    assert!(
        transport
            .submit(dispatch, artifacts)
            .await
            .unwrap_err()
            .to_string()
            .contains("no cancellation attempted")
    );
    assert!(!fixture.path().join("cancelled").exists());
    assert_eq!(
        state.unresolved.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert_eq!(
        fixture
            .calls_log()
            .lines()
            .filter(|s| s.starts_with("--parsable "))
            .count(),
        1
    );
}

fn private_profile_file(path: &Path, raw: &[u8]) -> FileRef {
    use sha2::Digest;
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, raw).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    FileRef {
        domain: Default::default(),
        path: path.to_string_lossy().into_owned(),
        format: None,
        fingerprint: Some(FileFingerprint {
            size: raw.len() as u64,
            mtime_ns: 0,
            content_hash: Some(format!("{:x}", sha2::Sha256::digest(raw))),
            immutable_remote: false,
        }),
    }
}
#[tokio::test]
async fn pinned_ssh_profile_is_explicit_and_dependency_drift_prevents_any_command() {
    let fixture = Fixture::new();
    let config = private_profile_file(&fixture.path().join("config"), b"Host fakehost\n");
    let hosts = private_profile_file(&fixture.path().join("known_hosts"), b"SYNTHETIC HOST KEY\n");
    let ssh = fixture
        .ssh_target()
        .with_pinned_configuration(config.clone(), vec![hosts.clone()])
        .unwrap();
    assert!(ssh.run("true").await.unwrap().status.success());
    assert_eq!(
        std::fs::read_to_string(fixture.path().join("pinned_profile_calls.log"))
            .unwrap()
            .trim(),
        config.path
    );
    let calls = fixture.ssh_calls_log();
    std::fs::write(&hosts.path, b"CHANGED HOST KEY\n").unwrap();
    assert!(ssh.run("true").await.is_err());
    assert_eq!(fixture.ssh_calls_log(), calls);
    assert!(
        ssh.stream_remote_to_file("true", &fixture.path().join("download.part"))
            .await
            .is_err()
    );
    assert!(!fixture.path().join("download.part").exists());
    assert_eq!(fixture.ssh_calls_log(), calls);
}
#[test]
fn pinned_ssh_profile_rejects_world_readable_and_symlink_sources() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let fixture = Fixture::new();
    let config = private_profile_file(&fixture.path().join("config"), b"Host fakehost\n");
    std::fs::set_permissions(&config.path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        fixture
            .ssh_target()
            .with_pinned_configuration(config.clone(), vec![])
            .is_err()
    );
    std::fs::set_permissions(&config.path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let alias = fixture.path().join("alias");
    symlink(&config.path, &alias).unwrap();
    let mut changed = config;
    changed.path = alias.to_string_lossy().into_owned();
    assert!(
        fixture
            .ssh_target()
            .with_pinned_configuration(changed, vec![])
            .is_err()
    );
    assert!(fixture.ssh_calls_log().is_empty());
}
