use std::sync::Arc;

use agentik_sdk::model::Model;
use arc_swap::ArcSwapOption;
use uuid::Uuid;

use super::event::KnowledgeEvent;
use super::job::KnowledgeJob;
use super::stage::{KnowledgeStage, StageContext, StageOutcome, StageStatus};
use super::stages::default_stages;
use super::{MemoryBackend, MemorySource, error::Error};
use crate::now_ms;
use crate::store::KnowledgeQueue;

pub struct KnowledgePipeline {
    queue: Arc<dyn KnowledgeQueue>,
    source: Arc<dyn MemorySource>,
    backend: Arc<MemoryBackend>,
    model: Arc<ArcSwapOption<Model>>,
    stages: Vec<Arc<dyn KnowledgeStage>>,
}

impl KnowledgePipeline {
    #[must_use]
    pub fn new(
        queue: Arc<dyn KnowledgeQueue>,
        source: Arc<dyn MemorySource>,
        backend: Arc<MemoryBackend>,
        model: Arc<ArcSwapOption<Model>>,
    ) -> Self {
        Self {
            queue,
            source,
            backend,
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
        let mut count = 0;
        for _ in 0..4 {
            let claimed = self.queue.claim_due(now_ms(), limit).await?;
            if claimed.is_empty() {
                break;
            }
            count += claimed.len();
            for queued in claimed {
                self.execute_queued(queued).await?;
            }
        }
        Ok(count)
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
            backend: Arc::clone(&self.backend),
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
