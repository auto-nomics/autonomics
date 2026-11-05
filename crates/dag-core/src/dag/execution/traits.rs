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

    /// Resolve operator policy once per DAG invocation, including dynamic jobs.
    /// Existing executors remain immutable and need no snapshot.
    async fn bind_run(&self) -> Result<Option<std::sync::Arc<dyn TaskExecutor>>, DagError> {
        Ok(None)
    }

    fn execution_binding(&self) -> Option<serde_json::Value> {
        None
    }

    /// The scheduler must join this executor after cancellation so it can
    /// finish remote cancellation and retain any unconfirmed lease evidence.
    fn requires_cancel_join(&self) -> bool {
        false
    }

    /// Inspect compiled requirements on the already-bound executor before any
    /// node starts. Dynamic nodes must also be checked at dispatch.
    fn preflight(&self, _nodes: &[(&str, &dyn crate::DagNode)]) -> Result<(), DagError> {
        Ok(())
    }

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
    /// Checked by coordinators before staging or remote input resolution.
    /// Formal transports retain their separate durable submission fence too.
    fn ensure_ready(&self) -> Result<(), DagError> {
        Ok(())
    }

    async fn submit(
        &self,
        dispatch: TaskDispatch,
        artifacts: Vec<FileRef>,
    ) -> Result<TaskLease, DagError>;

    async fn wait(&self, lease: &TaskLease) -> Result<TaskAttemptReceipt, DagError>;

    async fn wait_observed(
        &self,
        lease: &TaskLease,
        _reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<TaskAttemptReceipt, DagError> {
        self.wait(lease).await
    }

    async fn cancel(&self, lease: &TaskLease) -> Result<(), DagError>;
}

/// Shared artifact transfer contract used before submit and after completion.
#[async_trait::async_trait]
pub trait TaskArtifactStore: Send + Sync {
    /// Resolve an already-authorized input on the worker's data home without
    /// downloading it to the coordinator. Default stores always stage locally.
    async fn remote_input(
        &self,
        _artifact: FileRef,
        _storage: Option<&vfs::OpendalFileStorage>,
    ) -> Result<Option<FileRef>, DagError> {
        Ok(None)
    }

    /// Publish a stable coordinator-readable reference to a retained remote
    /// file. Arrow DataFrames may use a provenance-bound remote-only provider;
    /// Channels still cross the coordinator's typed value protocol.
    async fn retained_reference(&self, _artifact: FileRef) -> Result<Option<FileRef>, DagError> {
        Ok(None)
    }

    async fn upload_observed(
        &self,
        artifact: FileRef,
        task_id: &str,
        name: &str,
        reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<FileRef, DagError> {
        reporter.phase(&format!("uploading:{name}"), "waiting for artifact upload");
        self.upload(artifact, task_id, name).await
    }

    async fn download_observed(
        &self,
        artifact: FileRef,
        task_id: &str,
        name: &str,
        reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<FileRef, DagError> {
        reporter.phase(
            &format!("downloading:{name}"),
            "waiting for artifact download",
        );
        self.download(artifact, task_id, name).await
    }

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
