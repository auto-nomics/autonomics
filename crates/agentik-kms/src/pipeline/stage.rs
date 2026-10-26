use std::sync::Arc;

use agentik_sdk::model::Model;
use arc_swap::ArcSwapOption;
use async_trait::async_trait;

use super::job::KnowledgeJob;
use crate::KmsSystem;
use crate::MemorySource;

#[derive(Clone)]
pub struct StageContext {
    pub source: Arc<dyn MemorySource>,
    pub system: Arc<KmsSystem>,
    pub model: Arc<ArcSwapOption<Model>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageStatus {
    Completed,
    RetryLater { after_secs: u64, reason: String },
    Failed { reason: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct StageOutcome {
    pub status: StageStatus,
    pub next_jobs: Vec<KnowledgeJob>,
}

impl StageOutcome {
    #[must_use]
    pub fn completed(next_jobs: Vec<KnowledgeJob>) -> Self {
        Self {
            status: StageStatus::Completed,
            next_jobs,
        }
    }

    #[must_use]
    pub fn retry(after_secs: u64, reason: impl Into<String>) -> Self {
        Self {
            status: StageStatus::RetryLater {
                after_secs,
                reason: reason.into(),
            },
            next_jobs: Vec::new(),
        }
    }

    #[must_use]
    pub fn failed(reason: impl Into<String>) -> Self {
        Self {
            status: StageStatus::Failed {
                reason: reason.into(),
            },
            next_jobs: Vec::new(),
        }
    }
}

/// A unified execution boundary for knowledge-pipeline stages.
///
/// Implementations own domain logic, while the runner owns persistence,
/// retries, leases, and downstream-job enqueueing.
#[async_trait]
pub trait KnowledgeStage: Send + Sync {
    fn name(&self) -> &'static str;

    fn accepts(&self, job: &KnowledgeJob) -> bool;

    async fn execute(
        &self,
        context: StageContext,
        job: KnowledgeJob,
    ) -> Result<StageOutcome, super::error::Error>;
}
