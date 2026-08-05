//! Agent persistence: unified storage trait, data types, and restore helpers.
//!
//! Three persistence layers:
//! - **Snapshot** — full `Memory` checkpoints at session boundaries.
//! - **Session log (WAL)** — incremental message append for fast recovery.
//! - **Agent registry** — which agents exist + their topology (relations).

pub mod turso_storage;

use async_trait::async_trait;
use thiserror::Error;
use uuid::Uuid;

use agentik_sdk::types::messages::Message;

use crate::{lifecycle::AgentLifecycleStatus, memory::Memory};

// ═══════════════════════════════════════════════════════════════════════
// Data types
// ═══════════════════════════════════════════════════════════════════════

/// One full-memory checkpoint. Stored in the `snapshots` table.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentSnapshot {
    pub snapshot_id: Uuid,
    pub ts: i64,
    pub agent_id: Uuid,
    pub agent_status: AgentLifecycleStatus,
    pub memory: Memory,
}

/// One row in the `agents` registry table.
///
/// `config_json` holds the serialized agent configuration (e.g.
/// `RuntimeConfig`) as an opaque JSON value to avoid a circular dependency
/// between `agentik-core` and the `runtime` crate.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentRecord {
    pub id: Uuid,
    pub name: String,
    pub config_json: serde_json::Value,
    pub created_at: i64,
    pub last_active: i64,
}

/// One row in the `agent_relations` table.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentRelation {
    pub parent_id: Uuid,
    pub child_id: Uuid,
    pub kind: RelationKind,
}

/// The kind of relationship between two agents.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum RelationKind {
    Spawned,
    Delegated,
    Parallel,
}

impl RelationKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Spawned => "spawned",
            Self::Delegated => "delegated",
            Self::Parallel => "parallel",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "spawned" => Some(Self::Spawned),
            "delegated" => Some(Self::Delegated),
            "parallel" => Some(Self::Parallel),
            _ => None,
        }
    }
}

/// Operations sent through the async persistence channel (WAL).
///
/// `Memory::remember()` pushes `AppendMessage` ops; the agent loop pushes
/// `StartSession` / `EndSession` at session boundaries. A background worker
/// drains the channel and writes to the database without blocking the LLM
/// loop.
#[derive(Debug, Clone)]
pub enum PersistOp {
    StartSession { agent_id: Uuid, session_id: Uuid },
    AppendMessage { session_id: Uuid, message: Message },
    EndSession { session_id: Uuid },
}

// ═══════════════════════════════════════════════════════════════════════
// Agent Profile (blueprint / preset)
// ═══════════════════════════════════════════════════════════════════════

/// A named, persisted agent configuration template that can be instantiated
/// into a running [`Agent`](crate::Agent).
///
/// Unlike [`AgentRecord`] (which tracks *runtime state* — memory, sessions,
/// last-active), an `AgentProfile` is a **blueprint**: it defines what an
/// agent *is* (identity, prompts, tool capabilities, model preference) but
/// holds no conversation history.
///
/// Users create profiles in the UI, pick one to instantiate, and the
/// resulting running agent gets its own `AgentRecord` for persistence.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentProfile {
    pub id: Uuid,
    pub name: String,
    pub description: String,

    // ── Prompt ──
    pub agent_identity: String,
    /// Long-form system prompt. `None` means "use built-in default".
    pub system_prompt: Option<String>,

    // ── Tool capability flags ──
    pub enable_bibliography: bool,
    pub enable_opengwas: bool,
    pub enable_opentargets: bool,
    pub enable_gwascatalog: bool,
    pub enable_iceberg: bool,
    pub enable_dag_history: bool,

    // ── Model preference ──
    /// Preferred model in `"provider:model"` format, or `None` to use the
    /// global default.
    #[serde(default)]
    pub preferred_model: Option<String>,

    pub created_at: i64,
    pub updated_at: i64,
}

impl AgentProfile {
    /// Create a new profile with sensible defaults (all tools enabled).
    pub fn new(name: impl Into<String>) -> Self {
        let now = now_ms();
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            description: String::new(),
            agent_identity: "You are a helpful assistant.".into(),
            system_prompt: None,
            enable_bibliography: true,
            enable_opengwas: true,
            enable_opentargets: true,
            enable_gwascatalog: true,
            enable_iceberg: true,
            enable_dag_history: true,
            preferred_model: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Return the built-in default profiles seeded on first run.
    pub fn defaults() -> Vec<AgentProfile> {
        let now = now_ms();
        vec![
            AgentProfile {
                id: Uuid::new_v4(),
                name: "default".into(),
                description: "Full-featured biomedical research assistant.".into(),
                agent_identity: "You are a biomedical research assistant specializing \
                    in genomics, GWAS analysis, and literature mining.".into(),
                system_prompt: None,
                enable_bibliography: true,
                enable_opengwas: true,
                enable_opentargets: true,
                enable_gwascatalog: true,
                enable_iceberg: true,
                enable_dag_history: true,
                preferred_model: None,
                created_at: now,
                updated_at: now,
            },
            AgentProfile {
                id: Uuid::new_v4(),
                name: "literature".into(),
                description: "Literature search and evidence synthesis expert.".into(),
                agent_identity: "You are a literature search expert specializing in \
                    systematic reviews, meta-analyses, and evidence synthesis. \
                    Use PubMed, Embase, and bioRxiv tools to find and analyze publications.".into(),
                system_prompt: None,
                enable_bibliography: true,
                enable_opengwas: false,
                enable_opentargets: true,
                enable_gwascatalog: false,
                enable_iceberg: false,
                enable_dag_history: false,
                preferred_model: None,
                created_at: now,
                updated_at: now,
            },
            AgentProfile {
                id: Uuid::new_v4(),
                name: "gwas-analysis".into(),
                description: "GWAS data analysis and statistical genetics expert.".into(),
                agent_identity: "You are a GWAS analysis expert specializing in \
                    statistical genetics. Use OpenGWAS, GWAS Catalog, and the \
                    data pipeline engine to analyze genetic association data.".into(),
                system_prompt: None,
                enable_bibliography: false,
                enable_opengwas: true,
                enable_opentargets: true,
                enable_gwascatalog: true,
                enable_iceberg: true,
                enable_dag_history: true,
                preferred_model: None,
                created_at: now,
                updated_at: now,
            },
        ]
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// ═══════════════════════════════════════════════════════════════════════
// Error
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("agent not found: {0}")]
    AgentNotFound(Uuid),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("database error: {0}")]
    Database(#[from] turso::Error),
    #[error("storage error: {0}")]
    Other(#[from] Box<dyn std::error::Error + Send + Sync>),
}

// ═══════════════════════════════════════════════════════════════════════
// Unified storage trait
// ═══════════════════════════════════════════════════════════════════════

/// Unified storage trait combining snapshot, registry, and session-log
/// persistence. Every method returns [`StorageError`] so that `dyn
/// AgentStorage` gives callers direct access to the full API surface without
/// trait-upcasting.
#[async_trait]
pub trait AgentStorage: Send + Sync {
    // ── Snapshot ─────────────────────────────────────────────

    async fn create_snapshot(&self, snapshot: AgentSnapshot) -> Result<(), StorageError>;
    async fn get_snapshot(&self, snapshot_id: Uuid) -> Result<AgentSnapshot, StorageError>;
    async fn get_agent_snapshots(&self, agent_id: Uuid) -> Result<Vec<AgentSnapshot>, StorageError>;
    async fn get_latest_snapshot(
        &self,
        agent_id: Uuid,
    ) -> Result<Option<AgentSnapshot>, StorageError>;
    async fn list_all_agent_ids(&self) -> Result<Vec<Uuid>, StorageError>;
    async fn delete_agent_snapshots(&self, agent_id: Uuid) -> Result<usize, StorageError>;

    // ── Registry ─────────────────────────────────────────────

    async fn upsert_agent(&self, record: AgentRecord) -> Result<(), StorageError>;
    async fn get_agent(&self, agent_id: Uuid) -> Result<Option<AgentRecord>, StorageError>;
    async fn get_agent_by_name(&self, name: &str) -> Result<Option<AgentRecord>, StorageError>;
    async fn list_agents(&self) -> Result<Vec<AgentRecord>, StorageError>;
    async fn delete_agent(&self, agent_id: Uuid) -> Result<(), StorageError>;
    async fn add_relation(&self, relation: AgentRelation) -> Result<(), StorageError>;
    async fn list_children(&self, agent_id: Uuid) -> Result<Vec<AgentRelation>, StorageError>;
    async fn list_parents(&self, agent_id: Uuid) -> Result<Vec<AgentRelation>, StorageError>;
    async fn touch_agent(&self, agent_id: Uuid) -> Result<(), StorageError>;

    // ── Session log (WAL) ────────────────────────────────────

    async fn start_session(
        &self,
        agent_id: Uuid,
        session_id: Uuid,
    ) -> Result<(), StorageError>;
    async fn append_message(
        &self,
        session_id: Uuid,
        message: &Message,
    ) -> Result<(), StorageError>;
    async fn end_session(&self, session_id: Uuid) -> Result<(), StorageError>;
    async fn get_messages_since(
        &self,
        agent_id: Uuid,
        ts: i64,
    ) -> Result<Vec<Message>, StorageError>;
}

// ═══════════════════════════════════════════════════════════════════════
// Agent Profile Registry trait
// ═══════════════════════════════════════════════════════════════════════

/// CRUD for [`AgentProfile`] blueprints. This is a separate trait from
/// [`AgentStorage`] because profiles are *definitions* (user-authored
/// templates) rather than *runtime state* (memory, sessions). A storage
/// backend can implement both traits on the same connection.
#[async_trait]
pub trait AgentProfileRegistry: Send + Sync {
    async fn create_profile(&self, profile: AgentProfile) -> Result<(), StorageError>;
    async fn get_profile(&self, id: Uuid) -> Result<Option<AgentProfile>, StorageError>;
    async fn get_profile_by_name(&self, name: &str) -> Result<Option<AgentProfile>, StorageError>;
    async fn list_profiles(&self) -> Result<Vec<AgentProfile>, StorageError>;
    async fn update_profile(&self, profile: AgentProfile) -> Result<(), StorageError>;
    async fn delete_profile(&self, id: Uuid) -> Result<(), StorageError>;

    /// Seed the default profiles if the table is empty. Returns `true` if
    /// profiles were inserted.
    async fn seed_defaults_if_empty(&self) -> Result<bool, StorageError> {
        let existing = self.list_profiles().await?;
        if !existing.is_empty() {
            return Ok(false);
        }
        for profile in AgentProfile::defaults() {
            self.create_profile(profile).await?;
        }
        Ok(true)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Restore helper
// ═══════════════════════════════════════════════════════════════════════

/// Reconstruct an agent's [`Memory`] from storage.
///
/// Loads the latest snapshot as a base, then replays all messages persisted
/// after the snapshot's timestamp. The snapshot's `ts` acts as a watermark:
/// any message with `ts > snapshot.ts` was appended to the WAL after the
/// snapshot was taken and therefore is not yet in the snapshot's memory.
pub async fn restore_memory(
    storage: &dyn AgentStorage,
    agent_id: Uuid,
) -> Result<Memory, StorageError> {
    let snapshot = storage.get_latest_snapshot(agent_id).await?;
    let snapshot_ts = snapshot.as_ref().map(|s| s.ts).unwrap_or(0);
    // Use Memory::new() (which seeds one empty MemoryItem) instead of
    // Memory::default() (which has an empty items vector). Without this,
    // the first remember() call panics with EmptyMemoryItem.
    let mut memory = snapshot.map(|s| s.memory).unwrap_or_else(Memory::new);
    // Defensive: even a restored snapshot might have an empty items vector
    // (e.g. from a corrupt or edge-case state). Ensure at least one segment.
    if memory.items.is_empty() {
        memory.items.push(crate::memory::MemoryItem::default());
    }
    let messages = storage.get_messages_since(agent_id, snapshot_ts).await?;
    for msg in messages {
        let _ = memory.remember(msg);
    }
    Ok(memory)
}
