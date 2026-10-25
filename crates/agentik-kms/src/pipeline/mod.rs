//! Event-driven knowledge extraction and consolidation pipeline.

mod consolidation_worker;
mod error;
mod event;
mod job;
mod phase2;
mod prompting;
mod service;
mod session_extract;
mod session_extract_worker;
mod stage;
mod stages;
mod worker;

pub use crate::store::KnowledgeQueue;
pub use consolidation_worker::{
    ConsolidationWorker, ConsolidationWorkerConfig, ConsolidationWorkerHandle,
    ConsolidationWorkerStatus, start_consolidation_worker,
};
pub use error::Error as PipelineError;
pub use event::KnowledgeEvent;
pub use job::{KnowledgeJob, KnowledgeJobKind};
pub use service::{KnowledgePipeline, KnowledgeRunSummary};
pub use session_extract_worker::{
    SessionExtractWorker, SessionExtractWorkerConfig, SessionExtractWorkerHandle,
    SessionExtractWorkerStatus, start_session_extract_worker,
};
pub use stage::{KnowledgeStage, StageContext, StageOutcome, StageStatus};
pub use worker::{
    KnowledgeWorker, KnowledgeWorkerConfig, KnowledgeWorkerHandle, KnowledgeWorkerStatus,
    start_knowledge_worker,
};
