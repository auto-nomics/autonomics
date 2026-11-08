//! Real-cluster smoke tests for the Slurm task backend: these submit actual
//! Slurm jobs over ssh, so they are `#[ignore]`d and must only be run with
//! explicit operator approval, e.g.
//!
//! ```text
//! cargo test -p dag-core --test slurm_smoke_real -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! Host and partition default to the 交我算 smoke queue (`sjtu`,
//! `debug64c512g`) and can be overridden with `AUTONOMICS_SMOKE_SSH_HOST` /
//! `AUTONOMICS_SMOKE_PARTITION`. Remote scratch lives under
//! `~/autonomics-smoke-<unix-stamp>`; each test prints its scratch directory
//! for the operator to clean up after triage.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dag_core::dag::graph::PortOutputs;
use dag_core::dag::node_event::{JobResult, NodeReporter};
use dag_core::node::{NodeInput, NodePorts};
use dag_core::registry::NodeCtx;
use dag_core::{
    FileRef, ProcessTaskOutput, ProcessTaskSpec, SlurmSettings, SlurmTaskTransport, SshTarget,
    TaskDispatch, TaskExecution, TaskExecutor, TaskInputBinding, TaskInputSource, TaskLease,
    TaskOutputBinding, TaskResources, TaskSpec, TaskSubmission, TaskTransport, dag::DagError,
    local_task_executor, slurm_backed_executor,
};
use datafusion::prelude::SessionContext;
use tokio_util::sync::CancellationToken;

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
            "ConnectTimeout=10",
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

fn execution(submission: TaskSubmission) -> TaskExecution {
    TaskExecution {
        submission,
        node: noop_node(),
        engine_ctx: test_ctx(),
        reporter: NodeReporter::noop(),
        cancellation: CancellationToken::new(),
    }
}

fn output_bindings(process: &ProcessTaskSpec) -> Vec<TaskOutputBinding> {
    process
        .outputs
        .iter()
        .map(|output| TaskOutputBinding {
            name: format!("port_{}", output.port),
            port: output.port,
            // The smoke output is a plain text file: the `file` payload is
            // restored as-is instead of being decoded as a Channel.
            payload: "file".into(),
            artifact_paths: Vec::new(),
        })
        .collect()
}

fn input_binding(input: &FileRef) -> TaskInputBinding {
    TaskInputBinding {
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
    }
}

fn settings(
    host: &str,
    partition: String,
    scratch: String,
    local_artifacts: &Path,
) -> SlurmSettings {
    SlurmSettings {
        apptainer: None,
        ssh: SshTarget::new("ssh", host),
        scratch_dir: scratch,
        local_artifact_root: local_artifacts.to_path_buf(),
        partition,
        account: None,
        gpus: None,
        qos: None,
        mem_per_cpu: "8G".into(),
        default_time: "00:10:00".into(),
        poll_interval: Duration::from_secs(3),
    }
}

// ── tests ────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "submits real Slurm jobs over ssh; run only with explicit operator approval"]
async fn real_cluster_runs_process_task_end_to_end() {
    let host = smoke_env("AUTONOMICS_SMOKE_SSH_HOST", "sjtu");
    let partition = smoke_env("AUTONOMICS_SMOKE_PARTITION", "debug64c512g");
    let scratch = format!("~/autonomics-smoke-{}", unix_stamp());
    let task_id = "smoke-ok";
    let local_artifacts = tempfile::tempdir().unwrap();
    let coordinator = tempfile::tempdir().unwrap();

    let input_path = coordinator.path().join("input.txt");
    std::fs::write(&input_path, "remote smoke input").unwrap();
    let input = FileRef::local(&input_path, Some("text".into())).unwrap();

    // sleep 5 keeps the job observable in squeue; the script consumes the
    // uploaded input so the artifact-store path is proven end to end.
    let process = ProcessTaskSpec {
        container: None,
        script: Some("sleep 5\necho ok > out-0.txt\ncat {{input.f}} >> out-0.txt\n".into()),
        command: Vec::new(),
        env: Default::default(),
        outputs: vec![ProcessTaskOutput {
            port: 0,
            pattern: "out-*.txt".into(),
            optional: false,
        }],
    };
    let submission = TaskSubmission {
        spec: TaskSpec {
            id: task_id.into(),
            logical_node: Some("process".into()),
            kind: "process".into(),
            spec: serde_json::to_value(&process).unwrap(),
            item_key: None,
            item: None,
            inputs: vec![input_binding(&input)],
            outputs: output_bindings(&process),
        },
        inputs: vec![NodeInput::file(0, input)],
        resources: TaskResources::default(),
    };

    let executor = slurm_backed_executor(
        settings(&host, partition, scratch.clone(), local_artifacts.path()),
        coordinator.path(),
        local_task_executor(),
    );

    let result = executor.run(execution(submission)).await;
    let JobResult::Success {
        outputs, details, ..
    } = result
    else {
        panic!("real-cluster process task should succeed: {result:?}");
    };

    // Output restored as a file value pointing at the downloaded artifact.
    let restored = outputs.get(&0).unwrap().as_file().unwrap();
    assert!(restored.path.contains("output-0-0"), "{}", restored.path);

    let details = details.expect("success carries an attempt receipt");
    assert_eq!(details.exit_code, Some(0));
    let downloaded = &details.output_artifacts_by_port[&0][0];
    // `echo ok` contributes "ok\n" and `cat` appends the uploaded input byte
    // for byte (no trailing newline was written locally).
    assert_eq!(
        std::fs::read_to_string(&downloaded.path).unwrap(),
        "ok\nremote smoke input"
    );

    // Manifest custody on the coordinator side.
    let manifest_path = local_artifacts.path().join(task_id).join("task.json");
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["spec"]["id"], task_id);

    // The uploaded input really landed in the cluster scratch.
    let uploaded = ssh_text(&host, &format!("cat {scratch}/{task_id}/input-0-0"));
    assert_eq!(uploaded, "remote smoke input");

    println!("smoke-ok job completed; receipt: {details:?}");
    println!("remote scratch to clean: {scratch}");
}

#[tokio::test]
#[ignore = "submits real Slurm jobs over ssh; run only with explicit operator approval"]
async fn real_cluster_cancel_paths() {
    let host = smoke_env("AUTONOMICS_SMOKE_SSH_HOST", "sjtu");
    let partition = smoke_env("AUTONOMICS_SMOKE_PARTITION", "debug64c512g");
    let scratch = format!("~/autonomics-smoke-{}", unix_stamp());
    let local_artifacts = tempfile::tempdir().unwrap();
    let staged = tempfile::tempdir().unwrap();
    let transport = SlurmTaskTransport::new(settings(
        &host,
        partition,
        scratch.clone(),
        local_artifacts.path(),
    ));

    let input_path = staged.path().join("input.txt");
    std::fs::write(&input_path, "cancel smoke").unwrap();

    let long = ProcessTaskSpec {
        container: None,
        script: Some("sleep 300\n".into()),
        command: Vec::new(),
        env: Default::default(),
        outputs: vec![ProcessTaskOutput {
            port: 0,
            pattern: "out-*.txt".into(),
            optional: false,
        }],
    };

    // Transport-level submit with the input pre-staged the way the remote
    // executor would provide it after upload.
    async fn submit_sleep(
        transport: &SlurmTaskTransport,
        task_id: &str,
        process: &ProcessTaskSpec,
        staged: &Path,
    ) -> TaskLease {
        let input = FileRef::local(staged.join("input.txt"), Some("text".into())).unwrap();
        let mut spec = TaskSpec {
            id: task_id.into(),
            logical_node: Some("process".into()),
            kind: "process".into(),
            spec: serde_json::to_value(process).unwrap(),
            item_key: None,
            item: None,
            inputs: vec![input_binding(&input)],
            outputs: output_bindings(process),
        };
        spec.inputs[0].staged_paths = vec![staged.join("input.txt").to_string_lossy().into_owned()];
        let dispatch: TaskDispatch = TaskSubmission {
            spec,
            inputs: vec![NodeInput::file(0, input.clone())],
            resources: TaskResources::default(),
        }
        .into_remote_dispatch()
        .unwrap();
        transport.submit(dispatch, vec![input]).await.unwrap()
    }

    // (1) Operator-side scancel: the real wait() must observe the job leave
    // the queue and map the cluster's CANCELLED state to the protocol receipt.
    let lease_a = submit_sleep(&transport, "smoke-cancel-external", &long, staged.path()).await;
    println!("external-cancel job id: {}", lease_a.lease_id);
    ssh_text(&host, &format!("scancel {}", lease_a.lease_id));
    let receipt = transport.wait(&lease_a).await.unwrap();
    assert_eq!(receipt.status, "cancelled");
    assert_eq!(receipt.exit_code, 130);

    // (2) Transport-level cancel: the lease is consumed, scancel hit the
    // cluster, and a later wait reports the lease as cancelled.
    let lease_b = submit_sleep(&transport, "smoke-cancel-transport", &long, staged.path()).await;
    println!("transport-cancel job id: {}", lease_b.lease_id);
    transport.cancel(&lease_b).await.unwrap();
    let error = transport.wait(&lease_b).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unknown, cancelled, or already consumed"),
        "unexpected error: {error}"
    );

    // Cluster accounting must confirm both cancellations (records can lag the
    // kill by a few seconds).
    for job in [lease_a.lease_id.as_str(), lease_b.lease_id.as_str()] {
        let mut state = String::new();
        for _ in 0..15 {
            state = ssh_text(&host, &format!("sacct -j {job} -n -P -X -o State"));
            if state.contains("CANCELLED") {
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        assert!(
            state.contains("CANCELLED"),
            "job {job} accounting: {state:?}"
        );
    }

    println!("remote scratch to clean: {scratch}");
}
