use std::sync::Arc;

use agentik_sdk::model::Model;
use arc_swap::ArcSwapOption;
use uuid::Uuid;

use super::event::KnowledgeEvent;
use super::job::KnowledgeJob;
use super::stage::{KnowledgeStage, StageContext, StageOutcome, StageStatus};
use super::stages::default_stages;
use super::{MemorySource, error::Error};
use crate::KmsSystem;
use crate::now_ms;
use crate::store::KnowledgeQueue;

pub struct KnowledgePipeline {
    queue: Arc<dyn KnowledgeQueue>,
    source: Arc<dyn MemorySource>,
    system: Arc<KmsSystem>,
    model: Arc<ArcSwapOption<Model>>,
    stages: Vec<Arc<dyn KnowledgeStage>>,
}

/// One `run_due` pass, separated by what the runner did with each claim.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KnowledgeRunSummary {
    pub claimed: usize,
    pub executed: usize,
    pub released: usize,
}

impl KnowledgePipeline {
    #[must_use]
    pub fn new(
        queue: Arc<dyn KnowledgeQueue>,
        source: Arc<dyn MemorySource>,
        system: Arc<KmsSystem>,
        model: Arc<ArcSwapOption<Model>>,
    ) -> Self {
        Self {
            queue,
            source,
            system,
            model,
            stages: default_stages(),
        }
    }

    pub async fn dispatch(&self, event: KnowledgeEvent) -> Result<Vec<Uuid>, Error> {
        let jobs = event.jobs();
        let mut ids = Vec::new();
        for job in jobs {
            ids.push(self.queue.enqueue(job).await?);
        }
        Ok(ids)
    }

    pub async fn run_due(&self, limit: usize) -> Result<usize, Error> {
        Ok(self.run_due_internal(None, limit).await?.executed)
    }

    /// Claim due jobs, execute only `job_kind`, and release foreign jobs.
    ///
    /// This is the consumer primitive used by specialized workers. A Phase 1
    /// worker can therefore leave Phase 2 work pending instead of holding its
    /// lease for an hour.
    pub async fn run_due_for_kind(
        &self,
        job_kind: &'static str,
        limit: usize,
    ) -> Result<KnowledgeRunSummary, Error> {
        self.run_due_internal(Some(job_kind), limit).await
    }

    async fn run_due_internal(
        &self,
        job_kind: Option<&'static str>,
        limit: usize,
    ) -> Result<KnowledgeRunSummary, Error> {
        let mut summary = KnowledgeRunSummary::default();
        for _ in 0..4 {
            let claimed = self.queue.claim_due(now_ms(), limit).await?;
            if claimed.is_empty() {
                break;
            }
            summary.claimed += claimed.len();
            let mut executed = 0;
            for queued in claimed {
                if let Some(kind) = job_kind
                    && queued.job.kind() != kind
                {
                    self.queue.release(queued.id).await?;
                    summary.released += 1;
                    continue;
                }
                executed += 1;
                self.execute_queued(queued).await?;
            }
            summary.executed += executed;
            if executed == 0 {
                break;
            }
        }
        Ok(summary)
    }

    async fn execute_queued(&self, queued: crate::store::QueuedKnowledgeJob) -> Result<(), Error> {
        let Some(stage) = self
            .stages
            .iter()
            .find(|stage| stage.accepts(&queued.job))
            .cloned()
        else {
            self.queue
                .complete(
                    queued.id,
                    StageOutcome::failed(format!("no stage accepts {}", queued.job.kind())),
                )
                .await?;
            return Ok(());
        };

        let context = StageContext {
            source: Arc::clone(&self.source),
            system: Arc::clone(&self.system),
            model: Arc::clone(&self.model),
        };
        let mut outcome = match stage.execute(context, queued.job.clone()).await {
            Ok(outcome) => outcome,
            Err(error) => {
                if queued.attempts >= queued.max_attempts {
                    StageOutcome::failed(error.to_string())
                } else {
                    let delay = 60_u64.saturating_mul(1_u64 << (queued.attempts - 1).min(5));
                    StageOutcome::retry(delay, error.to_string())
                }
            }
        };
        if queued.attempts >= queued.max_attempts
            && matches!(outcome.status, StageStatus::RetryLater { .. })
        {
            outcome = StageOutcome::failed("job exhausted retry attempts");
        }
        self.queue.complete(queued.id, outcome).await?;
        Ok(())
    }
}
