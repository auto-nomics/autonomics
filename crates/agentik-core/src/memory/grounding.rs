//! Bridge from persistent-memory observations to a semantic knowledge system.

use async_trait::async_trait;

use super::SemanticObservation;

/// Outcome of validating and promoting one candidate semantic observation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SemanticGroundingOutcome {
    pub accepted: bool,
    pub reason: String,
}

/// Implementations write validated observations into an external semantic
/// system (KMS in the runtime crate) and must clean up partial writes when a
/// diagnostic check fails.
#[async_trait]
pub trait SemanticGrounding: Send + Sync {
    async fn ground(
        &self,
        observation: &SemanticObservation,
    ) -> Result<SemanticGroundingOutcome, String>;
}
