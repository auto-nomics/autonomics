use std::sync::Arc;

mod consolidation;
mod diagnose;
mod ingest_observation;
mod propose_skill;
mod repair;
mod session_extract;

use crate::pipeline::stage::KnowledgeStage;
pub use consolidation::Phase2Stage;
pub use diagnose::DiagnoseKnowledgeNodeStage;
pub use ingest_observation::IngestObservationStage;
pub use propose_skill::ProposeSkillRevisionStage;
pub use repair::RepairKnowledgeNodeStage;
pub use session_extract::SessionExtractStage;

#[must_use]
pub fn default_stages() -> Vec<Arc<dyn KnowledgeStage>> {
    vec![
        Arc::new(SessionExtractStage),
        Arc::new(Phase2Stage),
        Arc::new(IngestObservationStage),
        Arc::new(ProposeSkillRevisionStage),
        Arc::new(DiagnoseKnowledgeNodeStage),
        Arc::new(RepairKnowledgeNodeStage),
    ]
}
