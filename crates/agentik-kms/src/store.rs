//! Persistent memory repository contract.
//!
//! Memory deliberately has its own repository boundary: session storage owns
//! rollouts and WAL replay, while this trait owns durable memory state and the
//! candidate semantic observations that will later be grounded into KMS.

pub mod error;
mod knowledge_store;
mod queue_store;
pub mod turso_store;

use async_trait::async_trait;
use uuid::Uuid;

use crate::knowledge::Knowledge;
use error::Result;

pub use knowledge_store::KnowledgeStore;
pub use queue_store::{KnowledgeQueue, QueuedKnowledgeJob};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MemoryStage1Record {
    pub session_id: Uuid,
    pub source_hash: String,
    pub raw_memory: String,
    pub rollout_summary: String,
    pub rollout_slug: Option<String>,
    pub status: String,
    pub generated_at: i64,
    pub lease_until: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MemorySummary {
    pub schema_version: String,
    pub summary_md: String,
    pub source_hash: String,
    pub generated_at: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MemoryNote {
    pub id: Uuid,
    pub slug: String,
    pub content: String,
    pub status: String,
    pub created_at: i64,
}

/// Source-normalized evidence retained for deterministic consolidation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct KnowledgeEvidence {
    pub id: String,
    pub source_kind: String,
    pub source_id: String,
    pub evidence_kind: String,
    pub summary: String,
    pub body: String,
    pub anchor_path: Option<String>,
    pub error: Option<String>,
    pub content_hash: String,
    pub status: String,
    pub occurrence_count: u64,
    pub created_at: i64,
    pub last_seen_at: i64,
}

/// One immutable imported skill version.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SkillVersion {
    pub id: Uuid,
    pub skill_name: String,
    pub node_path: String,
    pub version: u32,
    pub status: String,
    pub title: String,
    pub body_md: String,
    pub metadata_json: String,
    pub content_hash: String,
    pub source_tier: String,
    pub source_path: Option<String>,
    pub created_at: i64,
}

/// A text artifact attached to a skill version.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SkillArtifact {
    pub id: Uuid,
    pub skill_version_id: Uuid,
    pub relative_path: String,
    pub content: String,
    pub content_hash: String,
    pub created_at: i64,
}

/// A durable skill revision awaiting human approval.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SkillProposal {
    pub id: Uuid,
    pub skill_name: String,
    pub node_path: String,
    pub status: String,
    pub title: String,
    pub body_md: String,
    pub metadata_json: String,
    pub rationale: String,
    pub source_evidence_ids: Vec<String>,
    pub artifacts: Vec<crate::artifacts::KnowledgeSkillArtifactInput>,
    pub content_hash: String,
    pub approved_skill_version_id: Option<Uuid>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Candidate subject-predicate-object observation. Observations are retained
/// independently from KMS so consolidation can validate and ground them before
/// promoting semantic facts into the knowledge graph.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SemanticObservation {
    pub id: Uuid,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub content: String,
    pub status: String,
    pub confidence: f64,
    pub source_hash: String,
    pub created_at: i64,
    pub last_error: Option<String>,
}

/// A persisted diagnosis for one knowledge node and its direct children.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct KnowledgeDiagnosticFinding {
    pub id: Uuid,
    pub target_path: String,
    pub action: String,
    pub severity: String,
    pub status: String,
    pub message: String,
    pub recommendation: String,
    pub proposed_paths: Vec<String>,
    pub confidence: f64,
    pub family_hash: String,
    pub created_at: i64,
}

#[async_trait]
pub trait MemoryStore: KnowledgeStore + Send + Sync {
    async fn get_stage1_output(&self, session_id: Uuid) -> Result<Option<MemoryStage1Record>>;

    async fn claim_stage1(
        &self,
        session_id: Uuid,
        source_hash: &str,
        lease_until: i64,
    ) -> Result<bool>;

    async fn complete_stage1(&self, output: MemoryStage1Record) -> Result<()>;

    async fn fail_stage1(&self, session_id: Uuid, source_hash: &str, error: &str) -> Result<()>;

    async fn list_stage1_outputs(&self, limit: usize) -> Result<Vec<MemoryStage1Record>>;

    async fn claim_phase2(&self, source_hash: &str, lease_until: i64) -> Result<bool>;

    async fn complete_phase2(
        &self,
        source_hash: &str,
        nodes: Vec<Knowledge>,
        summary_md: &str,
        observations: Vec<SemanticObservation>,
        consumed_note_ids: Vec<Uuid>,
    ) -> Result<()>;

    async fn fail_phase2(&self, source_hash: &str, error: &str) -> Result<()>;

    async fn get_summary(&self) -> Result<Option<MemorySummary>>;

    async fn list_pending_notes(&self) -> Result<Vec<MemoryNote>>;

    async fn insert_note(&self, slug: &str, content: &str) -> Result<MemoryNote>;

    async fn upsert_evidence(
        &self,
        input: crate::artifacts::KnowledgeEvidenceInput,
    ) -> Result<KnowledgeEvidence>;

    async fn list_pending_evidence(&self, limit: usize) -> Result<Vec<KnowledgeEvidence>>;

    async fn list_knowledge_evidence(&self, limit: usize) -> Result<Vec<KnowledgeEvidence>>;

    async fn mark_evidence_consumed(&self, evidence_ids: Vec<String>) -> Result<()>;

    async fn has_evidence_source(&self, source_kind: &str, source_id: &str) -> Result<bool>;

    async fn import_skill_version(
        &self,
        input: crate::artifacts::KnowledgeSkillVersionInput,
    ) -> Result<Option<SkillVersion>>;

    async fn list_skill_versions(&self, limit: usize) -> Result<Vec<SkillVersion>>;

    async fn list_skill_artifacts(&self, skill_version_id: Uuid) -> Result<Vec<SkillArtifact>>;

    async fn create_skill_proposal(
        &self,
        input: crate::artifacts::KnowledgeSkillProposalInput,
    ) -> Result<Option<SkillProposal>>;

    async fn list_skill_proposals(&self, limit: usize) -> Result<Vec<SkillProposal>>;

    async fn approve_skill_proposal(&self, proposal_id: Uuid) -> Result<Option<SkillVersion>>;

    async fn reject_skill_proposal(&self, proposal_id: Uuid) -> Result<bool>;

    async fn list_observations(
        &self,
        status: &str,
        limit: usize,
    ) -> Result<Vec<SemanticObservation>>;

    async fn set_observation_status(
        &self,
        observation_id: Uuid,
        status: &str,
        error: Option<&str>,
    ) -> Result<()>;
}
