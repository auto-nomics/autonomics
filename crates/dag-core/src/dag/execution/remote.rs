//! Remote execution: the coordinator-side executor that drives a task
//! transport, plus the shared-filesystem artifact store used to move
//! inputs and outputs across the coordinator boundary.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use arrow::ipc::reader::FileReader as ArrowFileReader;

use super::contract::{TaskAttemptReceipt, TaskExecution, task_timeout};
use super::traits::{TaskArtifactStore, TaskExecutor, TaskTransport};
use super::workspace::LocalTaskWorkspace;
use crate::ChannelValue;
use crate::dag::DagError;
use crate::dag::graph::PortOutputs;
use crate::dag::node_event::JobResult;
use crate::registry::NodeCtx;
use crate::value::{FileRef, NodeValue};

/// Shared-filesystem implementation of the remote artifact transfer contract.
#[derive(Debug, Clone)]
pub struct LocalDirectoryArtifactStore {
    root: PathBuf,
}

impl LocalDirectoryArtifactStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn transfer(&self, artifact: FileRef, task_id: &str, name: &str) -> Result<FileRef, DagError> {
        let source = Path::new(&artifact.path);
        let destination = self.root.join(task_id).join(name);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                DagError::Schedule(format!(
                    "cannot create artifact transfer directory `{}`: {error}",
                    parent.display()
                ))
            })?;
        }
        std::fs::copy(source, &destination).map_err(|error| {
            DagError::Schedule(format!(
                "cannot transfer artifact `{}` -> `{}`: {error}",
                source.display(),
                destination.display()
            ))
        })?;
        let mut transferred = FileRef::local(&destination, artifact.format.clone())?;
        transferred.fingerprint = artifact.fingerprint.clone();
        Ok(transferred)
    }
}

#[async_trait::async_trait]
impl TaskArtifactStore for LocalDirectoryArtifactStore {
    async fn upload(
        &self,
        artifact: FileRef,
        task_id: &str,
        name: &str,
    ) -> Result<FileRef, DagError> {
        self.transfer(artifact, task_id, name)
    }

    async fn download(
        &self,
        artifact: FileRef,
        task_id: &str,
        name: &str,
    ) -> Result<FileRef, DagError> {
        self.transfer(artifact, task_id, name)
    }
}

/// Coordinator-side executor for remote task transports.
#[derive(Clone)]
pub struct RemoteTaskExecutor {
    transport: Arc<dyn TaskTransport>,
    artifact_store: Arc<dyn TaskArtifactStore>,
    coordinator_workspace_root: PathBuf,
}

impl RemoteTaskExecutor {
    pub fn new(
        transport: Arc<dyn TaskTransport>,
        artifact_store: Arc<dyn TaskArtifactStore>,
        coordinator_workspace_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            transport,
            artifact_store,
            coordinator_workspace_root: coordinator_workspace_root.into(),
        }
    }

    fn local_artifact(path: &str) -> Result<FileRef, DagError> {
        let format = Path::new(path)
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_string);
        FileRef::local(path, format)
    }

    async fn load_artifact(
        artifact: &FileRef,
        payload: &str,
        engine_ctx: &NodeCtx,
    ) -> Result<NodeValue, DagError> {
        match payload {
            "file" => Ok(NodeValue::File(artifact.clone())),
            "file_set" => Err(DagError::Schedule(
                "remote file-set outputs must be loaded as a group by port".into(),
            )),
            "dataframe" => {
                let file = std::fs::File::open(&artifact.path).map_err(|error| {
                    DagError::Schedule(format!(
                        "cannot open remote Arrow artifact `{}`: {error}",
                        artifact.path
                    ))
                })?;
                let reader = ArrowFileReader::try_new(file, None).map_err(|error| {
                    DagError::Schedule(format!(
                        "cannot read remote Arrow artifact `{}`: {error}",
                        artifact.path
                    ))
                })?;
                let batches = reader
                    .into_iter()
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|error| {
                        DagError::Schedule(format!(
                            "cannot decode remote Arrow artifact `{}`: {error}",
                            artifact.path
                        ))
                    })?;
                let dataframe = engine_ctx
                    .session()
                    .read_batches(batches)
                    .map_err(|error| {
                        DagError::Schedule(format!(
                            "cannot load remote Arrow artifact `{}`: {error}",
                            artifact.path
                        ))
                    })?;
                Ok(NodeValue::DataFrame(dataframe))
            }
            "channel" => {
                let file = std::fs::File::open(&artifact.path).map_err(|error| {
                    DagError::Schedule(format!(
                        "cannot open remote Channel artifact `{}`: {error}",
                        artifact.path
                    ))
                })?;
                let items = serde_json::from_reader(file).map_err(|error| {
                    DagError::Schedule(format!(
                        "cannot decode remote Channel artifact `{}`: {error}",
                        artifact.path
                    ))
                })?;
                Ok(NodeValue::Channel(crate::ChannelValue { items }))
            }
            _ => match artifact.format.as_deref() {
                Some("arrow") => {
                    Box::pin(Self::load_artifact(artifact, "dataframe", engine_ctx)).await
                }
                Some("json") => {
                    Box::pin(Self::load_artifact(artifact, "channel", engine_ctx)).await
                }
                _ => Ok(NodeValue::File(artifact.clone())),
            },
        }
    }

    async fn load_output(
        artifacts: &[FileRef],
        payload: &str,
        engine_ctx: &NodeCtx,
    ) -> Result<NodeValue, DagError> {
        match payload {
            "file" => {
                let [artifact] = artifacts else {
                    return Err(DagError::Schedule(format!(
                        "remote file output expects one artifact, got {}",
                        artifacts.len()
                    )));
                };
                Ok(NodeValue::File(artifact.clone()))
            }
            "file_set" => Ok(NodeValue::FileSet(artifacts.to_vec())),
            _ => {
                let [artifact] = artifacts else {
                    return Err(DagError::Schedule(format!(
                        "remote typed output expects one artifact, got {}",
                        artifacts.len()
                    )));
                };
                Self::load_artifact(artifact, payload, engine_ctx).await
            }
        }
    }
}

#[async_trait::async_trait]
impl TaskExecutor for RemoteTaskExecutor {
    fn name(&self) -> &'static str {
        "remote"
    }

    async fn run(&self, execution: TaskExecution) -> JobResult {
        let start = std::time::Instant::now();
        let task_id = execution.submission.spec.id.clone();
        let engine_ctx = Arc::clone(&execution.engine_ctx);
        let cancellation = execution.cancellation.clone();
        let mut workspace = match LocalTaskWorkspace::create(
            &self.coordinator_workspace_root,
            &execution.submission,
        ) {
            Ok(workspace) => workspace,
            Err(error) => {
                return JobResult::Failed {
                    id: task_id,
                    error,
                    duration: start.elapsed(),
                    details: execution.reporter.take_run_details(),
                };
            }
        };
        let mut staged_submission = execution.submission.clone();
        let dispatch = workspace
            .stage_inputs(
                &mut staged_submission,
                execution.engine_ctx.opendal.as_deref(),
            )
            .await
            .and_then(|()| staged_submission.into_remote_dispatch());

        let dispatch = match dispatch {
            Ok(dispatch) => dispatch,
            Err(error) => {
                let duration = start.elapsed();
                let mut details = execution.reporter.take_run_details();
                let logs = execution.reporter.take_logs();
                workspace.complete(
                    &task_id,
                    self.name(),
                    &mut details,
                    logs,
                    126,
                    "remote_input_staging_failed",
                    duration,
                );
                return JobResult::Failed {
                    id: task_id,
                    error,
                    duration,
                    details,
                };
            }
        };

        let output_contracts = dispatch.spec.outputs.clone();
        let timeout = task_timeout(&dispatch.resources);
        let submission_result = async {
            let mut uploaded_inputs = Vec::new();
            for (port, input) in dispatch.spec.inputs.iter().enumerate() {
                for (index, path) in input.staged_paths.iter().enumerate() {
                    let artifact = Self::local_artifact(path)?;
                    let uploaded = self
                        .artifact_store
                        .upload(artifact, &task_id, &format!("input-{port}-{index}"))
                        .await?;
                    uploaded_inputs.push(uploaded);
                }
            }

            self.transport.submit(dispatch, uploaded_inputs).await
        }
        .await;

        let lease = match submission_result {
            Ok(lease) => lease,
            Err(error) => {
                let duration = start.elapsed();
                let mut details = execution.reporter.take_run_details();
                let logs = execution.reporter.take_logs();
                workspace.complete(
                    &task_id,
                    self.name(),
                    &mut details,
                    logs,
                    127,
                    "remote_submit_failed",
                    duration,
                );
                return JobResult::Failed {
                    id: task_id,
                    error,
                    duration,
                    details,
                };
            }
        };

        enum WaitOutcome {
            Receipt(Result<TaskAttemptReceipt, DagError>),
            Cancelled,
            TimedOut,
        }

        let outcome = tokio::select! {
            receipt = self.transport.wait(&lease) => WaitOutcome::Receipt(receipt),
            _ = cancellation.cancelled() => WaitOutcome::Cancelled,
            _ = tokio::time::sleep(timeout.unwrap_or(Duration::MAX)), if timeout.is_some() => WaitOutcome::TimedOut,
        };

        let receipt = match outcome {
            WaitOutcome::Receipt(Ok(receipt)) => receipt,
            WaitOutcome::Receipt(Err(error)) => {
                let _ = self.transport.cancel(&lease).await;
                let duration = start.elapsed();
                let mut details = execution.reporter.take_run_details();
                let logs = execution.reporter.take_logs();
                workspace.complete(
                    &task_id,
                    self.name(),
                    &mut details,
                    logs,
                    125,
                    "remote_failed",
                    duration,
                );
                return JobResult::Failed {
                    id: task_id,
                    error,
                    duration,
                    details,
                };
            }
            WaitOutcome::Cancelled => {
                let _ = self.transport.cancel(&lease).await;
                let duration = start.elapsed();
                let mut details = execution.reporter.take_run_details();
                let logs = execution.reporter.take_logs();
                workspace.complete(
                    &task_id,
                    self.name(),
                    &mut details,
                    logs,
                    130,
                    "cancelled",
                    duration,
                );
                return JobResult::Failed {
                    id: task_id,
                    error: DagError::Schedule("remote task cancelled".into()),
                    duration,
                    details,
                };
            }
            WaitOutcome::TimedOut => {
                let _ = self.transport.cancel(&lease).await;
                let duration = start.elapsed();
                let mut details = execution.reporter.take_run_details();
                let logs = execution.reporter.take_logs();
                workspace.complete(
                    &task_id,
                    self.name(),
                    &mut details,
                    logs,
                    124,
                    "timeout",
                    duration,
                );
                return JobResult::Failed {
                    id: task_id,
                    error: DagError::Schedule("remote task timed out".into()),
                    duration,
                    details,
                };
            }
        };
        if receipt.task_id != task_id {
            let error = DagError::Schedule(format!(
                "remote receipt task `{}` does not match submitted task `{task_id}`",
                receipt.task_id
            ));
            let duration = start.elapsed();
            let mut details = execution.reporter.take_run_details();
            let logs = execution.reporter.take_logs();
            workspace.complete(
                &task_id,
                self.name(),
                &mut details,
                logs,
                125,
                "remote_receipt_mismatch",
                duration,
            );
            return JobResult::Failed {
                id: task_id,
                error,
                duration,
                details,
            };
        }

        if receipt.status != "success" {
            let error = DagError::Schedule(format!(
                "remote task failed with status `{}` and exit code `{}`",
                receipt.status, receipt.exit_code
            ));
            let duration = start.elapsed();
            let mut details = execution.reporter.take_run_details();
            let logs = execution.reporter.take_logs();
            workspace.complete(
                &task_id,
                self.name(),
                &mut details,
                logs,
                receipt.exit_code,
                "remote_failed",
                duration,
            );
            return JobResult::Failed {
                id: task_id,
                error,
                duration,
                details,
            };
        }

        let remote_outputs = if !receipt.output_artifacts_by_port.is_empty() {
            receipt.output_artifacts_by_port.clone()
        } else if receipt.output_artifacts.len() == 1 && output_contracts.len() == 1 {
            BTreeMap::from([(output_contracts[0].port, receipt.output_artifacts.clone())])
        } else if receipt.output_artifacts.is_empty() {
            BTreeMap::new()
        } else {
            let error = DagError::Schedule(format!(
                "remote receipt for task `{task_id}` does not group outputs by port"
            ));
            let duration = start.elapsed();
            let mut details = execution.reporter.take_run_details();
            let logs = execution.reporter.take_logs();
            workspace.complete(
                &task_id,
                self.name(),
                &mut details,
                logs,
                125,
                "remote_receipt_invalid",
                duration,
            );
            return JobResult::Failed {
                id: task_id,
                error,
                duration,
                details,
            };
        };

        let download = async {
            let task_manifest = self
                .artifact_store
                .download(receipt.task_manifest.clone(), &task_id, "task.json")
                .await?;
            let mut local_outputs = BTreeMap::<u8, Vec<FileRef>>::new();
            for (port, artifacts) in &remote_outputs {
                let mut downloaded = Vec::with_capacity(artifacts.len());
                for (index, artifact) in artifacts.iter().enumerate() {
                    downloaded.push(
                        self.artifact_store
                            .download(
                                artifact.clone(),
                                &task_id,
                                &format!("output-{port}-{index}"),
                            )
                            .await?,
                    );
                }
                local_outputs.insert(*port, downloaded);
            }

            let mut outputs = PortOutputs::new();
            for contract in &output_contracts {
                let artifacts = local_outputs
                    .get(&contract.port)
                    .ok_or_else(|| {
                        DagError::Schedule(format!(
                            "remote receipt lacks artifacts for output `{}` on port {}",
                            contract.name, contract.port
                        ))
                    })?
                    .clone();
                let value =
                    Self::load_output(&artifacts, &contract.payload, engine_ctx.as_ref()).await?;
                outputs.insert(contract.port, value);
            }
            Ok::<_, DagError>((task_manifest, local_outputs, outputs))
        }
        .await;

        let (remote_task_manifest, local_outputs, outputs) = match download {
            Ok(downloaded) => downloaded,
            Err(error) => {
                let duration = start.elapsed();
                let mut details = execution.reporter.take_run_details();
                let logs = execution.reporter.take_logs();
                workspace.complete(
                    &task_id,
                    self.name(),
                    &mut details,
                    logs,
                    127,
                    "remote_output_download_failed",
                    duration,
                );
                return JobResult::Failed {
                    id: task_id,
                    error,
                    duration,
                    details,
                };
            }
        };

        let output_artifacts = local_outputs
            .values()
            .flat_map(|artifacts| artifacts.iter().cloned())
            .collect::<Vec<_>>();
        let mut details = execution.reporter.take_run_details();
        let logs = execution.reporter.take_logs();
        workspace.complete(
            &task_id,
            self.name(),
            &mut details,
            logs,
            receipt.exit_code,
            "success",
            Duration::from_millis(receipt.elapsed_ms),
        );
        details.get_or_insert_with(Default::default);
        if let Some(details) = details.as_mut() {
            details.workspace = Some(receipt.workspace.clone());
            details.task_manifest = Some(remote_task_manifest);
            details.exit_code = Some(receipt.exit_code);
            details.output_artifacts = output_artifacts;
            details.output_artifacts_by_port = local_outputs;
        }

        JobResult::Success {
            id: task_id,
            outputs,
            duration: start.elapsed(),
            details: details.take(),
        }
    }
}
