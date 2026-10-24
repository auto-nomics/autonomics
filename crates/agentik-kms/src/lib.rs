//! Unified knowledge, memory, and skill management.
//!
//! The crate currently owns the durable knowledge tree and the extraction
//! pipeline that turns agent rollouts and explicit notes into wiki-style
//! knowledge nodes. Legacy KMS and skill subsystems can migrate onto these
//! contracts without becoming dependencies of `agentik-core`.

pub mod error;
pub mod source;

mod artifacts;
mod grounding;
mod knowledge;
mod pipeline;
mod store;
mod system;

pub use artifacts::{
    KnowledgeDraft, MemoryConsolidation, MemoryExtraction, SemanticObservationDraft,
    parse_json_object, redact_secrets,
};
pub use error::{Error, Result};
pub use grounding::{SemanticGrounding, SemanticGroundingOutcome};
pub use knowledge::{Knowledge, KnowledgePath};
pub use pipeline::{
    KnowledgeEvent, KnowledgeJob, KnowledgePipeline, KnowledgeQueue, KnowledgeStage,
    MEMORY_SCOPE_ID, PipelineError, StageContext, StageOutcome, StageStatus, run_memory_pipeline,
};
pub use source::{MemorySessionRecord, MemorySource};
pub use store::error::Error as StoreError;
pub use store::turso_store::TursoKmsStorage;
pub use store::{
    KnowledgeStore, MemoryNote, MemoryStage1Record, MemoryStore, MemorySummary, QueuedKnowledgeJob,
    SemanticObservation,
};
pub use system::{KmsAgentHandle, KmsSystem};

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
