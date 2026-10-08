//! Execution backend contracts: the in-process executor interface, the
//! remote transport protocol, and the artifact store used to move inputs
//! and outputs across the coordinator boundary.

use serde::{Deserialize, Serialize};

use super::contract::{TaskAttemptReceipt, TaskDispatch, TaskExecution};
use crate::dag::DagError;
use crate::dag::node_event::JobResult;
use crate::value::FileRef;

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
