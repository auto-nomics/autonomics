//! Two-phase memory extraction and consolidation backed by a memory repository.

use std::sync::Arc;

use agentik_sdk::model::Model;
use arc_swap::ArcSwapOption;
use uuid::Uuid;

use super::MemoryBackend;
use super::MemoryConfig;
use crate::source::MemorySource;

mod error;
mod grounding;
mod phase1;
mod phase2;
mod prompting;

use error::Result;

/// All root agents share one global memory scope. Scope partitioning can be
/// introduced later without changing the repository contract.
pub const MEMORY_SCOPE_ID: Uuid = Uuid::nil();

pub async fn run_memory_pipeline(
    agent_id: Uuid,
    storage: Arc<dyn MemorySource>,
    memory: Arc<MemoryBackend>,
    model_handle: Arc<ArcSwapOption<Model>>,
) {
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
