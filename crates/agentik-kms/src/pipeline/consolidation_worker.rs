use std::sync::Arc;

use super::{
    KnowledgeJobKind, KnowledgePipeline, KnowledgeWorker, KnowledgeWorkerConfig,
    KnowledgeWorkerHandle, KnowledgeWorkerStatus, start_knowledge_worker,
};

pub type ConsolidationWorker = KnowledgeWorker;
pub type ConsolidationWorkerConfig = KnowledgeWorkerConfig;
pub type ConsolidationWorkerHandle = KnowledgeWorkerHandle;
pub type ConsolidationWorkerStatus = KnowledgeWorkerStatus;

#[must_use]
pub fn start_consolidation_worker(
    pipeline: Arc<KnowledgePipeline>,
    config: ConsolidationWorkerConfig,
) -> ConsolidationWorkerHandle {
    start_knowledge_worker(
        pipeline,
        KnowledgeJobKind::ConsolidateKnowledge,
        "consolidation",
        config,
    )
}
