//! A coordinator-wide limit on active remote executor futures.
//!
//! This is not a count of live Slurm jobs: a transport can report an uncertain
//! outcome while a job remains alive. Slurm/QoS and reconciliation remain the
//! authority for cluster resources and uncertain attempts.

use std::{sync::Arc, time::Duration};

use tokio::sync::Semaphore;

use super::{TaskExecution, TaskExecutor};
use crate::dag::{DagError, node_event::JobResult};

pub const MAX_REMOTE_TASKS: usize = 1024;

/// An explicitly requested limit; ordinary remote placement has no default cap.
/// Share one instance across sessions; its permits span the complete inner
/// invocation, including staging, submission, polling and cancellation cleanup.
pub struct LimitedTaskExecutor {
    inner: Arc<dyn TaskExecutor>,
    permits: Semaphore,
}

impl LimitedTaskExecutor {
    pub fn new(inner: Arc<dyn TaskExecutor>, limit: usize) -> Result<Self, DagError> {
        if !(1..=MAX_REMOTE_TASKS).contains(&limit) {
            return Err(DagError::Schedule(format!(
                "max_remote_tasks must be between 1 and {MAX_REMOTE_TASKS}"
            )));
        }
        Ok(Self {
            inner,
            permits: Semaphore::new(limit),
        })
    }
}

#[async_trait::async_trait]
impl TaskExecutor for LimitedTaskExecutor {
    fn requires_cancel_join(&self) -> bool {
        self.inner.requires_cancel_join()
    }
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    async fn run(&self, execution: TaskExecution) -> JobResult {
        let cancelled = || JobResult::Failed {
            id: execution.submission.spec.id.clone(),
            error: DagError::Schedule("task cancelled before remote admission".into()),
            duration: Duration::ZERO,
            details: execution.reporter.take_run_details(),
        };
        execution.reporter.phase(
            "waiting_for_capacity",
            "waiting for remote execution capacity",
        );
        let _permit = tokio::select! {
            biased;
            _ = execution.cancellation.cancelled() => return cancelled(),
            permit = self.permits.acquire() => match permit {
                Ok(permit) => permit,
                Err(_) => return cancelled(),
            },
        };
        if execution.cancellation.is_cancelled() {
            return cancelled();
        }
        // Do not select against cancellation here: dropping an active remote
        // future would bypass its lease cancellation/reconciliation protocol.
        self.inner.run(execution).await
    }
}

#[cfg(test)]
#[path = "limited_tests.rs"]
mod tests;
