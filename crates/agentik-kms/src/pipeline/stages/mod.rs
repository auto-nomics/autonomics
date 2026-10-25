use std::sync::Arc;

mod consolidation;
mod diagnose;
mod session_extract;

use crate::pipeline::stage::KnowledgeStage;
pub use consolidation::Phase2Stage;
pub use diagnose::DiagnoseKnowledgeNodeStage;
pub use session_extract::SessionExtractStage;

#[must_use]
pub fn default_stages() -> Vec<Arc<dyn KnowledgeStage>> {
    vec![
        Arc::new(SessionExtractStage),
        Arc::new(Phase2Stage),
        Arc::new(DiagnoseKnowledgeNodeStage),
    ]
}
