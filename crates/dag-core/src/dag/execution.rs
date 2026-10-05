//! Executor-facing task contracts.
//!
//! The scheduler currently dispatches every task through
//! [`LocalTaskExecutor`]. The trait boundary keeps that path replaceable: a
//! future SLURM, Kubernetes, or batch executor can consume the same task
//! identity, process inputs, resources, and terminal result protocol.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
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

/// Resource requests associated with one task.
///
/// These are protocol fields only for now: the local executor uses the
/// scheduler semaphores and does not yet provision isolated task sandboxes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskResources {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpus: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_duration_ms: Option<u64>,
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
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalTaskExecutor;

#[async_trait::async_trait]
impl TaskExecutor for LocalTaskExecutor {
    fn name(&self) -> &'static str {
        "local"
    }

    async fn run(&self, execution: TaskExecution) -> JobResult {
        let task_id = execution.submission.spec.id.clone();
        let start = std::time::Instant::now();
        let mut execution = execution;

        let result = tokio::select! {
            result = AssertUnwindSafe(execution.node.execute(
                    &execution.engine_ctx,
                    &execution.submission.inputs,
                    &execution.reporter,
                ))
                .catch_unwind() => result,
            _ = execution.cancellation.cancelled() => {
                return JobResult::Failed {
                    id: task_id,
                    error: DagError::Schedule(
                        "task cancelled by DAG run cancellation".into(),
                    ),
                    duration: start.elapsed(),
                    details: execution.reporter.take_run_details(),
                };
            }
        };

        let duration = start.elapsed();
        let details = execution.reporter.take_run_details();
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
    Arc::new(LocalTaskExecutor)
}

/// Placeholder kept explicit so future remote executors cannot accidentally
/// treat an absent timeout as an infinite lease.
pub fn task_timeout(resources: &TaskResources) -> Option<Duration> {
    resources
        .max_duration_ms
        .map(|milliseconds| Duration::from_millis(milliseconds))
}
