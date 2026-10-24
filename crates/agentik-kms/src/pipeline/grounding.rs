use std::sync::Arc;

use super::MEMORY_SCOPE_ID;
use super::error::{Error, Result};
use crate::SemanticGrounding;

pub(super) async fn run(
    memory: &Arc<crate::MemoryBackend>,
    semantic_grounding: &Arc<dyn SemanticGrounding>,
) -> Result<()> {
    let observations = memory
        .store
        .list_observations(MEMORY_SCOPE_ID, "candidate", 1000)
        .await
        .map_err(|e| Error::store("list candidate semantic observations", e))?;
    for observation in observations {
        match semantic_grounding.ground(&observation).await {
            Ok(outcome) => {
                let status = if outcome.accepted {
                    "accepted"
                } else {
                    "rejected"
                };
                let reason = (!outcome.accepted).then_some(outcome.reason);
                memory
                    .store
                    .set_observation_status(
                        MEMORY_SCOPE_ID,
                        observation.id,
                        status,
                        reason.as_deref(),
                    )
                    .await
                    .map_err(|e| Error::store("set observation status", e))?;
            }
            Err(error) => {
                memory
                    .store
                    .set_observation_status(MEMORY_SCOPE_ID, observation.id, "error", Some(&error))
                    .await
                    .map_err(|e| Error::store("set observation error status", e))?;
            }
        }
    }
    Ok(())
}
