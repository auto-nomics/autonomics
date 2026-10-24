pub mod error;
pub mod source;

mod artifacts;
mod backend;
mod grounding;
mod knowledge;
mod pipeline;
mod store;

pub use artifacts::{
    KnowledgeDraft, MemoryConsolidation, MemoryExtraction, SemanticObservationDraft,
    parse_json_object, redact_secrets,
};
pub use backend::{MemoryBackend, memory_prompt_section};
pub use error::{Error, Result};
pub use grounding::{SemanticGrounding, SemanticGroundingOutcome};
pub use knowledge::{Knowledge, KnowledgePath};
pub use pipeline::{MEMORY_SCOPE_ID, run_memory_pipeline};
pub use source::{MemorySessionRecord, MemorySource};
pub use store::error::Error as StoreError;
pub use store::turso_store::TursoMemoryStorage;
pub use store::{
    KnowledgeStore, MemoryNote, MemoryStage1Record, MemoryStore, MemorySummary, SemanticObservation,
};

/// Fully resolved runtime settings carried by the memory backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentRuntimeConfig {
    pub use_memory: bool,
    pub generate_memory: bool,
    pub reasoning_effort: Option<agentik_types::ReasoningEffort>,
}

impl AgentRuntimeConfig {
    #[must_use]
    pub fn new(use_memory: bool, generate_memory: bool) -> Self {
        Self {
            use_memory,
            generate_memory,
            reasoning_effort: None,
        }
    }
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MemoryConfig {
    pub use_memory: bool,
    pub generate_memory: bool,
    pub reasoning_effort: Option<agentik_types::ReasoningEffort>,
    pub max_source_sessions: usize,
    pub max_age_days: i64,
    pub min_idle_hours: i64,
    pub phase2_inputs: usize,
    pub summary_max_bytes: usize,
    pub lease_seconds: i64,
}

impl MemoryConfig {
    #[must_use]
    pub fn new() -> Self {
        Self {
            use_memory: true,
            generate_memory: true,
            reasoning_effort: None,
            max_source_sessions: 20,
            max_age_days: 90,
            min_idle_hours: 12,
            phase2_inputs: 100,
            summary_max_bytes: 16 * 1024,
            lease_seconds: 3600,
        }
    }

    #[must_use]
    pub fn is_root_agent(&self, path: &agentik_types::AgentPath) -> bool {
        path.segments().len() <= 2
    }
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self::new()
    }
}
