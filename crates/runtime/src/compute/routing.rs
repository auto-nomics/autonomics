use super::{BuiltTarget, ComputeRegistry, ExecutionBinding};
use async_trait::async_trait;
use container_runtime::{
    ContainerRunRequest, ContainerRunResult, ContainerRuntimeError, PodmanConnection,
    types::PrivateEnvironment,
};
use dag_core::{
    TaskExecution, TaskExecutor,
    dag::{DagError, node_event::JobResult},
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

tokio::task_local! { static CONTAINERS: Arc<dyn PodmanConnection>; }

pub struct ComputeExecutor(pub Arc<ComputeRegistry>);

#[async_trait]
impl TaskExecutor for ComputeExecutor {
    fn name(&self) -> &'static str {
        "compute-router"
    }
    async fn bind_run(&self) -> Result<Option<Arc<dyn TaskExecutor>>, DagError> {
        let (target, binding) = self.0.bind().map_err(DagError::Schedule)?;
        Ok(Some(Arc::new(BoundExecutor { target, binding })))
    }
    async fn run(&self, execution: TaskExecution) -> JobResult {
        match self.bind_run().await {
            Ok(Some(executor)) => executor.run(execution).await,
            Ok(None) => unreachable!(),
            Err(error) => JobResult::Failed {
                id: execution.submission.spec.id,
                error,
                duration: Duration::ZERO,
                details: execution.reporter.take_run_details(),
            },
        }
    }
}

struct BoundExecutor {
    target: Arc<BuiltTarget>,
    binding: ExecutionBinding,
}
#[async_trait]
impl TaskExecutor for BoundExecutor {
    fn requires_cancel_join(&self) -> bool {
        self.target.executor.requires_cancel_join()
            || matches!(
                self.target.target.backend,
                super::ComputeBackend::Slurm { .. }
            )
    }
    fn name(&self) -> &'static str {
        "bound-compute-target"
    }
    fn execution_binding(&self) -> Option<serde_json::Value> {
        serde_json::to_value(&self.binding).ok()
    }
    fn preflight(&self, nodes: &[(&str, &dyn dag_core::DagNode)]) -> Result<(), DagError> {
        if !matches!(
            self.target.target.backend,
            super::ComputeBackend::Slurm { .. }
        ) {
            return Ok(());
        }
        let mut blockers = Vec::new();
        for (id, node) in nodes {
            for image in node.required_container_images() {
                if let Err(error) = self.target.containers.image_resolution(image) {
                    blockers.push(serde_json::json!({"node":id,"image":image,
                        "code":"IMAGE_MAPPING_UNAVAILABLE","detail":error.to_string()}));
                }
            }
        }
        if blockers.is_empty() {
            return Ok(());
        }
        // A streaming workflow may already have dispatched other tasks before
        // this preflight is reached.  Only the blocked nodes are guaranteed
        // not to have been submitted by this call.
        Err(DagError::Schedule(
            serde_json::json!({"state":"WORKFLOW_BLOCKED",
            "target_id":self.binding.target_id,"target_revision":self.binding.target_revision,
            "blockers":blockers,"blocked_nodes_submitted":false})
            .to_string(),
        ))
    }
    async fn run(&self, execution: TaskExecution) -> JobResult {
        // Covers dynamic fanout and direct executor callers on the same target
        // captured at bind_run; a policy change cannot retarget this check.
        if let Err(error) = self.preflight(&[(
            execution.submission.spec.id.as_str(),
            execution.node.as_ref(),
        )]) {
            return JobResult::Failed {
                id: execution.submission.spec.id,
                error,
                duration: Duration::ZERO,
                details: execution.reporter.take_run_details(),
            };
        }
        // Every node future carries the same container backend as its DAG run.
        CONTAINERS
            .scope(
                self.target.containers.clone(),
                self.target.executor.run(execution),
            )
            .await
    }
}

pub(crate) struct ContainerRouter {
    pub registry: Arc<ComputeRegistry>,
    pub root: PathBuf,
}
impl ContainerRouter {
    fn selected(&self) -> Result<Arc<dyn PodmanConnection>, ContainerRuntimeError> {
        CONTAINERS
            .try_with(Arc::clone)
            .or_else(|_| self.registry.selected().map(|(t, _)| t.containers.clone()))
            .map_err(ContainerRuntimeError::Invalid)
    }
}
#[async_trait]
impl PodmanConnection for ContainerRouter {
    fn workspace_root(&self) -> &Path {
        &self.root
    }
    fn name(&self) -> &'static str {
        "compute-router"
    }
    async fn run_observed(
        &self,
        request: ContainerRunRequest,
        env: Option<PrivateEnvironment>,
        observer: container_runtime::observation::RunObserver,
    ) -> Result<ContainerRunResult, ContainerRuntimeError> {
        self.selected()?.run_observed(request, env, observer).await
    }
    async fn run(
        &self,
        request: ContainerRunRequest,
    ) -> Result<ContainerRunResult, ContainerRuntimeError> {
        self.selected()?.run(request).await
    }
    async fn run_with_private_env(
        &self,
        request: ContainerRunRequest,
        env: PrivateEnvironment,
    ) -> Result<ContainerRunResult, ContainerRuntimeError> {
        self.selected()?.run_with_private_env(request, env).await
    }
}

pub(crate) struct UnavailableContainers {
    pub root: PathBuf,
}

/// Image preparation has a local implementation only. Background preparation
/// and direct callers must obey the same policy as agent tool admission.
pub(crate) struct ImageBuilder {
    pub registry: Arc<ComputeRegistry>,
    pub local: Arc<dyn container_runtime::ImageBuildConnection>,
}
#[async_trait]
impl container_runtime::ImageBuildConnection for ImageBuilder {
    async fn build(
        &self,
        request: container_runtime::ImageBuildRequest,
    ) -> Result<container_runtime::ImageBuildResult, ContainerRuntimeError> {
        let (target, policy) = self
            .registry
            .selected()
            .map_err(ContainerRuntimeError::Invalid)?;
        if !matches!(target.target.backend, super::ComputeBackend::Local) {
            return Err(ContainerRuntimeError::Invalid("image preparation is unavailable on the selected remote target; prepare and admit its environment separately".into()));
        }
        self.registry
            .record_binding(&target, &policy)
            .map_err(ContainerRuntimeError::Invalid)?;
        self.local.build(request).await
    }
    async fn push(
        &self,
        request: container_runtime::ImagePushRequest,
    ) -> Result<container_runtime::ImagePushResult, ContainerRuntimeError> {
        // Publishing already-built bytes is a storage operation.
        self.local.push(request).await
    }
}
#[async_trait]
impl PodmanConnection for UnavailableContainers {
    fn workspace_root(&self) -> &Path {
        &self.root
    }
    fn name(&self) -> &'static str {
        "unavailable-remote-container"
    }
    async fn run(
        &self,
        _: ContainerRunRequest,
    ) -> Result<ContainerRunResult, ContainerRuntimeError> {
        Err(ContainerRuntimeError::Invalid("selected Slurm target has no admitted Apptainer image manifest; local fallback is disabled".into()))
    }
}
