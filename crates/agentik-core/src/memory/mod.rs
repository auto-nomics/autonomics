//! Compatibility façade over the standalone memory crate.
//!
//! This module re-exports the domain implementation and supplies the
//! `agentik-core` storage/tool adapters that the standalone crate cannot know
//! about without a dependency cycle.

mod adapter;
mod tools;

pub use adapter::{AgentStorageMemorySource, agent_storage_memory_source};
pub use agentik_kms::{
    AgentRuntimeConfig, EmptyMemorySource, Error, KmsAgentHandle, KmsSystem, Knowledge,
    KnowledgeEvent, KnowledgePath, KnowledgeQueue, KnowledgeStore, MemoryConfig, MemoryNote,
    MemorySource, MemoryStage1Record, MemoryStore, MemorySummary, QueuedKnowledgeJob, Result,
    SemanticObservation, TursoKmsStorage, now_ms, redact_secrets,
};
pub use agentik_kms::{
    KmsWorkerStatusSnapshot, KnowledgeEvidenceInput, KnowledgeJobKind, KnowledgeSkillArtifactInput,
    KnowledgeSkillProposalInput, KnowledgeSkillVersionInput, KnowledgeWorkerEvent,
    KnowledgeWorkerState, KnowledgeWorkerStatus,
};
pub use agentik_kms::{SkillArtifact, SkillProposal, SkillVersion};
pub use tools::memory_registrations;
