//! Executor-facing task contracts.
//!
//! The scheduler currently dispatches every task through
//! [`LocalTaskExecutor`]. The trait boundary keeps that path replaceable: a
//! future SLURM, Kubernetes, or batch executor can consume the same task
//! identity, process inputs, resources, and terminal result protocol.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
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
use arrow::ipc::reader::FileReader as ArrowFileReader;
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
    /// File-backed outputs grouped by the port that produced them.
    pub output_artifacts_by_port: BTreeMap<u8, Vec<FileRef>>,
}

/// Nextflow-style process definition carried in [`TaskSpec::spec`].
///
/// The coordinator never depends on in-process node objects for this backend:
/// a process task is fully described by an executable script or argv,
/// environment bindings, and output file patterns relative to its workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessTaskSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<ProcessTaskOutput>,
}

impl ProcessTaskSpec {
    fn parse(value: &serde_json::Value) -> Result<Self, DagError> {
        let process =
            serde_json::from_value::<ProcessTaskSpec>(value.clone()).map_err(|error| {
                DagError::Schedule(format!("invalid process task specification: {error}"))
            })?;
        process.validate()?;
        Ok(process)
    }

    pub fn validate(&self) -> Result<(), DagError> {
        if self.script.is_none() && self.command.is_empty() {
            return Err(DagError::Schedule(
                "process task requires either script or command".into(),
            ));
        }
        if self.script.is_some() && !self.command.is_empty() {
            return Err(DagError::Schedule(
                "process task cannot define both script and command".into(),
            ));
        }
        if let Some(script) = &self.script
            && script.trim().is_empty()
        {
            return Err(DagError::Schedule("process script cannot be empty".into()));
        }
        if self.command.first().is_some_and(String::is_empty) {
            return Err(DagError::Schedule(
                "process command executable is empty".into(),
            ));
        }
        let mut ports = BTreeSet::new();
        for output in &self.outputs {
            if output.port != ports.len() as u8 {
                return Err(DagError::Schedule(format!(
                    "process outputs must use contiguous ports starting at zero; expected {}, got {}",
                    ports.len(),
                    output.port
                )));
            }
            if !ports.insert(output.port) {
                return Err(DagError::Schedule(format!(
                    "process task declares output port {} more than once",
                    output.port
                )));
            }
            if output.pattern.trim().is_empty() {
                return Err(DagError::Schedule(format!(
                    "process output on port {} has an empty pattern",
                    output.port
                )));
            }
            let pattern_path = Path::new(&output.pattern);
            if pattern_path.is_absolute()
                || pattern_path
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir))
            {
                return Err(DagError::Schedule(format!(
                    "process output pattern `{}` must stay inside the task workspace",
                    output.pattern
                )));
            }
            glob::Pattern::new(&output.pattern).map_err(|error| {
                DagError::Schedule(format!(
                    "process output pattern `{}` is invalid: {error}",
                    output.pattern
                ))
            })?;
        }
        Ok(())
    }
}

/// Files produced by a process task, grouped by the physical output port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessTaskOutput {
    pub port: u8,
    pub pattern: String,
}

/// A task transport that runs real OS processes in isolated workspaces.
///
/// This is the first concrete backend behind [`TaskTransport`]. It models a
/// shared-filesystem remote worker: artifacts are copied in, the process runs
/// outside coordinator memory, and artifacts plus an attempt receipt are
/// copied back through the existing transfer protocol.
#[derive(Debug)]
pub struct ProcessTaskTransport {
    workspace_root: PathBuf,
    leases: AtomicU64,
    resource_budget: LocalResourceBudget,
    jobs: std::sync::Mutex<
        HashMap<String, tokio::task::JoinHandle<Result<TaskAttemptReceipt, DagError>>>,
    >,
}

struct PreparedProcessTask {
    dispatch: TaskDispatch,
    process: ProcessTaskSpec,
    workspace: PathBuf,
    inputs_by_name: BTreeMap<String, Vec<PathBuf>>,
}

impl ProcessTaskTransport {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            leases: AtomicU64::new(1),
            resource_budget: LocalTaskExecutor::discover_resource_budget(),
            jobs: std::sync::Mutex::new(HashMap::new()),
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
            leases: AtomicU64::new(1),
            resource_budget: LocalResourceBudget::new(cpu_limit, memory_limit_bytes),
            jobs: std::sync::Mutex::new(HashMap::new()),
        })
    }

    fn parse_process(dispatch: &TaskDispatch) -> Result<ProcessTaskSpec, DagError> {
        if dispatch.spec.kind != "process" {
            return Err(DagError::Schedule(format!(
                "process transport cannot execute task kind `{}`",
                dispatch.spec.kind
            )));
        }
        let process = ProcessTaskSpec::parse(&dispatch.spec.spec)?;

        let expected_ports = dispatch
            .spec
            .outputs
            .iter()
            .map(|output| output.port)
            .collect::<BTreeSet<_>>();
        let actual_ports = process
            .outputs
            .iter()
            .map(|output| output.port)
            .collect::<BTreeSet<_>>();
        if expected_ports != actual_ports {
            return Err(DagError::Schedule(format!(
                "process outputs must match task output ports; expected {expected_ports:?}, got {actual_ports:?}"
            )));
        }
        Ok(process)
    }

    fn prepare(
        &self,
        dispatch: TaskDispatch,
        artifacts: Vec<FileRef>,
        lease_id: &str,
    ) -> Result<PreparedProcessTask, DagError> {
        let process = Self::parse_process(&dispatch)?;
        let workspace = self
            .workspace_root
            .join(sanitize_path_component(&dispatch.spec.id))
            .join(sanitize_path_component(lease_id));
        let input_root = workspace.join("inputs");
        std::fs::create_dir_all(&input_root).map_err(|error| {
            DagError::Schedule(format!(
                "cannot create process workspace `{}`: {error}",
                input_root.display()
            ))
        })?;

        let mut artifacts = artifacts.into_iter();
        let mut inputs_by_name = BTreeMap::new();
        for binding in &dispatch.spec.inputs {
            let mut staged = Vec::with_capacity(binding.staged_paths.len());
            for (index, _) in binding.staged_paths.iter().enumerate() {
                let artifact = artifacts.next().ok_or_else(|| {
                    DagError::Schedule(format!(
                        "process task `{}` is missing uploaded artifact for input `{}` index {index}",
                        dispatch.spec.id, binding.name
                    ))
                })?;
                let source = Path::new(&artifact.path);
                let file_name = source
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| format!("artifact-{index}"));
                let destination = input_root.join(format!(
                    "{}-{index}-{}",
                    sanitize_path_component(&binding.name),
                    file_name
                ));
                std::fs::copy(source, &destination).map_err(|error| {
                    DagError::Schedule(format!(
                        "cannot stage process input `{}` -> `{}`: {error}",
                        source.display(),
                        destination.display()
                    ))
                })?;
                staged.push(destination);
            }
            inputs_by_name.insert(binding.name.clone(), staged);
        }
        if artifacts.next().is_some() {
            return Err(DagError::Schedule(format!(
                "process task `{}` received more uploaded artifacts than declared staged inputs",
                dispatch.spec.id
            )));
        }

        let manifest_path = workspace.join("task.json");
        let manifest_bytes = serde_json::to_vec_pretty(&dispatch).map_err(|error| {
            DagError::Schedule(format!("cannot serialize process task manifest: {error}"))
        })?;
        std::fs::write(&manifest_path, manifest_bytes).map_err(|error| {
            DagError::Schedule(format!(
                "cannot write process task manifest `{}`: {error}",
                manifest_path.display()
            ))
        })?;

        Ok(PreparedProcessTask {
            dispatch,
            process,
            workspace,
            inputs_by_name,
        })
    }

    async fn execute(
        prepared: PreparedProcessTask,
        _resource_lease: LocalResourceLease,
    ) -> Result<TaskAttemptReceipt, DagError> {
        let start = std::time::Instant::now();
        let PreparedProcessTask {
            dispatch,
            process,
            workspace,
            inputs_by_name,
        } = prepared;

        let mut command = if process.script.is_some() {
            tokio::process::Command::new(if cfg!(windows) { "cmd" } else { "sh" })
        } else {
            tokio::process::Command::new(&process.command[0])
        };
        command.current_dir(&workspace).kill_on_drop(true);

        let mut environment = process.env.clone();
        environment.insert("AUTONOMICS_TASK_ID".into(), dispatch.spec.id.clone());
        environment.insert(
            "AUTONOMICS_WORKSPACE".into(),
            workspace.to_string_lossy().into_owned(),
        );
        environment.insert(
            "AUTONOMICS_INPUTS".into(),
            serde_json::to_string(&inputs_by_name).map_err(|error| {
                DagError::Schedule(format!("cannot serialize process inputs: {error}"))
            })?,
        );
        environment.insert(
            "AUTONOMICS_INPUT_COUNT".into(),
            inputs_by_name
                .values()
                .map(|paths| paths.len())
                .sum::<usize>()
                .to_string(),
        );
        for (name, paths) in &inputs_by_name {
            let joined = paths
                .iter()
                .map(|path| path.to_string_lossy())
                .collect::<Vec<_>>()
                .join(":");
            environment.insert(
                format!("AUTONOMICS_INPUT_{}", sanitize_env_name(name)),
                joined,
            );
            for (index, path) in paths.iter().enumerate() {
                environment.insert(
                    format!("AUTONOMICS_INPUT_{}_{index}", sanitize_env_name(name)),
                    path.to_string_lossy().into_owned(),
                );
            }
        }
        let process_environment = environment.clone();
        for (name, value) in environment {
            command.env(name, value);
        }

        let stdout_path = workspace.join("stdout.log");
        let stderr_path = workspace.join("stderr.log");
        let stdout = std::fs::File::create(&stdout_path).map_err(|error| {
            DagError::Schedule(format!(
                "cannot create process stdout `{}`: {error}",
                stdout_path.display()
            ))
        })?;
        let stderr = std::fs::File::create(&stderr_path).map_err(|error| {
            DagError::Schedule(format!(
                "cannot create process stderr `{}`: {error}",
                stderr_path.display()
            ))
        })?;
        command.stdout(stdout).stderr(stderr);
        if let Some(script) = &process.script {
            if cfg!(windows) {
                command.arg("/C");
            } else {
                command.arg("-c");
            }
            let script = render_process_script(script, &process_environment, &inputs_by_name);
            command.arg(script);
        } else {
            let rendered = process
                .command
                .iter()
                .map(|argument| {
                    render_process_script(argument, &process_environment, &inputs_by_name)
                })
                .collect::<Vec<_>>();
            command.arg(&rendered[0]);
            command.args(&rendered[1..]);
        }

        let mut child = command.spawn().map_err(|error| {
            DagError::Schedule(format!(
                "cannot start process task `{}`: {error}",
                dispatch.spec.id
            ))
        })?;
        let timeout = task_timeout(&dispatch.resources);
        let outcome = tokio::select! {
            status = child.wait() => (status, false),
            _ = tokio::time::sleep(timeout.unwrap_or(Duration::MAX)), if timeout.is_some() => {
                let _ = child.kill().await;
                (child.wait().await, true)
            }
        };
        let (status, timed_out) = outcome;
        let status = status.map_err(|error| {
            DagError::Schedule(format!(
                "failed to wait for process task `{}`: {error}",
                dispatch.spec.id
            ))
        })?;
        let success = !timed_out && status.success();
        let status_name = if timed_out {
            "timeout"
        } else if success {
            "success"
        } else {
            "failed"
        };

        let mut artifacts_by_port = BTreeMap::<u8, Vec<FileRef>>::new();
        if success {
            for output in &process.outputs {
                let pattern = workspace.join(&output.pattern);
                let pattern = pattern.to_str().ok_or_else(|| {
                    DagError::Schedule("process workspace is not valid UTF-8".into())
                })?;
                let matches = glob::glob(pattern).map_err(|error| {
                    DagError::Schedule(format!(
                        "invalid process output pattern `{}`: {error}",
                        output.pattern
                    ))
                })?;
                let mut files = Vec::new();
                for match_path in matches {
                    let path = match_path.map_err(|error| {
                        DagError::Schedule(format!(
                            "cannot enumerate process output `{}`: {error}",
                            output.pattern
                        ))
                    })?;
                    if path.is_file() {
                        files.push(path);
                    }
                }
                files.sort();
                if files.is_empty() {
                    return Err(DagError::Schedule(format!(
                        "process output port {} matched no files for `{}`",
                        output.port, output.pattern
                    )));
                }
                let mut artifacts = Vec::with_capacity(files.len());
                for path in files {
                    artifacts.push(FileRef::local(&path, format_for_path(&path))?);
                }
                artifacts_by_port.insert(output.port, artifacts);
            }
        }

        let output_artifacts = artifacts_by_port
            .values()
            .flat_map(|artifacts| artifacts.iter().cloned())
            .collect::<Vec<_>>();
        let manifest = FileRef::local(workspace.join("task.json"), Some("json".into()))?;
        let receipt = TaskAttemptReceipt {
            task_id: dispatch.spec.id,
            executor: "process".into(),
            status: status_name.into(),
            elapsed_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
            exit_code: if timed_out {
                124
            } else {
                status.code().unwrap_or(if success { 0 } else { 1 })
            },
            workspace: workspace.to_string_lossy().into_owned(),
            task_manifest: manifest,
            output_artifacts,
            output_artifacts_by_port: artifacts_by_port,
        };
        let receipt_path = workspace.join("attempt.json");
        let receipt_bytes = serde_json::to_vec_pretty(&receipt).map_err(|error| {
            DagError::Schedule(format!("cannot serialize process attempt receipt: {error}"))
        })?;
        std::fs::write(&receipt_path, receipt_bytes).map_err(|error| {
            DagError::Schedule(format!(
                "cannot write process attempt receipt `{}`: {error}",
                receipt_path.display()
            ))
        })?;
        Ok(receipt)
    }
}

#[async_trait::async_trait]
impl TaskTransport for ProcessTaskTransport {
    async fn submit(
        &self,
        dispatch: TaskDispatch,
        artifacts: Vec<FileRef>,
    ) -> Result<TaskLease, DagError> {
        dispatch.resources.validate()?;
        let resource_lease = self.resource_budget.acquire(&dispatch.resources).await?;
        let lease_sequence = self.leases.fetch_add(1, Ordering::Relaxed);
        let lease_id = format!("lease-{lease_sequence}");
        let prepared = self.prepare(dispatch, artifacts, &lease_id)?;
        let task_id = prepared.dispatch.spec.id.clone();
        let handle = tokio::spawn(Self::execute(prepared, resource_lease));
        self.jobs
            .lock()
            .map_err(|_| DagError::Schedule("process transport state poisoned".into()))?
            .insert(lease_id.clone(), handle);
        Ok(TaskLease { lease_id, task_id })
    }

    async fn wait(&self, lease: &TaskLease) -> Result<TaskAttemptReceipt, DagError> {
        let handle = self
            .jobs
            .lock()
            .map_err(|_| DagError::Schedule("process transport state poisoned".into()))?
            .remove(&lease.lease_id)
            .ok_or_else(|| {
                DagError::Schedule(format!(
                    "process lease `{}` is unknown, cancelled, or already consumed",
                    lease.lease_id
                ))
            })?;
        handle.await.map_err(|error| {
            DagError::Schedule(format!(
                "process task for lease `{}` failed: {error}",
                lease.lease_id
            ))
        })?
    }

    async fn cancel(&self, lease: &TaskLease) -> Result<(), DagError> {
        if let Some(handle) = self
            .jobs
            .lock()
            .map_err(|_| DagError::Schedule("process transport state poisoned".into()))?
            .remove(&lease.lease_id)
        {
            handle.abort();
        }
        Ok(())
    }
}

fn sanitize_path_component(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric()
                || character == '.'
                || character == '_'
                || character == '-'
            {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
        "task".into()
    } else {
        sanitized
    }
}

fn sanitize_env_name(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn format_for_path(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .filter(|extension| !extension.is_empty())
}

fn render_process_script(
    script: &str,
    environment: &BTreeMap<String, String>,
    inputs_by_name: &BTreeMap<String, Vec<PathBuf>>,
) -> String {
    let mut rendered = script.to_string();
    for (name, paths) in inputs_by_name {
        let joined = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(":");
        rendered = rendered.replace(&format!("{{{{input.{name}}}}}"), &joined);
        rendered = rendered.replace(&format!("{{{{input.{name}[*]}}}}"), &joined);
        for (index, path) in paths.iter().enumerate() {
            rendered = rendered.replace(
                &format!("{{{{input.{name}[{index}]}}}}"),
                &path.to_string_lossy(),
            );
        }
    }
    for (name, value) in environment {
        rendered = rendered.replace(&format!("{{{{env.{name}}}}}"), value);
    }
    rendered
}

/// Execution backend for physical DAG tasks.
#[async_trait::async_trait]
pub trait TaskExecutor: Send + Sync {
    fn name(&self) -> &'static str;

    async fn run(&self, execution: TaskExecution) -> JobResult;
}

/// Lease returned by a remote execution transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskLease {
    pub lease_id: String,
    pub task_id: String,
}

/// Transport for submitting memory-independent task work orders.
#[async_trait::async_trait]
pub trait TaskTransport: Send + Sync {
    async fn submit(
        &self,
        dispatch: TaskDispatch,
        artifacts: Vec<FileRef>,
    ) -> Result<TaskLease, DagError>;

    async fn wait(&self, lease: &TaskLease) -> Result<TaskAttemptReceipt, DagError>;

    async fn cancel(&self, lease: &TaskLease) -> Result<(), DagError>;
}

/// Shared artifact transfer contract used before submit and after completion.
#[async_trait::async_trait]
pub trait TaskArtifactStore: Send + Sync {
    async fn upload(
        &self,
        artifact: FileRef,
        task_id: &str,
        name: &str,
    ) -> Result<FileRef, DagError>;

    async fn download(
        &self,
        artifact: FileRef,
        task_id: &str,
        name: &str,
    ) -> Result<FileRef, DagError>;
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
    ) -> Result<BTreeMap<u8, Vec<FileRef>>, DagError> {
        let output_root = self.root.join("outputs");
        std::fs::create_dir_all(&output_root).map_err(|error| {
            DagError::Schedule(format!(
                "cannot create output artifact directory `{}`: {error}",
                output_root.display()
            ))
        })?;
        let mut artifacts_by_port = BTreeMap::<u8, Vec<FileRef>>::new();
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
            artifacts_by_port.insert(port, port_artifacts);
        }
        for output in &mut submission.spec.outputs {
            if let Some(paths) = artifacts_by_port.get(&output.port) {
                output.artifact_paths =
                    paths.iter().map(|artifact| artifact.path.clone()).collect();
            }
        }
        self.manifest = Self::write_manifest(&self.root, submission)?;
        Ok(artifacts_by_port)
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
            output_artifacts_by_port: details.output_artifacts_by_port.clone(),
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
        let mut output_artifacts_by_port = BTreeMap::<u8, Vec<FileRef>>::new();
        let mut output_publish_error = None;
        if let Some(workspace) = workspace.as_mut()
            && let Ok(Ok(outputs)) = &result
        {
            match workspace
                .publish_outputs(&mut execution.submission, outputs)
                .await
            {
                Ok(artifacts) => output_artifacts_by_port = artifacts,
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
        let output_artifacts = output_artifacts_by_port
            .values()
            .flat_map(|artifacts| artifacts.iter().cloned())
            .collect::<Vec<_>>();
        if succeeded {
            let details = details.get_or_insert_with(Default::default);
            details.output_artifacts = output_artifacts;
            details.output_artifacts_by_port = output_artifacts_by_port;
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
