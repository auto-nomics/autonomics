//! Executor-facing task contracts.
//!
//! The scheduler currently dispatches every task through
//! [`LocalTaskExecutor`]. The trait boundary keeps that path replaceable: a
//! future SLURM, Kubernetes, or batch executor can consume the same task
//! identity, process inputs, resources, and terminal result protocol.

use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures::FutureExt;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::node_event::{EventLevel, JobResult, NodeReporter, ReportedLog};
use super::{DagNode, NodeInput};
use crate::dag::DagError;
use crate::dag::graph::PortOutputs;
use crate::registry::NodeCtx;
use crate::value::{FileRef, NodeValue};
use arrow::ipc::writer::FileWriter as ArrowFileWriter;

/// Resource requests associated with one task.
///
/// These are executor directives. The local executor enforces duration,
/// weighted CPU/memory request scheduling, and task workspaces.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskResources {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpus: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_duration_ms: Option<u64>,
}

impl TaskResources {
    /// Reject requests that would be silently meaningless at execution time.
    pub fn validate(&self) -> Result<(), DagError> {
        if self.cpus == Some(0) {
            return Err(DagError::Schedule(
                "task resource `cpus` must be greater than zero".into(),
            ));
        }
        if self.memory_bytes == Some(0) {
            return Err(DagError::Schedule(
                "task resource `memory_bytes` must be greater than zero".into(),
            ));
        }
        if self.max_duration_ms == Some(0) {
            return Err(DagError::Schedule(
                "task resource `max_duration_ms` must be greater than zero".into(),
            ));
        }
        Ok(())
    }
}

/// Stable identity and process-input description for one dispatched task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSpec {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logical_node: Option<String>,
    pub kind: String,
    pub spec: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub axis: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<serde_json::Value>,
    pub inputs: Vec<TaskInputBinding>,
    pub outputs: Vec<TaskOutputBinding>,
}

/// Where a process input came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskInputSource {
    UpstreamPort { from: String, from_port: u8 },
    ScatterItem { axis: String },
    DynamicFanoutItem { axis: String },
}

/// One named process input binding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskInputBinding {
    pub name: String,
    pub port: u8,
    pub source: TaskInputSource,
    pub payload: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<crate::value::FileFingerprint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub staged_paths: Vec<String>,
}

/// One named process output contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskOutputBinding {
    pub name: String,
    pub port: u8,
    pub payload: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_paths: Vec<String>,
}

/// Everything an executor needs for one attempt.
#[derive(Clone)]
pub struct TaskSubmission {
    pub spec: TaskSpec,
    pub inputs: Vec<NodeInput>,
    pub resources: TaskResources,
}

impl TaskSubmission {
    /// Build the serializable envelope consumed by a remote executor.
    ///
    /// Remote execution rejects unstaged in-memory inputs: once this method
    /// succeeds, every DataFrame/File/FileSet/Channel input has a path in
    /// `TaskSpec::inputs` and can be transferred without process memory.
    pub fn into_remote_dispatch(self) -> Result<TaskDispatch, DagError> {
        for input in &self.spec.inputs {
            if input.staged_paths.is_empty() {
                return Err(DagError::Schedule(format!(
                    "remote dispatch requires a staged artifact for task `{}` input `{}`",
                    self.spec.id, input.name
                )));
            }
        }
        Ok(TaskDispatch {
            spec: self.spec,
            resources: self.resources,
        })
    }
}

/// Serializable, memory-independent work order for a remote executor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskDispatch {
    pub spec: TaskSpec,
    pub resources: TaskResources,
}

/// An executor invocation, including the local node payload.
#[derive(Clone)]
pub struct TaskExecution {
    pub submission: TaskSubmission,
    pub node: Box<dyn DagNode>,
    pub engine_ctx: Arc<NodeCtx>,
    pub reporter: NodeReporter,
    pub cancellation: CancellationToken,
}

/// Serializable terminal evidence for one executor attempt.
///
/// A remote executor writes this receipt next to its task artifacts. The
/// coordinating executor uses it to distinguish an actually finished task from
/// a lost lease and to locate every output that must be transferred back.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskAttemptReceipt {
    pub task_id: String,
    pub executor: String,
    pub status: String,
    pub elapsed_ms: u64,
    pub exit_code: i32,
    pub workspace: String,
    pub task_manifest: FileRef,
    /// File-backed outputs that must be transferred back to the coordinator.
    pub output_artifacts: Vec<FileRef>,
}

/// Execution backend for physical DAG tasks.
#[async_trait::async_trait]
pub trait TaskExecutor: Send + Sync {
    fn name(&self) -> &'static str;

    async fn run(&self, execution: TaskExecution) -> JobResult;
}

/// Default in-process executor used by the current scheduler.
#[derive(Debug, Clone)]
pub struct LocalTaskExecutor {
    workspace_root: PathBuf,
    resource_budget: LocalResourceBudget,
    stage_inputs: bool,
}

impl Default for LocalTaskExecutor {
    fn default() -> Self {
        Self {
            workspace_root: std::env::temp_dir().join("autonomics-dag-tasks"),
            resource_budget: Self::discover_resource_budget(),
            stage_inputs: false,
        }
    }
}

impl LocalTaskExecutor {
    pub fn with_workspace_root(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            resource_budget: Self::discover_resource_budget(),
            stage_inputs: false,
        }
    }

    pub fn with_workspace_root_and_resource_limits(
        workspace_root: impl Into<PathBuf>,
        cpu_limit: Option<u32>,
        memory_limit_bytes: Option<u64>,
    ) -> Result<Self, DagError> {
        TaskResources {
            cpus: cpu_limit,
            memory_bytes: memory_limit_bytes,
            max_duration_ms: None,
        }
        .validate()?;
        Ok(Self {
            workspace_root: workspace_root.into(),
            resource_budget: LocalResourceBudget::new(cpu_limit, memory_limit_bytes),
            stage_inputs: false,
        })
    }

    pub fn with_workspace_root_resource_limits_and_input_staging(
        workspace_root: impl Into<PathBuf>,
        cpu_limit: Option<u32>,
        memory_limit_bytes: Option<u64>,
    ) -> Result<Self, DagError> {
        let mut executor = Self::with_workspace_root_and_resource_limits(
            workspace_root,
            cpu_limit,
            memory_limit_bytes,
        )?;
        executor.stage_inputs = true;
        Ok(executor)
    }

    fn discover_resource_budget() -> LocalResourceBudget {
        let cpus = std::thread::available_parallelism()
            .map(|cpus| cpus.get() as u32)
            .ok();
        let memory_bytes = crate::resource::sample_memory_usage()
            .ok()
            .map(|sample| sample.limit_bytes);
        LocalResourceBudget::new(cpus, memory_bytes)
    }
}

#[derive(Debug)]
struct LocalResourceState {
    cpu_limit: Option<u32>,
    memory_limit_bytes: Option<u64>,
    cpu_used: u64,
    memory_used_bytes: u64,
    generation: u64,
}

#[derive(Debug)]
struct LocalResourceBudget {
    state: std::sync::Arc<(std::sync::Mutex<LocalResourceState>, Notify)>,
}

impl Clone for LocalResourceBudget {
    fn clone(&self) -> Self {
        Self {
            state: std::sync::Arc::clone(&self.state),
        }
    }
}

impl LocalResourceBudget {
    fn new(cpu_limit: Option<u32>, memory_limit_bytes: Option<u64>) -> Self {
        Self {
            state: std::sync::Arc::new((
                std::sync::Mutex::new(LocalResourceState {
                    cpu_limit,
                    memory_limit_bytes,
                    cpu_used: 0,
                    memory_used_bytes: 0,
                    generation: 0,
                }),
                Notify::new(),
            )),
        }
    }

    fn fits(request: u64, used: u64, limit: Option<u64>) -> Result<bool, DagError> {
        let Some(limit) = limit else {
            return Ok(true);
        };
        let limit = limit as u64;
        if request > limit {
            return Err(DagError::Schedule(format!(
                "task requests {request} but the local executor limit is {limit}"
            )));
        }
        Ok(request.saturating_add(used) <= limit)
    }

    async fn acquire(&self, request: &TaskResources) -> Result<LocalResourceLease, DagError> {
        let cpus = request.cpus.map(u64::from).unwrap_or(0);
        let memory_bytes = request.memory_bytes.unwrap_or(0);
        loop {
            let observed_generation = {
                let mut state = self.state.0.lock().unwrap();
                let cpu_available =
                    Self::fits(cpus, state.cpu_used, state.cpu_limit.map(u64::from))?;
                let memory_available = Self::fits(
                    memory_bytes,
                    state.memory_used_bytes,
                    state.memory_limit_bytes,
                )?;
                if cpu_available && memory_available {
                    state.cpu_used += cpus;
                    state.memory_used_bytes += memory_bytes;
                    let generation = state.generation;
                    return Ok(LocalResourceLease {
                        state: std::sync::Arc::clone(&self.state),
                        cpus,
                        memory_bytes,
                        generation,
                    });
                }
                state.generation
            };

            let notified = self.state.1.notified();
            let unchanged = {
                let state = self.state.0.lock().unwrap();
                state.generation == observed_generation
            };
            if unchanged {
                notified.await;
            }
        }
    }
}

struct LocalResourceLease {
    state: std::sync::Arc<(std::sync::Mutex<LocalResourceState>, Notify)>,
    cpus: u64,
    memory_bytes: u64,
    #[allow(dead_code)]
    generation: u64,
}

impl Drop for LocalResourceLease {
    fn drop(&mut self) {
        {
            let mut state = self.state.0.lock().unwrap();
            state.cpu_used = state.cpu_used.saturating_sub(self.cpus);
            state.memory_used_bytes = state.memory_used_bytes.saturating_sub(self.memory_bytes);
            state.generation += 1;
        }
        self.state.1.notify_waiters();
    }
}

struct LocalTaskWorkspace {
    root: PathBuf,
    manifest: FileRef,
}

impl LocalTaskWorkspace {
    fn create(root: &std::path::Path, submission: &TaskSubmission) -> Result<Self, DagError> {
        std::fs::create_dir_all(root).map_err(|error| {
            DagError::Schedule(format!(
                "cannot create local task workspace root `{}`: {error}",
                root.display()
            ))
        })?;

        static WORKSPACE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let sequence = WORKSPACE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let workspace_root = root.join(format!("{}-{timestamp}-{sequence}", std::process::id()));
        match std::fs::create_dir(&workspace_root) {
            Ok(()) => {}
            Err(error) => {
                return Err(DagError::Schedule(format!(
                    "cannot create local task workspace `{}`: {error}",
                    workspace_root.display()
                )));
            }
        }

        let manifest = Self::write_manifest(&workspace_root, submission)?;
        Ok(Self {
            root: workspace_root,
            manifest,
        })
    }

    fn write_manifest(
        workspace_root: &std::path::Path,
        submission: &TaskSubmission,
    ) -> Result<FileRef, DagError> {
        let manifest_path = workspace_root.join("task.json");
        let manifest_json = serde_json::to_vec_pretty(&serde_json::json!({
            "spec": submission.spec,
            "resources": submission.resources,
        }))
        .map_err(|error| {
            DagError::Schedule(format!("cannot serialize local task manifest: {error}"))
        })?;
        std::fs::write(&manifest_path, manifest_json).map_err(|error| {
            DagError::Schedule(format!(
                "cannot write local task manifest `{}`: {error}",
                manifest_path.display()
            ))
        })?;
        FileRef::local(&manifest_path, Some("json".into()))
    }

    async fn write_dataframe(
        path: &std::path::Path,
        dataframe: &datafusion::prelude::DataFrame,
    ) -> Result<FileRef, DagError> {
        let schema = dataframe.schema();
        let batches = dataframe.clone().collect().await.map_err(|error| {
            DagError::Schedule(format!("cannot materialize DataFrame output: {error}"))
        })?;
        let file = std::fs::File::create(path).map_err(|error| {
            DagError::Schedule(format!(
                "cannot create Arrow IPC artifact `{}`: {error}",
                path.display()
            ))
        })?;
        let mut writer = ArrowFileWriter::try_new(file, schema.as_ref()).map_err(|error| {
            DagError::Schedule(format!(
                "cannot initialize Arrow IPC artifact `{}`: {error}",
                path.display()
            ))
        })?;
        for batch in &batches {
            writer.write(batch).map_err(|error| {
                DagError::Schedule(format!(
                    "cannot write Arrow IPC artifact `{}`: {error}",
                    path.display()
                ))
            })?;
        }
        writer.finish().map_err(|error| {
            DagError::Schedule(format!(
                "cannot finalize Arrow IPC artifact `{}`: {error}",
                path.display()
            ))
        })?;
        FileRef::local(path, Some("arrow".into()))
    }

    async fn read_source(
        path: &str,
        storage: Option<&vfs::OpendalFileStorage>,
    ) -> Result<Vec<u8>, DagError> {
        if let Some(virtual_path) = path.strip_prefix("vfs://") {
            let storage = storage.ok_or_else(|| {
                DagError::Schedule(format!(
                    "cannot stage VFS input `{path}` because no object store is configured"
                ))
            })?;
            let length = storage
                .content_length(virtual_path)
                .await
                .map_err(|error| {
                    DagError::Schedule(format!("cannot stat VFS input `{path}`: {error}"))
                })?;
            let bytes = storage
                .read_range(virtual_path, 0..length)
                .await
                .map_err(|error| {
                    DagError::Schedule(format!("cannot read VFS input `{path}`: {error}"))
                })?;
            return Ok(bytes.to_vec());
        }

        let local_path = path.strip_prefix("file://").unwrap_or(path);
        std::fs::read(local_path).map_err(|error| {
            DagError::Schedule(format!("cannot stage local input `{local_path}`: {error}"))
        })
    }

    async fn stage_file(
        input_root: &std::path::Path,
        sequence: &mut u64,
        source: &FileRef,
        storage: Option<&vfs::OpendalFileStorage>,
    ) -> Result<FileRef, DagError> {
        let bytes = Self::read_source(&source.path, storage).await?;
        let staged_path = input_root.join(format!("input-{}.bin", *sequence));
        *sequence += 1;
        std::fs::write(&staged_path, bytes).map_err(|error| {
            DagError::Schedule(format!(
                "cannot write staged input `{}`: {error}",
                staged_path.display()
            ))
        })?;
        let mut staged = FileRef::local(&staged_path, source.format.clone())?;
        if source.fingerprint.is_some() {
            staged.fingerprint = source.fingerprint.clone();
        }
        Ok(staged)
    }

    fn write_json_artifact(
        path: &std::path::Path,
        value: &serde_json::Value,
    ) -> Result<FileRef, DagError> {
        let bytes = serde_json::to_vec_pretty(value).map_err(|error| {
            DagError::Schedule(format!("cannot serialize JSON artifact: {error}"))
        })?;
        std::fs::write(path, bytes).map_err(|error| {
            DagError::Schedule(format!(
                "cannot write JSON artifact `{}`: {error}",
                path.display()
            ))
        })?;
        FileRef::local(path, Some("json".into()))
    }

    async fn stage_inputs(
        &mut self,
        submission: &mut TaskSubmission,
        storage: Option<&vfs::OpendalFileStorage>,
    ) -> Result<(), DagError> {
        let input_root = self.root.join("inputs");
        std::fs::create_dir_all(&input_root).map_err(|error| {
            DagError::Schedule(format!(
                "cannot create input staging directory `{}`: {error}",
                input_root.display()
            ))
        })?;
        let mut sequence = 0u64;
        let mut staged_by_port = std::collections::BTreeMap::<u8, Vec<String>>::new();

        for input in &mut submission.inputs {
            if let NodeValue::DataFrame(dataframe) = &input.data {
                let path = input_root.join(format!("input-{sequence}.arrow"));
                sequence += 1;
                let artifact = Self::write_dataframe(&path, dataframe).await?;
                staged_by_port
                    .entry(input.port)
                    .or_default()
                    .push(artifact.path.clone());
                continue;
            }
            if let NodeValue::Channel(channel) = &input.data {
                let path = input_root.join(format!("input-{sequence}.json"));
                sequence += 1;
                let artifact = Self::write_json_artifact(&path, &serde_json::json!(channel.items))?;
                staged_by_port
                    .entry(input.port)
                    .or_default()
                    .push(artifact.path.clone());
                continue;
            }
            let (sources, is_file_set) = match &input.data {
                NodeValue::File(file) => (vec![file.clone()], false),
                NodeValue::FileSet(files) => (files.clone(), true),
                NodeValue::DataFrame(_) | NodeValue::Channel(_) => continue,
            };
            let mut staged_files = Vec::with_capacity(sources.len());
            for source in &sources {
                let staged = Self::stage_file(&input_root, &mut sequence, source, storage).await?;
                staged_by_port
                    .entry(input.port)
                    .or_default()
                    .push(staged.path.clone());
                staged_files.push(staged);
            }
            input.data = if is_file_set {
                NodeValue::FileSet(staged_files)
            } else {
                NodeValue::File(
                    staged_files
                        .into_iter()
                        .next()
                        .ok_or_else(|| DagError::Schedule("file input had no source".into()))?,
                )
            };
        }
        for binding in &mut submission.spec.inputs {
            if let Some(paths) = staged_by_port.get(&binding.port) {
                binding.staged_paths = paths.clone();
            }
        }
        self.manifest = Self::write_manifest(&self.root, submission)?;
        Ok(())
    }

    async fn publish_outputs(
        &mut self,
        submission: &mut TaskSubmission,
        outputs: &PortOutputs,
    ) -> Result<Vec<FileRef>, DagError> {
        let output_root = self.root.join("outputs");
        std::fs::create_dir_all(&output_root).map_err(|error| {
            DagError::Schedule(format!(
                "cannot create output artifact directory `{}`: {error}",
                output_root.display()
            ))
        })?;
        let mut artifacts_by_port = std::collections::BTreeMap::<u8, Vec<String>>::new();
        let mut artifacts = Vec::new();
        let mut ports = outputs.iter().map(|(port, _)| *port).collect::<Vec<_>>();
        ports.sort_unstable();
        ports.dedup();

        for port in ports {
            let value = outputs
                .get(&port)
                .ok_or_else(|| DagError::Schedule(format!("output port {port} disappeared")))?;
            let mut port_artifacts = Vec::new();
            match value {
                NodeValue::DataFrame(dataframe) => {
                    let path = output_root.join(format!("port-{port}.arrow"));
                    let artifact = Self::write_dataframe(&path, dataframe).await?;
                    port_artifacts.push(artifact);
                }
                NodeValue::File(file) => port_artifacts.push(file.clone()),
                NodeValue::FileSet(files) => port_artifacts.extend(files.iter().cloned()),
                NodeValue::Channel(channel) => {
                    let path = output_root.join(format!("port-{port}.json"));
                    let artifact =
                        Self::write_json_artifact(&path, &serde_json::json!(channel.items))?;
                    port_artifacts.push(artifact);
                }
            }
            artifacts_by_port.insert(
                port,
                port_artifacts
                    .iter()
                    .map(|artifact| artifact.path.clone())
                    .collect::<Vec<_>>(),
            );
            artifacts.extend(port_artifacts);
        }
        for output in &mut submission.spec.outputs {
            if let Some(paths) = artifacts_by_port.get(&output.port) {
                output.artifact_paths = paths.clone();
            }
        }
        self.manifest = Self::write_manifest(&self.root, submission)?;
        Ok(artifacts)
    }

    fn capture_logs(&self, logs: Vec<ReportedLog>) -> (Option<FileRef>, Option<FileRef>) {
        let write = |name: &str, entries: Vec<&ReportedLog>| -> Option<FileRef> {
            if entries.is_empty() {
                return None;
            }
            let path = self.root.join(name);
            let rendered = entries
                .iter()
                .map(|entry| {
                    format!(
                        "{} {}",
                        serde_json::to_string(&entry.level).unwrap_or_default(),
                        entry.message
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            std::fs::write(path, format!("{rendered}\n")).ok()?;
            FileRef::local(self.root.join(name), Some("text".into())).ok()
        };
        let stdout = write(
            "stdout.log",
            logs.iter()
                .filter(|entry| !matches!(entry.level, EventLevel::Warn | EventLevel::Error))
                .collect(),
        );
        let stderr = write(
            "stderr.log",
            logs.iter()
                .filter(|entry| matches!(entry.level, EventLevel::Warn | EventLevel::Error))
                .collect(),
        );
        (stdout, stderr)
    }

    fn complete(
        &self,
        task_id: &str,
        executor: &str,
        details: &mut Option<super::runtime::NodeRunDetails>,
        logs: Vec<ReportedLog>,
        exit_code: i32,
        status: &str,
        duration: Duration,
    ) {
        let (stdout_log, stderr_log) = self.capture_logs(logs);
        let details = details.get_or_insert_with(Default::default);
        if details.stdout_log.is_none() {
            details.stdout_log = stdout_log;
        }
        if details.stderr_log.is_none() {
            details.stderr_log = stderr_log;
        }
        details.workspace = Some(self.root.to_string_lossy().into_owned());
        details.task_manifest = Some(self.manifest.clone());
        if details.exit_code.is_none() {
            details.exit_code = Some(exit_code);
        }
        self.finish(task_id, executor, details, status, duration);
    }

    fn finish(
        &self,
        task_id: &str,
        executor: &str,
        details: &super::runtime::NodeRunDetails,
        status: &str,
        duration: Duration,
    ) {
        let status_json = serde_json::json!({
            "status": status,
            "elapsed_ms": duration.as_millis().min(u64::MAX as u128) as u64,
        });
        let write_result = std::fs::write(
            self.root.join("status.json"),
            serde_json::to_vec_pretty(&status_json).unwrap_or_default(),
        );
        if let Err(error) = write_result {
            warn!(
                workspace = %self.root.display(),
                error = %error,
                "cannot write local task status"
            );
        }

        let receipt = TaskAttemptReceipt {
            task_id: task_id.to_string(),
            executor: executor.to_string(),
            status: status.to_string(),
            elapsed_ms: duration.as_millis().min(u64::MAX as u128) as u64,
            exit_code: details
                .exit_code
                .unwrap_or(if status == "success" { 0 } else { 1 }),
            workspace: self.root.to_string_lossy().into_owned(),
            task_manifest: self.manifest.clone(),
            output_artifacts: details.output_artifacts.clone(),
        };
        let receipt_path = self.root.join("attempt.json");
        let write_result = serde_json::to_vec_pretty(&receipt)
            .map_err(|error| {
                DagError::Schedule(format!("cannot serialize task attempt receipt: {error}"))
            })
            .and_then(|bytes| {
                std::fs::write(&receipt_path, bytes).map_err(|error| {
                    DagError::Schedule(format!(
                        "cannot write task attempt receipt `{}`: {error}",
                        receipt_path.display()
                    ))
                })
            });
        if let Err(error) = write_result {
            warn!(
                workspace = %self.root.display(),
                error = %error,
                "cannot write local task attempt receipt"
            );
        }
    }
}

#[async_trait::async_trait]
impl TaskExecutor for LocalTaskExecutor {
    fn name(&self) -> &'static str {
        "local"
    }

    async fn run(&self, execution: TaskExecution) -> JobResult {
        let task_id = execution.submission.spec.id.clone();
        let start = std::time::Instant::now();
        let mut execution = execution;
        if let Err(error) = execution.submission.resources.validate() {
            return JobResult::Failed {
                id: task_id,
                error,
                duration: start.elapsed(),
                details: execution.reporter.take_run_details(),
            };
        }

        let timeout = task_timeout(&execution.submission.resources);
        let cancellation = execution.cancellation.clone();
        let mut workspace =
            match LocalTaskWorkspace::create(&self.workspace_root, &execution.submission) {
                Ok(workspace) => Some(workspace),
                Err(error) => {
                    return JobResult::Failed {
                        id: task_id,
                        error,
                        duration: start.elapsed(),
                        details: execution.reporter.take_run_details(),
                    };
                }
            };

        let resource_lease = tokio::select! {
            lease = self.resource_budget.acquire(&execution.submission.resources) => match lease {
                Ok(lease) => lease,
                Err(error) => {
                    let duration = start.elapsed();
                    let mut details = execution.reporter.take_run_details();
                    let logs = execution.reporter.take_logs();
                    if let Some(workspace) = &workspace {
                        workspace.complete(
                            &task_id,
                            self.name(),
                            &mut details,
                            logs,
                            125,
                            "resource_rejected",
                            duration,
                        );
                    }
                    return JobResult::Failed {
                        id: task_id,
                        error,
                        duration,
                        details,
                    };
                }
            },
            _ = cancellation.cancelled() => {
                let duration = start.elapsed();
                let mut details = execution.reporter.take_run_details();
                let logs = execution.reporter.take_logs();
                if let Some(workspace) = &workspace {
                    workspace.complete(
                        &task_id,
                        self.name(),
                        &mut details,
                        logs,
                        130,
                        "cancelled",
                        duration,
                    );
                }
                return JobResult::Failed {
                    id: task_id,
                    error: DagError::Schedule(
                        "task cancelled by DAG run cancellation".into(),
                    ),
                    duration,
                    details,
                };
            },
            _ = tokio::time::sleep(timeout.unwrap_or(Duration::MAX)), if timeout.is_some() => {
                let duration = start.elapsed();
                let mut details = execution.reporter.take_run_details();
                let logs = execution.reporter.take_logs();
                if let Some(workspace) = &workspace {
                    workspace.complete(
                        &task_id,
                        self.name(),
                        &mut details,
                        logs,
                        124,
                        "timeout",
                        duration,
                    );
                }
                return JobResult::Failed {
                    id: task_id,
                    error: DagError::Schedule(format!(
                        "task `{}` exceeded max_duration_ms of {} while awaiting resources",
                        execution.submission.spec.id,
                        execution.submission.resources.max_duration_ms.unwrap_or_default()
                    )),
                    duration,
                    details,
                };
            }
        };

        if self.stage_inputs
            && let Some(workspace) = workspace.as_mut()
            && let Err(error) = workspace
                .stage_inputs(
                    &mut execution.submission,
                    execution.engine_ctx.opendal.as_deref(),
                )
                .await
        {
            drop(resource_lease);
            let duration = start.elapsed();
            let mut details = execution.reporter.take_run_details();
            let logs = execution.reporter.take_logs();
            workspace.complete(
                &task_id,
                self.name(),
                &mut details,
                logs,
                126,
                "input_staging_failed",
                duration,
            );
            return JobResult::Failed {
                id: task_id,
                error,
                duration,
                details,
            };
        }

        let result = tokio::select! {
            result = AssertUnwindSafe(execution.node.execute(
                    &execution.engine_ctx,
                    &execution.submission.inputs,
                    &execution.reporter,
                ))
                .catch_unwind() => result,
            _ = cancellation.cancelled() => {
                let duration = start.elapsed();
                let mut details = execution.reporter.take_run_details();
                let logs = execution.reporter.take_logs();
                if let Some(workspace) = &workspace {
                    workspace.complete(
                        &task_id,
                        self.name(),
                        &mut details,
                        logs,
                        130,
                        "cancelled",
                        duration,
                    );
                }
                return JobResult::Failed {
                    id: task_id,
                    error: DagError::Schedule(
                        "task cancelled by DAG run cancellation".into(),
                    ),
                    duration,
                    details,
                };
            },
            _ = tokio::time::sleep(timeout.unwrap_or(Duration::MAX)), if timeout.is_some() => {
                let duration = start.elapsed();
                let mut details = execution.reporter.take_run_details();
                let logs = execution.reporter.take_logs();
                if let Some(workspace) = &workspace {
                    workspace.complete(
                        &task_id,
                        self.name(),
                        &mut details,
                        logs,
                        124,
                        "timeout",
                        duration,
                    );
                }
                return JobResult::Failed {
                    id: task_id,
                    error: DagError::Schedule(format!(
                        "task `{}` exceeded max_duration_ms of {}",
                        execution.submission.spec.id,
                        execution.submission.resources.max_duration_ms.unwrap_or_default()
                    )),
                    duration,
                    details,
                };
            }
        };
        drop(resource_lease);

        let duration = start.elapsed();
        let mut output_artifacts = Vec::new();
        let mut output_publish_error = None;
        if let Some(workspace) = workspace.as_mut()
            && let Ok(Ok(outputs)) = &result
        {
            match workspace
                .publish_outputs(&mut execution.submission, outputs)
                .await
            {
                Ok(artifacts) => output_artifacts.extend(artifacts),
                Err(error) => output_publish_error = Some(error),
            }
        }
        if let Some(error) = output_publish_error {
            let mut details = execution.reporter.take_run_details();
            let logs = execution.reporter.take_logs();
            if let Some(workspace) = &workspace {
                workspace.complete(
                    &task_id,
                    self.name(),
                    &mut details,
                    logs,
                    127,
                    "output_publish_failed",
                    duration,
                );
            }
            return JobResult::Failed {
                id: task_id,
                error,
                duration,
                details,
            };
        }

        let mut details = execution.reporter.take_run_details();
        let logs = execution.reporter.take_logs();
        let succeeded = matches!(result, Ok(Ok(_)));
        if succeeded {
            details
                .get_or_insert_with(Default::default)
                .output_artifacts = output_artifacts;
        }
        if let Some(workspace) = &workspace {
            workspace.complete(
                &task_id,
                self.name(),
                &mut details,
                logs,
                if succeeded { 0 } else { 1 },
                if succeeded { "success" } else { "failed" },
                duration,
            );
        }
        match result {
            Ok(Ok(outputs)) => JobResult::Success {
                id: task_id,
                outputs,
                duration,
                details,
            },
            Ok(Err(error)) => {
                warn!(node = %task_id, error = %error, "task failed");
                JobResult::Failed {
                    id: task_id,
                    error,
                    duration,
                    details,
                }
            }
            Err(panic_payload) => {
                let message = panic_payload
                    .downcast_ref::<&str>()
                    .map(|value| (*value).to_string())
                    .or_else(|| panic_payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "panicked with non-string payload".to_string());
                warn!(node = %task_id, panic = %message, "task panicked");
                JobResult::Failed {
                    id: task_id,
                    error: DagError::Schedule(format!("task panicked: {message}")),
                    duration,
                    details,
                }
            }
        }
    }
}

/// Convenience constructor used by tests and future executor adapters.
pub fn local_task_executor() -> Arc<dyn TaskExecutor> {
    Arc::new(LocalTaskExecutor::default())
}

/// Placeholder kept explicit so future remote executors cannot accidentally
/// treat an absent timeout as an infinite lease.
pub fn task_timeout(resources: &TaskResources) -> Option<Duration> {
    resources
        .max_duration_ms
        .map(|milliseconds| Duration::from_millis(milliseconds))
}
