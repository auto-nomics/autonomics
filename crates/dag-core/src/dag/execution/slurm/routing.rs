//! Local/remote routing for targets without a typed-worker profile:
//! only `process` nodes go to the remote executor.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::super::contract::{TaskAttemptReceipt, TaskDispatch, TaskExecution};
use super::super::process::{
    ProcessTaskSpec, parse_process_task, process_task_environment, render_process_script,
    sanitize_path_component,
};
use super::super::remote::RemoteTaskExecutor;
use super::super::traits::{TaskArtifactStore, TaskExecutor, TaskLease, TaskTransport};
use super::artifact::SshArtifactStore;
use super::settings::SlurmSettings;
use super::ssh::SshTarget;
use super::transport::SlurmTaskTransport;
use crate::dag::DagError;
use crate::dag::node_event::JobResult;
use crate::value::{DomainId, FileFingerprint, FileRef};

// ── routing executor ─────────────────────────────────────────────────────
/// Coordinator executor that keeps every task on the local executor except
/// `kind == "process"` tasks, which route to the remote (Slurm) stack when
/// one is configured. Process tasks have no in-process node payload to run
/// — remote dispatch is their only execution path; typed nodes must stay
/// local because the remote path ignores the [`crate::dag::DagNode`]
/// payload entirely.
pub struct ProcessRoutingTaskExecutor {
    local: std::sync::Arc<dyn TaskExecutor>,
    remote: Option<std::sync::Arc<dyn TaskExecutor>>,
}
impl ProcessRoutingTaskExecutor {
    pub fn new(
        local: std::sync::Arc<dyn TaskExecutor>,
        remote: Option<std::sync::Arc<dyn TaskExecutor>>,
    ) -> Self {
        Self { local, remote }
    }
}
#[async_trait::async_trait]
impl TaskExecutor for ProcessRoutingTaskExecutor {
    fn requires_cancel_join(&self) -> bool {
        self.local.requires_cancel_join()
            || self
                .remote
                .as_ref()
                .is_some_and(|executor| executor.requires_cancel_join())
    }
    fn name(&self) -> &'static str {
        "process-routing"
    }
    async fn run(&self, mut execution: TaskExecution) -> JobResult {
        // The graph records a node's registered address as its kind
        // (`<plugin>/process`), while the remote contract speaks the bare
        // `process` kind. The node payload itself declares which contract it
        // implements, so route on that and normalize before dispatch — the
        // same rewrite the typed-worker lowering performs.
        if execution.submission.spec.kind != "process" && execution.node.kind() == "process" {
            execution.submission.spec.kind = "process".into();
        }
        if self.remote.is_none()
            && execution.submission.spec.kind == "process"
            && execution
                .submission
                .spec
                .spec
                .get("container")
                .is_some_and(|v| !v.is_null())
        {
            return JobResult::Failed {
                id: execution.submission.spec.id.clone(),
                error: DagError::Schedule(
                    "Apptainer tasks require an enabled Slurm backend; local fallback refused"
                        .into(),
                ),
                duration: Duration::ZERO,
                details: None,
            };
        }
        match &self.remote {
            Some(remote) if execution.submission.spec.kind == "process" => {
                remote.run(execution).await
            }
            _ => self.local.run(execution).await,
        }
    }
}
/// Assemble the full Slurm stack behind a routing executor: `process` tasks
/// go through [`RemoteTaskExecutor`] over [`SlurmTaskTransport`] +
/// [`SshArtifactStore`]; everything else stays on `local`.
pub fn slurm_backed_executor(
    settings: SlurmSettings,
    coordinator_workspace_root: impl Into<PathBuf>,
    local: std::sync::Arc<dyn TaskExecutor>,
) -> std::sync::Arc<dyn TaskExecutor> {
    let store = SshArtifactStore::new(
        settings.ssh.clone(),
        settings.scratch_dir.clone(),
        settings.local_artifact_root.clone(),
    );
    let transport = SlurmTaskTransport::new(settings);
    let remote = RemoteTaskExecutor::new(
        std::sync::Arc::new(transport),
        std::sync::Arc::new(store),
        coordinator_workspace_root,
    );
    std::sync::Arc::new(ProcessRoutingTaskExecutor::new(
        local,
        Some(std::sync::Arc::new(remote)),
    ))
}
