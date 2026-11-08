//! Real toy subprocesses verify artifact ownership across repeated logical IDs.
use super::*;

use datafusion::prelude::SessionContext;
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::dag::{
    ChannelNode, ChannelOperator, execution::ProcessTaskTransport, node_event::NodeReporter,
};
use crate::{
    NodeInput, TaskDispatch, TaskInputBinding, TaskInputSource, TaskLease, TaskOutputBinding,
    TaskResources, TaskSpec, TaskSubmission,
};

struct RecordingTransport {
    process: ProcessTaskTransport,
    dispatches: Arc<Mutex<Vec<(TaskDispatch, Vec<FileRef>)>>>,
}

struct UnconfirmedTransport {
    submitted: Arc<tokio::sync::Notify>,
}

struct UncertainSubmissionTransport;

#[async_trait::async_trait]
impl TaskTransport for UncertainSubmissionTransport {
    fn submission_failure_may_be_running(&self, _error: &DagError) -> bool {
        true
    }
    async fn submit(
        &self,
        _dispatch: TaskDispatch,
        _artifacts: Vec<FileRef>,
    ) -> Result<TaskLease, DagError> {
        Err(DagError::Schedule("submission acknowledgement lost".into()))
    }
    async fn wait(&self, _lease: &TaskLease) -> Result<TaskAttemptReceipt, DagError> {
        unreachable!()
    }
    async fn cancel(&self, _lease: &TaskLease) -> Result<(), DagError> {
        unreachable!()
    }
}

#[tokio::test]
async fn uncertain_submission_is_reported_as_unconfirmed() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("input.txt");
    std::fs::write(&input, b"input").unwrap();
    let executor = RemoteTaskExecutor::new(
        Arc::new(UncertainSubmissionTransport),
        Arc::new(LocalDirectoryArtifactStore::new(
            temp.path().join("artifacts"),
        )),
        temp.path().join("coordinator"),
    );
    match executor.run(execution(&input)).await {
        JobResult::Failed { error, .. } => assert!(
            error.to_string().contains("remote_stop_unconfirmed"),
            "{error}"
        ),
        other => panic!("uncertain submission must fail: {other:?}"),
    }
}

#[async_trait::async_trait]
impl TaskTransport for UnconfirmedTransport {
    async fn submit(
        &self,
        dispatch: TaskDispatch,
        _artifacts: Vec<FileRef>,
    ) -> Result<TaskLease, DagError> {
        self.submitted.notify_one();
        Ok(TaskLease {
            lease_id: "123".into(),
            task_id: dispatch.spec.id,
        })
    }

    async fn wait(&self, _lease: &TaskLease) -> Result<TaskAttemptReceipt, DagError> {
        std::future::pending().await
    }

    async fn cancel(&self, _lease: &TaskLease) -> Result<(), DagError> {
        Err(DagError::Schedule("scheduler unavailable".into()))
    }
}

#[tokio::test]
async fn remote_cancel_failure_is_reported_as_unconfirmed() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("input.txt");
    std::fs::write(&input, b"input").unwrap();
    let submitted = Arc::new(tokio::sync::Notify::new());
    let executor = Arc::new(RemoteTaskExecutor::new(
        Arc::new(UnconfirmedTransport {
            submitted: submitted.clone(),
        }),
        Arc::new(LocalDirectoryArtifactStore::new(
            temp.path().join("artifacts"),
        )),
        temp.path().join("coordinator"),
    ));
    let execution = execution(&input);
    let token = execution.cancellation.clone();
    let run = tokio::spawn(async move { executor.run(execution).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), submitted.notified())
        .await
        .unwrap();
    token.cancel();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), run)
        .await
        .unwrap()
        .unwrap();
    match result {
        JobResult::Failed { error, .. } => assert!(
            error.to_string().contains("remote_stop_unconfirmed"),
            "{error}"
        ),
        other => panic!("unconfirmed cancellation must fail: {other:?}"),
    }
}

#[async_trait::async_trait]
impl TaskTransport for RecordingTransport {
    async fn submit(
        &self,
        dispatch: TaskDispatch,
        artifacts: Vec<FileRef>,
    ) -> Result<TaskLease, DagError> {
        self.dispatches
            .lock()
            .unwrap()
            .push((dispatch.clone(), artifacts.clone()));
        self.process.submit(dispatch, artifacts).await
    }
    async fn wait(&self, lease: &TaskLease) -> Result<TaskAttemptReceipt, DagError> {
        self.process.wait(lease).await
    }
    async fn cancel(&self, lease: &TaskLease) -> Result<(), DagError> {
        self.process.cancel(lease).await
    }
}

fn execution(input: &Path) -> TaskExecution {
    TaskExecution {
        submission: TaskSubmission {
            spec: TaskSpec {
                id: "same-logical-node".into(),
                logical_node: None,
                kind: "process".into(),
                spec: serde_json::json!({"script":"cat -- \"$AUTONOMICS_INPUT_PAYLOAD\" > result.txt", "outputs":[{"port":0,"pattern":"result.txt"}]}),
                item_key: None,
                item: None,
                inputs: vec![TaskInputBinding {
                    name: "payload".into(),
                    port: 0,
                    source: TaskInputSource::UpstreamPort {
                        from: "source".into(),
                        from_port: 0,
                    },
                    payload: "file".into(),
                    path: None,
                    fingerprint: None,
                    staged_paths: vec![],
                }],
                outputs: vec![TaskOutputBinding {
                    name: "result".into(),
                    port: 0,
                    payload: "file".into(),
                    artifact_paths: vec![],
                }],
            },
            inputs: vec![NodeInput::file(
                0,
                FileRef::local(input, Some("txt".into())).unwrap(),
            )],
            resources: TaskResources::default(),
        },
        node: Box::new(ChannelNode::new(ChannelOperator::OfItems { items: vec![] })),
        engine_ctx: Arc::new(NodeCtx::new(SessionContext::new().runtime_env(), None)),
        reporter: NodeReporter::noop(),
        cancellation: CancellationToken::new(),
    }
}

fn result_file(result: JobResult) -> FileRef {
    let JobResult::Success { id, outputs, .. } = result else {
        panic!("real toy process should succeed: {result:?}")
    };
    assert_eq!(id, "same-logical-node");
    let NodeValue::File(file) = &outputs[&0] else {
        panic!("expected file output")
    };
    file.clone()
}

#[tokio::test]
async fn optional_process_output_can_be_absent_but_required_output_cannot() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("input.txt");
    std::fs::write(&input, b"input").unwrap();
    let executor = RemoteTaskExecutor::new(
        Arc::new(ProcessTaskTransport::new(temp.path().join("workers"))),
        Arc::new(LocalDirectoryArtifactStore::new(
            temp.path().join("artifacts"),
        )),
        temp.path().join("coordinator"),
    );
    let mut optional = execution(&input);
    optional.submission.spec.spec = serde_json::json!({
        "script": "true",
        "outputs": [{"port": 0, "pattern": "missing.txt", "optional": true}]
    });
    match executor.run(optional).await {
        JobResult::Success { outputs, .. } => assert!(outputs.is_empty()),
        other => panic!("optional missing output should succeed: {other:?}"),
    }
    let mut required = execution(&input);
    required.submission.spec.spec = serde_json::json!({
        "script": "true", "outputs": [{"port": 0, "pattern": "missing.txt"}]
    });
    assert!(matches!(
        executor.run(required).await,
        JobResult::Failed { .. }
    ));
}

#[tokio::test]
async fn repeated_logical_task_ids_never_share_uploaded_inputs_or_downloaded_outputs() {
    let temp = tempfile::tempdir().unwrap();
    let first_input = temp.path().join("first.txt");
    std::fs::write(&first_input, b"first invocation").unwrap();
    let second_input = temp.path().join("second.txt");
    std::fs::write(&second_input, b"second invocation").unwrap();
    let dispatches = Arc::new(Mutex::new(Vec::new()));
    let transport = Arc::new(RecordingTransport {
        process: ProcessTaskTransport::new(temp.path().join("workers")),
        dispatches: dispatches.clone(),
    });
    let executor = RemoteTaskExecutor::new(
        transport,
        Arc::new(LocalDirectoryArtifactStore::new(
            temp.path().join("artifacts"),
        )),
        temp.path().join("coordinator"),
    );
    let first = result_file(executor.run(execution(&first_input)).await);
    let second = result_file(executor.run(execution(&second_input)).await);
    assert_ne!(first.path, second.path);
    assert_eq!(std::fs::read(&first.path).unwrap(), b"first invocation");
    assert_eq!(std::fs::read(&second.path).unwrap(), b"second invocation");
    let dispatches = dispatches.lock().unwrap();
    assert_eq!(dispatches.len(), 2);
    let keys = dispatches
        .iter()
        .map(|(dispatch, artifacts)| {
            assert_eq!(dispatch.spec.id, "same-logical-node");
            let key = dispatch.spec.spec["_autonomics_attempt_key"]
                .as_str()
                .unwrap();
            assert_eq!(key.len(), 72);
            assert!(key.starts_with("attempt-"));
            assert!(
                key[8..]
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            );
            assert!(artifacts[0].path.contains(key));
            key
        })
        .collect::<Vec<_>>();
    assert_ne!(keys[0], keys[1]);
}

#[tokio::test]
async fn failed_process_preserves_real_exit_stderr_and_attempt_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let executor = RemoteTaskExecutor::new(
        Arc::new(ProcessTaskTransport::new(temp.path().join("workers"))),
        Arc::new(LocalDirectoryArtifactStore::new(
            temp.path().join("artifacts"),
        )),
        temp.path().join("coordinator"),
    );
    let input = temp.path().join("input");
    std::fs::write(&input, b"input").unwrap();
    let mut invocation = execution(&input);
    invocation.submission.spec.outputs.clear();
    invocation.submission.spec.spec = serde_json::json!({"script":r"printf 'worker started\n'; printf 'archive input missing on worker\n' >&2; exit 17"});
    let JobResult::Failed {
        error,
        details: Some(details),
        ..
    } = executor.run(invocation).await
    else {
        panic!("expected a failed process")
    };
    assert!(
        error
            .to_string()
            .contains("archive input missing on worker")
    );
    assert_eq!(details.exit_code, Some(17));
    assert_eq!(
        std::fs::read_to_string(&details.stderr_log.as_ref().unwrap().path).unwrap(),
        "archive input missing on worker\n"
    );
    assert_eq!(
        std::fs::read_to_string(&details.stdout_log.as_ref().unwrap().path).unwrap(),
        "worker started\n"
    );
    let receipt: TaskAttemptReceipt = serde_json::from_slice(
        &std::fs::read(Path::new(details.workspace.as_ref().unwrap()).join("attempt.json"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(receipt.exit_code, 17);
    assert_eq!(
        receipt.stderr_log.unwrap().path,
        details.stderr_log.unwrap().path
    );
}

struct UnavailableLogs(LocalDirectoryArtifactStore);

#[tokio::test]
async fn observed_empty_stderr_is_not_reported_as_unavailable() {
    let temp = tempfile::tempdir().unwrap();
    let executor = RemoteTaskExecutor::new(
        Arc::new(ProcessTaskTransport::new(temp.path().join("workers"))),
        Arc::new(LocalDirectoryArtifactStore::new(
            temp.path().join("artifacts"),
        )),
        temp.path().join("coordinator"),
    );
    let input = temp.path().join("input");
    std::fs::write(&input, b"input").unwrap();
    let mut invocation = execution(&input);
    invocation.submission.spec.outputs.clear();
    invocation.submission.spec.spec = serde_json::json!({"script":"exit 17"});
    let JobResult::Failed {
        error,
        details: Some(details),
        ..
    } = executor.run(invocation).await
    else {
        panic!("expected failure")
    };
    assert_eq!(details.exit_code, Some(17));
    assert!(error.to_string().contains("stderr is empty"), "{error}");
    assert!(!error.to_string().contains("unavailable"));
}

#[async_trait::async_trait]
impl TaskArtifactStore for UnavailableLogs {
    async fn upload(&self, file: FileRef, task: &str, name: &str) -> Result<FileRef, DagError> {
        self.0.upload(file, task, name).await
    }
    async fn download(&self, _: FileRef, _: &str, _: &str) -> Result<FileRef, DagError> {
        Err(DagError::Schedule(
            "synthetic log transfer unavailable".into(),
        ))
    }
}

#[tokio::test]
async fn log_transfer_failure_does_not_mask_the_worker_exit() {
    let temp = tempfile::tempdir().unwrap();
    let executor = RemoteTaskExecutor::new(
        Arc::new(ProcessTaskTransport::new(temp.path().join("workers"))),
        Arc::new(UnavailableLogs(LocalDirectoryArtifactStore::new(
            temp.path().join("artifacts"),
        ))),
        temp.path().join("coordinator"),
    );
    let input = temp.path().join("input");
    std::fs::write(&input, b"input").unwrap();
    let mut invocation = execution(&input);
    invocation.submission.spec.outputs.clear();
    invocation.submission.spec.spec =
        serde_json::json!({"script":"printf 'worker failure' >&2; exit 17"});
    let JobResult::Failed {
        error,
        details: Some(details),
        ..
    } = executor.run(invocation).await
    else {
        panic!("expected failure")
    };
    assert_eq!(details.exit_code, Some(17));
    assert!(error.to_string().contains("stderr unavailable"));
    assert!(
        std::fs::read_to_string(&details.stderr_log.unwrap().path)
            .unwrap()
            .contains("synthetic log transfer unavailable")
    );
}

#[tokio::test]
async fn large_process_stderr_keeps_the_final_diagnostic_and_the_raw_log() {
    let temp = tempfile::tempdir().unwrap();
    let executor = RemoteTaskExecutor::new(
        Arc::new(ProcessTaskTransport::new(temp.path().join("workers"))),
        Arc::new(LocalDirectoryArtifactStore::new(
            temp.path().join("artifacts"),
        )),
        temp.path().join("coordinator"),
    );
    let input = temp.path().join("input");
    std::fs::write(&input, b"input").unwrap();
    let mut invocation = execution(&input);
    invocation.submission.spec.outputs.clear();
    invocation.submission.spec.spec = serde_json::json!({"script":r"head -c 200000 /dev/zero | tr '\0' x >&2; printf '\nfinal process diagnostic\n' >&2; exit 17"});
    let JobResult::Failed {
        error,
        details: Some(details),
        ..
    } = executor.run(invocation).await
    else {
        panic!("expected failure")
    };
    assert_eq!(details.exit_code, Some(17));
    assert!(error.to_string().contains("final process diagnostic"));
    let snapshot = std::fs::read_to_string(&details.stderr_log.unwrap().path).unwrap();
    assert!(snapshot.starts_with("[truncated:"));
    assert!(snapshot.ends_with("final process diagnostic\n"));
    assert!(snapshot.len() < 66 * 1024);
    let raw = temp
        .path()
        .join("workers/same-logical-node/lease-1/stderr.log");
    assert!(std::fs::metadata(raw).unwrap().len() > 200_000);
}
