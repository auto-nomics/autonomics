//! Event-driven knowledge extraction and consolidation pipeline.

mod error;
mod event;
mod job;
mod phase2;
mod prompting;
mod service;
mod session_extract;
mod stage;
mod stages;
mod worker;

pub use crate::store::KnowledgeQueue;
pub use error::Error as PipelineError;
pub use event::KnowledgeEvent;
pub use job::{KnowledgeJob, KnowledgeJobKind};
pub use service::{KnowledgePipeline, KnowledgeRunSummary};
pub use stage::{KnowledgeStage, StageContext, StageOutcome, StageStatus};
pub use worker::{
    KnowledgeWorker, KnowledgeWorkerConfig, KnowledgeWorkerHandle, KnowledgeWorkerStatus,
    start_knowledge_worker,
};
