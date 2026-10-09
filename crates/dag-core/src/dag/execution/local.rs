//! The default in-process executor: creates the task workspace, leases the
//! resource budget, optionally stages inputs, runs the node under
//! cancellation and timeout, publishes outputs, and writes the receipt.

use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt;
use tracing::warn;

use super::contract::{TaskExecution, TaskResources, task_timeout};
use super::resources::LocalResourceBudget;
use super::traits::TaskExecutor;
use super::workspace::LocalTaskWorkspace;
use crate::dag::DagError;
use crate::dag::node_event::JobResult;
use crate::value::FileRef;

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
            workspace_root: super::gc::dag_tasks_root(),
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

    pub(super) fn discover_resource_budget() -> LocalResourceBudget {
        let cpus = std::thread::available_parallelism()
            .map(|cpus| cpus.get() as u32)
            .ok();
        let memory_bytes = crate::resource::sample_memory_usage()
            .ok()
            .map(|sample| sample.limit_bytes);
        LocalResourceBudget::new(cpus, memory_bytes)
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
