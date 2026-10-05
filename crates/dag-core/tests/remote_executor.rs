use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arrow_array::{Array, Int64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use dag_core::dag::graph::PortOutputs;
use dag_core::dag::node_event::{JobResult, NodeReporter};
use dag_core::node::{NodeInput, NodePorts};
use dag_core::registry::NodeCtx;
use dag_core::{
    LocalDirectoryArtifactStore, PortType, ProcessTaskTransport, RemoteTaskExecutor,
    TaskAttemptReceipt, TaskDispatch, TaskExecution, TaskExecutor, TaskInputBinding,
    TaskInputSource, TaskLease, TaskOutputBinding, TaskResources, TaskSpec, TaskSubmission,
    TaskTransport, dag::DagError,
};
use datafusion::prelude::SessionContext;
use tokio_util::sync::CancellationToken;

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

#[derive(Clone, Copy)]
enum RemoteWait {
    Success,
    Failure,
    Pending,
}

#[derive(Default)]
struct FakeTransportState {
    dispatch: Option<TaskDispatch>,
    uploaded_inputs: Vec<dag_core::FileRef>,
    cancelled_leases: Vec<TaskLease>,
    receipt: Option<TaskAttemptReceipt>,
}

struct FakeRemoteTransport {
    root: PathBuf,
    wait: RemoteWait,
    state: Mutex<FakeTransportState>,
}

impl FakeRemoteTransport {
    fn new(root: impl Into<PathBuf>, wait: RemoteWait) -> Self {
        Self {
            root: root.into(),
            wait,
            state: Mutex::new(FakeTransportState::default()),
        }
    }

    fn write_arrow(path: &Path) -> Result<dag_core::FileRef, DagError> {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "value",
            DataType::Int64,
            false,
        )]));
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![Arc::new(Int64Array::from(vec![7])) as Arc<dyn Array>],
        )
        .map_err(|error| DagError::Schedule(error.to_string()))?;
        let file =
            std::fs::File::create(path).map_err(|error| DagError::Schedule(error.to_string()))?;
        let mut writer = arrow::ipc::writer::FileWriter::try_new(file, schema.as_ref())
            .map_err(|error| DagError::Schedule(error.to_string()))?;
        writer
            .write(&batch)
            .and_then(|()| writer.finish())
            .map_err(|error| DagError::Schedule(error.to_string()))?;
        dag_core::FileRef::local(path, Some("arrow".into()))
    }

    fn write_receipt(&self, dispatch: &TaskDispatch) -> TaskAttemptReceipt {
        std::fs::create_dir_all(&self.root).unwrap();
        let manifest_path = self.root.join("task.json");
        std::fs::write(&manifest_path, serde_json::to_vec(&dispatch.spec).unwrap()).unwrap();

        let mut artifacts_by_port = BTreeMap::new();
        for output in &dispatch.spec.outputs {
            let mut artifacts = Vec::new();
            match output.payload.as_str() {
                "dataframe" => {
                    let path = self.root.join(format!("port-{}.arrow", output.port));
                    artifacts.push(Self::write_arrow(&path).unwrap());
                }
                "channel" => {
                    let path = self.root.join(format!("port-{}.json", output.port));
                    std::fs::write(&path, br#"[{"value":42}]"#).unwrap();
                    artifacts.push(dag_core::FileRef::local(&path, Some("json".into())).unwrap());
                }
                _ => {}
            }
            artifacts_by_port.insert(output.port, artifacts);
        }
        let output_artifacts = artifacts_by_port
            .values()
            .flat_map(|artifacts| artifacts.iter().cloned())
            .collect::<Vec<_>>();
        TaskAttemptReceipt {
            task_id: dispatch.spec.id.clone(),
            executor: "fake-remote".into(),
            status: "success".into(),
            elapsed_ms: 12,
            exit_code: 0,
            workspace: self.root.to_string_lossy().into_owned(),
            task_manifest: dag_core::FileRef::local(&manifest_path, Some("json".into())).unwrap(),
            output_artifacts,
            output_artifacts_by_port: artifacts_by_port,
        }
    }

    fn dispatch(&self) -> TaskDispatch {
        self.state.lock().unwrap().dispatch.clone().unwrap()
    }

    fn uploaded_inputs(&self) -> Vec<dag_core::FileRef> {
        self.state.lock().unwrap().uploaded_inputs.clone()
    }

    fn cancelled_leases(&self) -> Vec<TaskLease> {
        self.state.lock().unwrap().cancelled_leases.clone()
    }
}

#[async_trait::async_trait]
impl TaskTransport for FakeRemoteTransport {
    async fn submit(
        &self,
        dispatch: TaskDispatch,
        artifacts: Vec<dag_core::FileRef>,
    ) -> Result<TaskLease, DagError> {
        let receipt = self.write_receipt(&dispatch);
        let lease = TaskLease {
            lease_id: format!("lease-{}", dispatch.spec.id),
            task_id: dispatch.spec.id.clone(),
        };
        let mut state = self.state.lock().unwrap();
        state.dispatch = Some(dispatch);
        state.uploaded_inputs = artifacts;
        state.receipt = Some(receipt);
        Ok(lease)
    }

    async fn wait(&self, _lease: &TaskLease) -> Result<TaskAttemptReceipt, DagError> {
        match self.wait {
            RemoteWait::Success => self
                .state
                .lock()
                .unwrap()
                .receipt
                .clone()
                .ok_or_else(|| DagError::Schedule("missing fake receipt".into())),
            RemoteWait::Failure => {
                let mut receipt = self
                    .state
                    .lock()
                    .unwrap()
                    .receipt
                    .clone()
                    .ok_or_else(|| DagError::Schedule("missing fake receipt".into()))?;
                receipt.status = "failed".into();
                receipt.exit_code = 42;
                receipt.output_artifacts.clear();
                receipt.output_artifacts_by_port.clear();
                Ok(receipt)
            }
            RemoteWait::Pending => std::future::pending().await,
        }
    }

    async fn cancel(&self, lease: &TaskLease) -> Result<(), DagError> {
        self.state
            .lock()
            .unwrap()
            .cancelled_leases
            .push(lease.clone());
        Ok(())
    }
}

fn test_ctx() -> Arc<NodeCtx> {
    Arc::new(NodeCtx::new(SessionContext::new().runtime_env(), None))
}

fn submission(
    task_id: &str,
    input_path: &Path,
    outputs: &[(u8, PortType)],
    resources: TaskResources,
) -> TaskSubmission {
    let input = dag_core::FileRef::local(input_path, Some("text".into())).unwrap();
    TaskSubmission {
        spec: TaskSpec {
            id: task_id.into(),
            logical_node: Some("process".into()),
            kind: "noop".into(),
            spec: serde_json::json!({"mode": "remote"}),
            axis: None,
            item_key: None,
            item: None,
            inputs: vec![TaskInputBinding {
                name: "input".into(),
                port: 0,
                source: TaskInputSource::UpstreamPort {
                    from: "source".into(),
                    from_port: 0,
                },
                payload: "file".into(),
                path: Some(input.path.clone()),
                fingerprint: input.fingerprint.clone(),
                staged_paths: Vec::new(),
            }],
            outputs: outputs
                .iter()
                .map(|(port, payload)| TaskOutputBinding {
                    name: format!("port_{port}"),
                    port: *port,
                    payload: payload.to_string(),
                    artifact_paths: Vec::new(),
                })
                .collect(),
        },
        inputs: vec![NodeInput::file(0, input)],
        resources,
    }
}

fn execution(submission: TaskSubmission, cancellation: CancellationToken) -> TaskExecution {
    TaskExecution {
        submission,
        node: Box::new(NoopNode::default()),
        engine_ctx: test_ctx(),
        reporter: NodeReporter::noop(),
        cancellation,
    }
}

#[tokio::test]
async fn remote_executor_stages_inputs_and_restores_typed_outputs() {
    let coordinator = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    let transfers = tempfile::tempdir().unwrap();
    let input_path = coordinator.path().join("input.txt");
    std::fs::write(&input_path, "remote input").unwrap();

    let submission = submission(
        "typed-remote",
        &input_path,
        &[(0, PortType::DataFrame), (1, PortType::Channel)],
        TaskResources::default(),
    );
    let transport = Arc::new(FakeRemoteTransport::new(remote.path(), RemoteWait::Success));
    let executor = RemoteTaskExecutor::new(
        Arc::clone(&transport) as Arc<dyn TaskTransport>,
        Arc::new(LocalDirectoryArtifactStore::new(transfers.path())),
        coordinator.path(),
    );

    let result = executor
        .run(execution(submission, CancellationToken::new()))
        .await;
    let JobResult::Success {
        outputs, details, ..
    } = result
    else {
        panic!("remote execution should succeed: {result:?}");
    };

    let dispatch = transport.dispatch();
    assert_eq!(dispatch.spec.inputs[0].staged_paths.len(), 1);
    let uploads = transport.uploaded_inputs();
    assert_eq!(uploads.len(), 1);
    assert!(Path::new(&uploads[0].path).is_file());

    let dataframe = outputs.get(&0).unwrap().as_dataframe().unwrap();
    assert_eq!(dataframe.clone().collect().await.unwrap()[0].num_rows(), 1);
    assert_eq!(
        outputs.get(&1).unwrap().as_channel().unwrap().items.len(),
        1
    );

    let details = details.unwrap();
    assert_eq!(details.exit_code, Some(0));
    assert_eq!(details.output_artifacts_by_port.len(), 2);
    assert!(
        details.output_artifacts_by_port[&0][0]
            .path
            .contains("output-0-0")
    );
    assert!(
        details.output_artifacts_by_port[&1][0]
            .path
            .contains("output-1-0")
    );
}

#[tokio::test]
async fn remote_executor_maps_failed_receipt_to_failed_job() {
    let coordinator = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    let transfers = tempfile::tempdir().unwrap();
    let input_path = coordinator.path().join("input.txt");
    std::fs::write(&input_path, "remote input").unwrap();
    let submission = submission("failed-remote", &input_path, &[], TaskResources::default());
    let transport = Arc::new(FakeRemoteTransport::new(remote.path(), RemoteWait::Failure));
    let executor = RemoteTaskExecutor::new(
        Arc::clone(&transport) as Arc<dyn TaskTransport>,
        Arc::new(LocalDirectoryArtifactStore::new(transfers.path())),
        coordinator.path(),
    );

    let result = executor
        .run(execution(submission, CancellationToken::new()))
        .await;
    let JobResult::Failed { error, details, .. } = result else {
        panic!("failed receipt should fail the job: {result:?}");
    };
    assert!(error.to_string().contains("status `failed`"));
    assert_eq!(details.unwrap().exit_code, Some(42));
}

#[tokio::test]
async fn remote_executor_times_out_and_cancels_lease() {
    let coordinator = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    let transfers = tempfile::tempdir().unwrap();
    let input_path = coordinator.path().join("input.txt");
    std::fs::write(&input_path, "remote input").unwrap();
    let submission = submission(
        "timeout-remote",
        &input_path,
        &[],
        TaskResources {
            max_duration_ms: Some(10),
            ..TaskResources::default()
        },
    );
    let transport = Arc::new(FakeRemoteTransport::new(remote.path(), RemoteWait::Pending));
    let executor = RemoteTaskExecutor::new(
        Arc::clone(&transport) as Arc<dyn TaskTransport>,
        Arc::new(LocalDirectoryArtifactStore::new(transfers.path())),
        coordinator.path(),
    );

    let result = executor
        .run(execution(submission, CancellationToken::new()))
        .await;
    assert!(matches!(result, JobResult::Failed { .. }));
    assert_eq!(transport.cancelled_leases().len(), 1);
}

#[tokio::test]
async fn remote_executor_cancels_pending_lease() {
    let coordinator = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    let transfers = tempfile::tempdir().unwrap();
    let input_path = coordinator.path().join("input.txt");
    std::fs::write(&input_path, "remote input").unwrap();
    let submission = submission("cancel-remote", &input_path, &[], TaskResources::default());
    let transport = Arc::new(FakeRemoteTransport::new(remote.path(), RemoteWait::Pending));
    let executor = RemoteTaskExecutor::new(
        Arc::clone(&transport) as Arc<dyn TaskTransport>,
        Arc::new(LocalDirectoryArtifactStore::new(transfers.path())),
        coordinator.path(),
    );
    let cancellation = CancellationToken::new();
    let execution_cancellation = cancellation.clone();
    let waiter = tokio::spawn(async move {
        executor
            .run(execution(submission, execution_cancellation))
            .await
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    cancellation.cancel();
    assert!(matches!(waiter.await.unwrap(), JobResult::Failed { .. }));
    assert_eq!(transport.cancelled_leases().len(), 1);
}

fn process_submission(
    task_id: &str,
    input_path: &Path,
    process_spec: serde_json::Value,
    output_type: PortType,
    resources: TaskResources,
) -> TaskSubmission {
    let mut submission = submission(task_id, input_path, &[(0, output_type)], resources);
    submission.spec.kind = "process".into();
    submission.spec.spec = process_spec;
    submission
}

#[tokio::test]
async fn process_transport_runs_memory_independent_task_end_to_end() {
    let coordinator = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    let transfers = tempfile::tempdir().unwrap();
    let input_path = coordinator.path().join("input.txt");
    std::fs::write(&input_path, "process input").unwrap();

    let submission = process_submission(
        "process-copy",
        &input_path,
        serde_json::json!({
            "script": "printf 'prefix:' > result.txt; cat '{{input.input}}' >> result.txt",
            "outputs": [{"port": 0, "pattern": "result.txt"}]
        }),
        PortType::File,
        TaskResources::default(),
    );
    let executor = RemoteTaskExecutor::new(
        Arc::new(ProcessTaskTransport::new(remote.path())),
        Arc::new(LocalDirectoryArtifactStore::new(transfers.path())),
        coordinator.path(),
    );

    let result = executor
        .run(execution(submission, CancellationToken::new()))
        .await;
    let JobResult::Success {
        outputs, details, ..
    } = result
    else {
        panic!("process transport should succeed: {result:?}");
    };
    let output = outputs.get(&0).unwrap().as_file().unwrap().clone();
    assert_eq!(
        std::fs::read_to_string(&output.path).unwrap(),
        "prefix:process input"
    );
    assert!(Path::new(&output.path).is_file());

    let details = details.unwrap();
    assert_eq!(details.exit_code, Some(0));
    assert_eq!(details.output_artifacts_by_port[&0].len(), 1);
    let receipt = serde_json::from_str::<TaskAttemptReceipt>(
        &std::fs::read_to_string(remote.path().join("process-copy/lease-1/attempt.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(receipt.executor, "process");
    assert_eq!(receipt.status, "success");
    assert!(
        remote
            .path()
            .join("process-copy/lease-1/stdout.log")
            .is_file()
    );
}

#[tokio::test]
async fn process_transport_collects_multi_file_output_ports() {
    let coordinator = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    let transfers = tempfile::tempdir().unwrap();
    let input_path = coordinator.path().join("input.txt");
    std::fs::write(&input_path, "abc").unwrap();

    let submission = process_submission(
        "process-glob",
        &input_path,
        serde_json::json!({
            "script": "mkdir results; cp '{{input.input}}' results/a.txt; cp '{{input.input}}' results/b.txt",
            "outputs": [{"port": 0, "pattern": "results/*.txt"}]
        }),
        PortType::FileSet,
        TaskResources::default(),
    );
    let executor = RemoteTaskExecutor::new(
        Arc::new(ProcessTaskTransport::new(remote.path())),
        Arc::new(LocalDirectoryArtifactStore::new(transfers.path())),
        coordinator.path(),
    );

    let result = executor
        .run(execution(submission, CancellationToken::new()))
        .await;
    let JobResult::Success { outputs, .. } = result else {
        panic!("multi-file process output should succeed: {result:?}");
    };
    let files = outputs.get(&0).unwrap().as_file_set().unwrap();
    assert_eq!(files.len(), 2);
    assert!(files.iter().all(|file| Path::new(&file.path).is_file()));
}

#[tokio::test]
async fn process_transport_reports_nonzero_exit() {
    let coordinator = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    let transfers = tempfile::tempdir().unwrap();
    let input_path = coordinator.path().join("input.txt");
    std::fs::write(&input_path, "abc").unwrap();
    let submission = process_submission(
        "process-failure",
        &input_path,
        serde_json::json!({
            "script": "echo process-failure >&2; exit 37",
            "outputs": [{"port": 0, "pattern": "result.txt"}]
        }),
        PortType::File,
        TaskResources::default(),
    );
    let executor = RemoteTaskExecutor::new(
        Arc::new(ProcessTaskTransport::new(remote.path())),
        Arc::new(LocalDirectoryArtifactStore::new(transfers.path())),
        coordinator.path(),
    );

    let result = executor
        .run(execution(submission, CancellationToken::new()))
        .await;
    let JobResult::Failed { error, details, .. } = result else {
        panic!("nonzero process exit should fail: {result:?}");
    };
    assert!(error.to_string().contains("exit code `37`"));
    assert_eq!(details.unwrap().exit_code, Some(37));
}

#[tokio::test]
async fn process_transport_enforces_timeout() {
    let coordinator = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    let transfers = tempfile::tempdir().unwrap();
    let input_path = coordinator.path().join("input.txt");
    std::fs::write(&input_path, "abc").unwrap();
    let mut submission = process_submission(
        "process-timeout",
        &input_path,
        serde_json::json!({"script": "sleep 5", "outputs": []}),
        PortType::File,
        TaskResources {
            max_duration_ms: Some(20),
            ..TaskResources::default()
        },
    );
    submission.spec.outputs.clear();
    let executor = RemoteTaskExecutor::new(
        Arc::new(ProcessTaskTransport::new(remote.path())),
        Arc::new(LocalDirectoryArtifactStore::new(transfers.path())),
        coordinator.path(),
    );

    let result = executor
        .run(execution(submission, CancellationToken::new()))
        .await;
    let JobResult::Failed { error, details, .. } = result else {
        panic!("process timeout should fail: {result:?}");
    };
    assert!(
        error.to_string().contains("timed out"),
        "unexpected timeout error: {error}"
    );
    assert_eq!(details.unwrap().exit_code, Some(124));
}

#[tokio::test]
async fn process_transport_rejects_workspace_escape_patterns() {
    let coordinator = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    let transfers = tempfile::tempdir().unwrap();
    let input_path = coordinator.path().join("input.txt");
    std::fs::write(&input_path, "abc").unwrap();
    let submission = process_submission(
        "process-escape",
        &input_path,
        serde_json::json!({
            "script": "true",
            "outputs": [{"port": 0, "pattern": "../escape.txt"}]
        }),
        PortType::File,
        TaskResources::default(),
    );
    let executor = RemoteTaskExecutor::new(
        Arc::new(ProcessTaskTransport::new(remote.path())),
        Arc::new(LocalDirectoryArtifactStore::new(transfers.path())),
        coordinator.path(),
    );

    let result = executor
        .run(execution(submission, CancellationToken::new()))
        .await;
    let JobResult::Failed { error, .. } = result else {
        panic!("workspace escape pattern should fail: {result:?}");
    };
    assert!(
        error
            .to_string()
            .contains("must stay inside the task workspace")
    );
}

#[tokio::test]
async fn process_transport_cancels_pending_process() {
    let coordinator = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    let transfers = tempfile::tempdir().unwrap();
    let input_path = coordinator.path().join("input.txt");
    std::fs::write(&input_path, "abc").unwrap();
    let mut submission = process_submission(
        "process-cancel",
        &input_path,
        serde_json::json!({"script": "sleep 5", "outputs": []}),
        PortType::File,
        TaskResources::default(),
    );
    submission.spec.outputs.clear();
    let cancellation = CancellationToken::new();
    let execution_cancellation = cancellation.clone();
    let executor = RemoteTaskExecutor::new(
        Arc::new(ProcessTaskTransport::new(remote.path())),
        Arc::new(LocalDirectoryArtifactStore::new(transfers.path())),
        coordinator.path(),
    );
    let waiter = tokio::spawn(async move {
        executor
            .run(execution(submission, execution_cancellation))
            .await
    });

    tokio::time::sleep(Duration::from_millis(10)).await;
    cancellation.cancel();
    assert!(matches!(waiter.await.unwrap(), JobResult::Failed { .. }));
}

#[tokio::test]
async fn process_transport_rejects_requests_over_resource_limit() {
    let coordinator = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    let transfers = tempfile::tempdir().unwrap();
    let input_path = coordinator.path().join("input.txt");
    std::fs::write(&input_path, "abc").unwrap();
    let mut submission = process_submission(
        "process-resource",
        &input_path,
        serde_json::json!({"script": "true", "outputs": []}),
        PortType::File,
        TaskResources {
            cpus: Some(2),
            ..TaskResources::default()
        },
    );
    submission.spec.outputs.clear();
    let executor = RemoteTaskExecutor::new(
        Arc::new(
            ProcessTaskTransport::with_workspace_root_and_resource_limits(
                remote.path(),
                Some(1),
                None,
            )
            .unwrap(),
        ),
        Arc::new(LocalDirectoryArtifactStore::new(transfers.path())),
        coordinator.path(),
    );

    let result = executor
        .run(execution(submission, CancellationToken::new()))
        .await;
    let JobResult::Failed { error, .. } = result else {
        panic!("oversized process resource request should fail: {result:?}");
    };
    assert!(error.to_string().contains("requests 2"));
}
