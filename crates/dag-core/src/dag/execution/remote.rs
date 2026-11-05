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
    async fn observe_logs(
        &self,
        receipt: &TaskAttemptReceipt,
        attempt_key: &str,
        reporter: &crate::dag::node_event::NodeReporter,
    ) -> (Option<String>, bool) {
        let mut details = reporter.take_run_details().unwrap_or_default();
        let mut diagnostic = None;
        let mut observed_empty_stderr = false;
        if let Some(error) = &receipt.log_capture_error {
            reporter.warn(format!("remote log collection failed: {error}"));
        }
        for (name, source) in [
            ("stdout", &receipt.stdout_log),
            ("stderr", &receipt.stderr_log),
        ] {
            let Some(source) = source else { continue };
            // Slurm supplies bounded snapshots. Reject oversized/unmeasured
            // log references instead of downloading unbounded worker output.
            if source
                .fingerprint
                .as_ref()
                .is_none_or(|fp| fp.size > 66 * 1024)
            {
                reporter.warn(format!(
                    "remote {name} log is unmeasured or exceeds the transport bound"
                ));
                continue;
            }
            match self
                .artifact_store
                .download(source.clone(), attempt_key, &format!("remote-{name}.log"))
                .await
            {
                Ok(local) => {
                    let reference =
                        match self.artifact_store.retained_reference(source.clone()).await {
                            Ok(Some(retained)) => retained,
                            _ => local.clone(),
                        };
                    if name == "stderr" {
                        details.stderr_log = Some(reference);
                        if let Ok(bytes) = std::fs::read(&local.path) {
                            let text = String::from_utf8_lossy(&bytes);
                            let tail = text
                                .chars()
                                .rev()
                                .take(4096)
                                .collect::<String>()
                                .chars()
                                .rev()
                                .collect::<String>();
                            if !tail.trim().is_empty() {
                                diagnostic = Some(tail.trim().to_owned());
                            } else {
                                observed_empty_stderr = true;
                            }
                        }
                    } else {
                        details.stdout_log = Some(reference);
                    }
                }
                Err(error) => reporter.warn(format!("cannot retrieve remote {name} log: {error}")),
            }
        }
        reporter.set_run_details(details);
        (diagnostic, observed_empty_stderr)
    }

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
                if artifact.path.starts_with("vfs://") {
                    return Ok(NodeValue::DataFrame(
                        super::retained_arrow::load_retained_arrow(artifact, engine_ctx).await?,
                    ));
                }
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
                let schema = reader.schema();
                let mut batches = reader
                    .into_iter()
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|error| {
                        DagError::Schedule(format!(
                            "cannot decode remote Arrow artifact `{}`: {error}",
                            artifact.path
                        ))
                    })?;
                // IPC files may have a schema but no record batches. Preserve
                // that schema instead of asking DataFusion to infer from [].
                if batches.is_empty() {
                    batches.push(arrow::record_batch::RecordBatch::new_empty(schema));
                }
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

    pub(super) async fn load_output(
        artifacts: &[FileRef],
        payload: &str,
        engine_ctx: &NodeCtx,
    ) -> Result<NodeValue, DagError> {
        match payload {
            "channel" => {
                use std::io::Read;
                let first = artifacts.first().ok_or_else(|| {
                    DagError::Schedule("remote channel output is missing its artifact".into())
                })?;
                let file = std::fs::File::open(&first.path).map_err(|e| {
                    DagError::Schedule(format!("cannot open remote Channel artifact: {e}"))
                })?;
                // Ordinary channel payloads are JSON arrays. Structured payloads
                // are tagged JSON objects followed by their ordered artifacts.
                let first_byte = std::io::BufReader::new(file)
                    .bytes()
                    .find(|byte| byte.as_ref().map_or(true, |b| !b.is_ascii_whitespace()))
                    .transpose()
                    .map_err(|e| {
                        DagError::Schedule(format!("cannot read remote Channel artifact: {e}"))
                    })?;
                if first_byte == Some(b'{') {
                    Box::pin(Self::load_output(
                        artifacts,
                        "structured_channel",
                        engine_ctx,
                    ))
                    .await
                } else {
                    let [artifact] = artifacts else {
                        return Err(DagError::Schedule(format!(
                            "remote channel output expects one artifact, got {}",
                            artifacts.len()
                        )));
                    };
                    Self::load_artifact(artifact, "channel", engine_ctx).await
                }
            }
            "structured_channel" => {
                use std::io::Read;
                const MAX_BYTES: u64 = 16 * 1024 * 1024;
                let (manifest, artifacts) = artifacts.split_first().ok_or_else(|| {
                    DagError::Schedule("structured channel input is missing its manifest".into())
                })?;
                let file = std::fs::File::open(&manifest.path).map_err(|e| {
                    DagError::Schedule(format!("cannot open structured channel manifest: {e}"))
                })?;
                let mut bytes = Vec::new();
                file.take(MAX_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| {
                        DagError::Schedule(format!("cannot read structured channel manifest: {e}"))
                    })?;
                let mut value =
                    super::ChannelTransportValue::decode(&bytes, MAX_BYTES as usize, 64, 100_000)
                        .map_err(DagError::Schedule)?;
                value
                    .replace_artifacts(artifacts.to_vec())
                    .map_err(DagError::Schedule)?;
                Ok(NodeValue::Structured(value))
            }
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
        if let Err(error) = self.transport.ensure_ready() {
            return JobResult::Failed {
                id: task_id,
                error,
                duration: start.elapsed(),
                details: execution.reporter.take_run_details(),
            };
        }
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
        let attempt_key = workspace.attempt_key();
        let mut staged_submission = execution.submission.clone();
        execution
            .reporter
            .phase("staging_inputs", "preparing remote task inputs");
        let staged = workspace
            .stage_inputs_with_remote_observed(
                &mut staged_submission,
                execution.engine_ctx.opendal.as_deref(),
                Some(self.artifact_store.as_ref()),
                Some(&execution.reporter),
            )
            .await
            .and_then(|remote_inputs| {
                let mut dispatch = staged_submission.into_remote_dispatch()?;
                if dispatch.spec.kind == "process" {
                    let process = dispatch.spec.spec.as_object_mut().ok_or_else(|| {
                        DagError::Schedule("process specification must be an object".into())
                    })?;
                    process.insert(
                        "_autonomics_attempt_key".into(),
                        serde_json::Value::String(attempt_key.clone()),
                    );
                }
                Ok((dispatch, remote_inputs))
            });

        let (dispatch, remote_inputs) = match staged {
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
        let optional_output_ports = if dispatch.spec.kind == "process" {
            super::process::parse_process_task(&dispatch)
                .ok()
                .map(|process| {
                    process
                        .outputs
                        .into_iter()
                        .filter(|output| output.optional)
                        .map(|output| output.port)
                        .collect::<std::collections::BTreeSet<_>>()
                })
                .unwrap_or_default()
        } else {
            Default::default()
        };
        let timeout = task_timeout(&dispatch.resources);
        let submission_result = async {
            let mut uploaded_inputs = Vec::new();
            for (port, input) in dispatch.spec.inputs.iter().enumerate() {
                for (index, path) in input.staged_paths.iter().enumerate() {
                    if let Some(remote) = remote_inputs.get(&(input.port, index)) {
                        uploaded_inputs.push(remote.clone());
                        continue;
                    }
                    let artifact = Self::local_artifact(path)?;
                    let basename = Path::new(path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("input");
                    let name = if matches!(input.source, super::TaskInputSource::SpecFile { .. }) {
                        format!(
                            "input-{port}-{index}-{}",
                            super::process::sanitize_path_component(basename)
                        )
                    } else {
                        format!("input-{port}-{index}")
                    };
                    let uploaded = self
                        .artifact_store
                        .upload_observed(artifact, &attempt_key, &name, &execution.reporter)
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
            receipt = self.transport.wait_observed(&lease, &execution.reporter) => WaitOutcome::Receipt(receipt),
            _ = cancellation.cancelled() => WaitOutcome::Cancelled,
            _ = tokio::time::sleep(timeout.unwrap_or(Duration::MAX)), if timeout.is_some() => WaitOutcome::TimedOut,
        };

        let receipt = match outcome {
            WaitOutcome::Receipt(Ok(receipt)) => receipt,
            WaitOutcome::Receipt(Err(error)) => {
                if let Err(cancel_error) = self.transport.cancel(&lease).await {
                    // An unconfirmed cancel must surface in logs: the remote
                    // job may still be running, so nothing may clean its
                    // workspace.
                    tracing::warn!(
                        task = %task_id,
                        lease = %lease.lease_id,
                        error = %cancel_error,
                        "remote task failed and its cancel could not be confirmed"
                    );
                }
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
                if let Err(cancel_error) = self.transport.cancel(&lease).await {
                    tracing::warn!(
                        task = %task_id,
                        lease = %lease.lease_id,
                        error = %cancel_error,
                        "remote task cancelled but its stop could not be confirmed; retaining remote workspace"
                    );
                }
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
                if let Err(cancel_error) = self.transport.cancel(&lease).await {
                    tracing::warn!(
                        task = %task_id,
                        lease = %lease.lease_id,
                        error = %cancel_error,
                        "remote task timed out and its cancel could not be confirmed; retaining remote workspace"
                    );
                }
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

        let (diagnostic, observed_empty_stderr) = self
            .observe_logs(&receipt, &attempt_key, &execution.reporter)
            .await;
        if receipt.status != "success" {
            let error = DagError::Schedule(format!(
                "remote task failed with status `{}` and exit code `{}`; {}",
                receipt.status,
                receipt.exit_code,
                diagnostic
                    .map(|text| format!("stderr: {text}"))
                    .unwrap_or_else(|| {
                        if let Some(error) = &receipt.log_capture_error {
                            format!("stderr unavailable: log collection failed: {error}")
                        } else if observed_empty_stderr {
                            "stderr is empty".into()
                        } else {
                            "stderr unavailable; inspect the attempt logs".into()
                        }
                    })
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
                .download_observed(
                    receipt.task_manifest.clone(),
                    &attempt_key,
                    "task.json",
                    &execution.reporter,
                )
                .await?;
            let mut local_outputs = BTreeMap::<u8, Vec<FileRef>>::new();
            for (port, artifacts) in &remote_outputs {
                let mut downloaded = Vec::with_capacity(artifacts.len());
                for (index, artifact) in artifacts.iter().enumerate() {
                    let file_payload = output_contracts.iter().any(|contract| {
                        contract.port == *port
                            && matches!(
                                contract.payload.as_str(),
                                "file" | "file_set" | "dataframe"
                            )
                    });
                    if file_payload {
                        if let Some(reference) = self
                            .artifact_store
                            .retained_reference(artifact.clone())
                            .await?
                        {
                            downloaded.push(reference);
                            continue;
                        }
                    }
                    downloaded.push(
                        self.artifact_store
                            .download_observed(
                                artifact.clone(),
                                &attempt_key,
                                &format!("output-{port}-{index}"),
                                &execution.reporter,
                            )
                            .await?,
                    );
                }
                local_outputs.insert(*port, downloaded);
            }

            let mut outputs = PortOutputs::new();
            for contract in &output_contracts {
                if !local_outputs.contains_key(&contract.port)
                    && optional_output_ports.contains(&contract.port)
                {
                    continue;
                }
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
            if let Some(value) = execution.submission.spec.spec.get("typed_dispatch") {
                let envelope: super::typed_worker::TypedWorkerEnvelope =
                    serde_json::from_value(value.clone()).map_err(|error| {
                        DagError::Schedule(format!("invalid typed publication envelope: {error}"))
                    })?;
                local_outputs.extend(
                    super::typed_publication::publish(&envelope, &mut outputs, &engine_ctx).await?,
                );
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
                    // Publication/transfer failure is a coordinator failure,
                    // not a different exit status for the stopped worker.
                    receipt.exit_code,
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
        let run_details = details.get_or_insert_with(Default::default);
        run_details.output_artifacts = output_artifacts;
        run_details.output_artifacts_by_port = local_outputs;
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
            // The coordinator attempt contains its actual receipt and status.
            // A worker workspace is a different host namespace; its retained
            // artifacts carry their own explicit references.
            details.task_manifest = Some(remote_task_manifest);
            details.exit_code = Some(receipt.exit_code);
        }

        JobResult::Success {
            id: task_id,
            outputs,
            duration: start.elapsed(),
            details: details.take(),
        }
    }
}

#[cfg(test)]
#[path = "remote_attempt_tests.rs"]
mod tests;
