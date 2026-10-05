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
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::node_event::{JobResult, NodeReporter};
use super::{DagNode, NodeInput};
use crate::dag::DagError;
use crate::dag::graph::PortOutputs;
use crate::registry::NodeCtx;
use crate::value::FileRef;

/// Resource requests associated with one task.
///
/// These are executor directives. The local executor enforces duration and
/// records task workspaces; CPU and memory requests are carried for future
/// resource-aware executors.
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
}

/// One named process output contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskOutputBinding {
    pub name: String,
    pub port: u8,
    pub payload: String,
}

/// Everything an executor needs for one attempt.
#[derive(Clone)]
pub struct TaskSubmission {
    pub spec: TaskSpec,
    pub inputs: Vec<NodeInput>,
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
}

impl Default for LocalTaskExecutor {
    fn default() -> Self {
        Self {
            workspace_root: std::env::temp_dir().join("autonomics-dag-tasks"),
        }
    }
}

impl LocalTaskExecutor {
    pub fn with_workspace_root(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
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
        let manifest = FileRef::local(&manifest_path, Some("json".into()))?;
        Ok(Self {
            root: workspace_root,
            manifest,
        })
    }

    fn finish(&self, status: &str, duration: Duration) {
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
    }

    fn attach(&self, details: &mut Option<super::runtime::NodeRunDetails>, exit_code: i32) {
        let details = details.get_or_insert_with(Default::default);
        details.workspace = Some(self.root.to_string_lossy().into_owned());
        details.task_manifest = Some(self.manifest.clone());
        if details.exit_code.is_none() {
            details.exit_code = Some(exit_code);
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
        let workspace =
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
                if let Some(workspace) = &workspace {
                    workspace.attach(&mut details, 130);
                    workspace.finish("cancelled", duration);
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
                if let Some(workspace) = &workspace {
                    workspace.attach(&mut details, 124);
                    workspace.finish("timeout", duration);
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

        let duration = start.elapsed();
        let mut details = execution.reporter.take_run_details();
        let succeeded = matches!(result, Ok(Ok(_)));
        if let Some(workspace) = &workspace {
            workspace.attach(&mut details, if succeeded { 0 } else { 1 });
            workspace.finish(if succeeded { "success" } else { "failed" }, duration);
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
