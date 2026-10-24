//! Event-driven knowledge extraction and consolidation pipeline.

use std::sync::Arc;

use agentik_sdk::model::Model;
use arc_swap::ArcSwapOption;
use uuid::Uuid;

use super::MemoryBackend;
use super::MemoryConfig;
use crate::source::MemorySource;

mod error;
mod event;
mod grounding;
mod job;
mod phase1;
mod phase2;
mod prompting;
mod service;
mod stage;
mod stages;

use error::Result;

/// All root agents share one global memory scope. Scope partitioning can be
/// introduced later without changing the repository contract.
pub const MEMORY_SCOPE_ID: Uuid = Uuid::nil();

pub use crate::store::KnowledgeQueue;
pub use error::Error as PipelineError;
pub use event::KnowledgeEvent;
pub use job::KnowledgeJob;
pub use service::KnowledgePipeline;
pub use stage::{KnowledgeStage, StageContext, StageOutcome, StageStatus};

pub async fn run_memory_pipeline(
    agent_id: Uuid,
    storage: Arc<dyn MemorySource>,
    memory: Arc<MemoryBackend>,
    model_handle: Arc<ArcSwapOption<Model>>,
) {
    if let Some(queue) = memory.pipeline_queue() {
        let pipeline = KnowledgePipeline::new(queue, storage, Arc::clone(&memory), model_handle);
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
        return;
    }

    let config = memory.effective_memory_config();
    if let Err(error) = run_pipeline(agent_id, &storage, &memory, &model_handle, &config).await {
        tracing::warn!(agent_id = %agent_id, error = %error, "memory pipeline failed");
    }
}

async fn run_pipeline(
    agent_id: Uuid,
    storage: &Arc<dyn MemorySource>,
    memory: &Arc<MemoryBackend>,
    model_handle: &Arc<ArcSwapOption<Model>>,
    config: &MemoryConfig,
) -> Result<()> {
    phase1::run(agent_id, storage, &memory.store, model_handle, config).await?;
    phase2::run(&memory.store, model_handle, config).await?;
    if let Some(semantic_grounding) = &memory.grounding {
        grounding::run(memory, semantic_grounding).await?;
    }
    Ok(())
}
