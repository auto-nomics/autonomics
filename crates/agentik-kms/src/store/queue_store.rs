use async_trait::async_trait;
use uuid::Uuid;

use super::error::Result;
use crate::pipeline::{KnowledgeJob, StageOutcome};

#[derive(Debug, Clone)]
pub struct QueuedKnowledgeJob {
    pub id: Uuid,
    pub job: KnowledgeJob,
    pub attempts: u32,
    pub max_attempts: u32,
}

/// Durable job queue contract for knowledge-pipeline execution.
#[async_trait]
pub trait KnowledgeQueue: Send + Sync {
    async fn enqueue(&self, job: KnowledgeJob) -> Result<Uuid>;

    async fn claim_due(&self, now_ms: i64, limit: usize) -> Result<Vec<QueuedKnowledgeJob>>;

    /// Return an unwanted claim without consuming an attempt.
    ///
    /// Specialized workers may claim a mixed batch while the queue only
    /// supports due-time claiming. Releasing a foreign job makes it visible
    /// to the next consumer immediately.
    async fn release(&self, job_id: Uuid) -> Result<()>;

    async fn complete(&self, job_id: Uuid, outcome: StageOutcome) -> Result<()>;
}
