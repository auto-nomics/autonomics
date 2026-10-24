//! Event-driven knowledge extraction and consolidation pipeline.

use std::sync::Arc;

use agentik_sdk::model::Model;
use arc_swap::ArcSwapOption;
use uuid::Uuid;

use super::KmsAgentHandle;
use crate::source::MemorySource;

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

pub use crate::store::KnowledgeQueue;
pub use error::Error as PipelineError;
pub use event::KnowledgeEvent;
pub use job::KnowledgeJob;
pub use service::{KnowledgePipeline, KnowledgeRunSummary};
pub use session_extract_worker::{
    SessionExtractWorker, SessionExtractWorkerConfig, SessionExtractWorkerHandle,
    SessionExtractWorkerStatus, start_session_extract_worker,
};
pub use stage::{KnowledgeStage, StageContext, StageOutcome, StageStatus};

pub async fn run_memory_pipeline(
    agent_id: Uuid,
    storage: Arc<dyn MemorySource>,
    memory: Arc<KmsAgentHandle>,
    model_handle: Arc<ArcSwapOption<Model>>,
) {
    let config = memory.effective_memory_config();
    if !config.generate_memory {
        return;
    }

    let system = memory.system();
    let pipeline =
        KnowledgePipeline::new(system.queue(), storage, Arc::clone(&system), model_handle);
    let dispatch_result = pipeline
        .dispatch(KnowledgeEvent::AgentStarted { agent_id })
        .await;
    let result = match dispatch_result {
        Ok(_) => pipeline.run_due(16).await,
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        tracing::warn!(agent_id = %agent_id, error = %error, "knowledge pipeline failed");
    }
}
