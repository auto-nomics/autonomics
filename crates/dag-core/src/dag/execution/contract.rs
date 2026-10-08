//! Task contracts: identity, resources, submissions, dispatches, receipts.
//!
//! Everything here is plain data plus validation, so the same task can be
//! described to the in-process executor or serialized to a remote one.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::dag::node_event::NodeReporter;
use crate::dag::{DagError, DagNode, NodeInput};
use crate::registry::NodeCtx;
use crate::value::FileRef;

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

/// Placeholder kept explicit so future remote executors cannot accidentally
/// treat an absent timeout as an infinite lease.
pub fn task_timeout(resources: &TaskResources) -> Option<Duration> {
    resources.max_duration_ms.map(Duration::from_millis)
}
