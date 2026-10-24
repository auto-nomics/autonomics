use std::sync::Arc;

use super::MEMORY_SCOPE_ID;
use super::error::{Error, Result};
use crate::KmsSystem;
use crate::SemanticGrounding;

pub(super) async fn run(
    system: &Arc<KmsSystem>,
    semantic_grounding: &Arc<dyn SemanticGrounding>,
) -> Result<()> {
    let store = system.store();
    let observations = store
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
                store
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
                store
                    .set_observation_status(MEMORY_SCOPE_ID, observation.id, "error", Some(&error))
                    .await
                    .map_err(|e| Error::store("set observation error status", e))?;
            }
        }
    }
    Ok(())
}
