//! Persistent memory repository contract.
//!
//! Memory deliberately has its own repository boundary: session storage owns
//! rollouts and WAL replay, while this trait owns durable memory state and the
//! candidate semantic observations that will later be grounded into KMS.

use async_trait::async_trait;
use uuid::Uuid;

use crate::storage::StorageError;

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
pub struct MemoryEntry {
    pub id: Uuid,
    pub scope_id: Uuid,
    pub entry_type: String,
    pub title: String,
    pub body_md: String,
    pub status: String,
    pub confidence: f64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MemorySummary {
    pub scope_id: Uuid,
    pub schema_version: String,
    pub summary_md: String,
    pub source_hash: String,
    pub generated_at: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MemoryNote {
    pub id: Uuid,
    pub scope_id: Uuid,
    pub slug: String,
    pub content: String,
    pub status: String,
    pub created_at: i64,
}

/// Candidate subject-predicate-object observation. Observations are retained
/// independently from KMS so consolidation can validate and ground them before
/// promoting semantic facts into the knowledge graph.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SemanticObservation {
    pub id: Uuid,
    pub scope_id: Uuid,
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

#[async_trait]
pub trait MemoryStore: Send + Sync {
    async fn get_stage1_output(
        &self,
        scope_id: Uuid,
        session_id: Uuid,
    ) -> Result<Option<MemoryStage1Record>, StorageError>;

    async fn claim_stage1(
        &self,
        scope_id: Uuid,
        session_id: Uuid,
        source_hash: &str,
        lease_until: i64,
    ) -> Result<bool, StorageError>;

    async fn complete_stage1(
        &self,
        scope_id: Uuid,
        output: MemoryStage1Record,
    ) -> Result<(), StorageError>;

    async fn fail_stage1(
        &self,
        scope_id: Uuid,
        session_id: Uuid,
        source_hash: &str,
        error: &str,
    ) -> Result<(), StorageError>;

    async fn list_stage1_outputs(
        &self,
        scope_id: Uuid,
        limit: usize,
    ) -> Result<Vec<MemoryStage1Record>, StorageError>;

    async fn claim_phase2(
        &self,
        scope_id: Uuid,
        source_hash: &str,
        lease_until: i64,
    ) -> Result<bool, StorageError>;

    async fn complete_phase2(
        &self,
        scope_id: Uuid,
        source_hash: &str,
        entries: Vec<MemoryEntry>,
        summary_md: &str,
        observations: Vec<SemanticObservation>,
        consumed_note_ids: Vec<Uuid>,
    ) -> Result<(), StorageError>;

    async fn fail_phase2(
        &self,
        scope_id: Uuid,
        source_hash: &str,
        error: &str,
    ) -> Result<(), StorageError>;

    async fn get_summary(&self, scope_id: Uuid) -> Result<Option<MemorySummary>, StorageError>;

    async fn list_entries(
        &self,
        scope_id: Uuid,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, StorageError>;

    async fn get_entry(
        &self,
        scope_id: Uuid,
        entry_id: Uuid,
    ) -> Result<Option<MemoryEntry>, StorageError>;

    async fn list_pending_notes(&self, scope_id: Uuid) -> Result<Vec<MemoryNote>, StorageError>;

    async fn insert_note(
        &self,
        scope_id: Uuid,
        slug: &str,
        content: &str,
    ) -> Result<MemoryNote, StorageError>;

    async fn list_observations(
        &self,
        scope_id: Uuid,
        status: &str,
        limit: usize,
    ) -> Result<Vec<SemanticObservation>, StorageError>;

    async fn set_observation_status(
        &self,
        scope_id: Uuid,
        observation_id: Uuid,
        status: &str,
        error: Option<&str>,
    ) -> Result<(), StorageError>;
}
