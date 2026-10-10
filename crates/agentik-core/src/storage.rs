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
use agentik_types::{AgentPlan, SessionTelemetry, TurnTelemetry};

use crate::lifecycle::AgentLifecycleStatus;
use crate::session::SessionState;

// ═══════════════════════════════════════════════════════════════════════
// Data types
// ═══════════════════════════════════════════════════════════════════════

/// One session-state checkpoint. Stored in the `snapshots` table.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentSnapshot {
    pub snapshot_id: Uuid,
    pub ts: i64,
    pub agent_id: Uuid,
    pub agent_status: AgentLifecycleStatus,
    /// Serialized session conversation state (messages + summaries).
    pub state: SessionState,
    /// Which session this snapshot belongs to.
    #[serde(default)]
    pub session_id: Option<Uuid>,
}

/// One row in the `agents` registry table.
///
/// `config_json` holds the serialized per-agent configuration
/// ([`crate::profile::AgentProfileConfig`]) as an opaque JSON value to
/// avoid a circular dependency between `agentik-core` and the `runtime`
/// crate. Rows written before the agent-kind refactor hold a full
/// serialized profile; `AgentProfileConfig::from_json` parses both shapes.
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

/// Runtime metadata for an active or recently-active agent in the
/// multi-agent host. Persisted by [`AgentStorage::upsert_agent_graph_entry`]
/// and updated on every status transition by
/// [`AgentStorage::update_agent_graph_status`].
///
/// Mirrors codex's `AgentGraphStore` design
/// (`codex-rs/agent-graph-store/src/store.rs:17-60`) but scoped to
/// autonomics: keyed by hierarchical `path` (not UUID) so the dashboard
/// can reconstruct the same view across process restarts.
///
/// The `status_json` field is stored as a raw JSON string (rather than
/// a typed `AgentStatus`) to avoid a `agentik-core` → `agentik-types`
/// dependency cycle. Callers in the `runtime` layer deserialize it back
/// to [`crate::runtime::control::AgentStatus`] (or whatever the live
/// enum looks like at the time).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PersistedAgentGraph {
    /// Full hierarchical agent path, e.g. `/root/researcher/worker`.
    /// Primary key.
    pub path: String,
    /// Parent agent's path, if this agent was spawned by another.
    /// `None` for root-level agents (e.g. `/root/...`).
    pub parent_path: Option<String>,
    /// Profile path used to instantiate the agent.
    pub profile_path: String,
    /// Runtime UUID of the underlying agent (mirrors `agents.id` so
    /// rollback / data migration can find the agent's own snapshot +
    /// session WAL even if the graph entry is stale).
    pub agent_id: Uuid,
    /// JSON-serialized runtime status. Forward-compatible: future
    /// status variants can be added without breaking the schema.
    pub status_json: String,
    /// One-line summary of the most recent event (tool name on
    /// AwaitingTool, error message on Failed, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_event: Option<String>,
    /// Unix-epoch millis when this entry was first written.
    pub created_at: i64,
    /// Unix-epoch millis when this entry was last updated (status change
    /// or registration refresh).
    pub updated_at: i64,
}

/// The daemon's currently-open multi-agent layout.
///
/// Unlike `agent_graph`, which can retain historical rows after abnormal
/// process exits, this snapshot names the exact set that should be restored on
/// the next daemon start. It is persisted as one atomic JSON document so a
/// crash can observe either the previous layout or the new layout, never a
/// mixture of registrations/removals.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AgentLayoutSnapshot {
    /// Monotonic writer-side version. Out-of-order persistence tasks compare
    /// revisions and never overwrite a newer snapshot with an older one.
    #[serde(default)]
    pub revision: u64,
    pub agents: Vec<PersistedAgentGraph>,
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
/// `Session::remember()` pushes `AppendMessage` ops; the agent loop pushes
/// `StartSession` / `EndSession` at session boundaries. A background worker
/// drains the channel and writes to the database without blocking the LLM
/// loop.
#[derive(Debug, Clone)]
pub enum PersistOp {
    StartSession {
        agent_id: Uuid,
        session_id: Uuid,
    },
    AppendMessage {
        session_id: Uuid,
        message: Message,
    },
    EndSession {
        session_id: Uuid,
    },
    /// Compaction replaced the session's message history in-place.
    /// The WAL must persist the new state so a crash after compaction
    /// doesn't restore stale pre-compaction messages.
    ReplaceSessionState {
        agent_id: Uuid,
        session_id: Uuid,
        state: SessionState,
    },
    /// Replace a session's cumulative telemetry counters.
    UpdateSessionTelemetry {
        session_id: Uuid,
        telemetry: SessionTelemetry,
    },
    /// Preserve the full user-facing transcript before compaction replaces the
    /// active model context.
    ArchiveTranscript {
        session_id: Uuid,
        messages: Vec<Message>,
    },
}

// ═══════════════════════════════════════════════════════════════════════
// Agent runtime overrides
// ═══════════════════════════════════════════════════════════════════════

/// Optional per-agent settings layered on the daemon-wide runtime defaults.
///
/// `None` means "inherit". The structure is deliberately shared by profiles,
/// the TUI, the gateway wire protocol, and headless embedders; new per-agent
/// settings should be added here rather than to a frontend-specific type.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRuntimeOverrides {
    /// Inject the persistent-memory summary and allow memory tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_memory: Option<bool>,
    /// Run persistent-memory extraction and consolidation for this agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generate_memory: Option<bool>,
}

/// Fully resolved per-agent settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentRuntimeConfig {
    pub use_memory: bool,
    pub generate_memory: bool,
}

impl AgentRuntimeConfig {
    #[must_use]
    pub fn new(use_memory: bool, generate_memory: bool) -> Self {
        Self {
            use_memory,
            generate_memory,
        }
    }
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
    async fn get_agent_snapshots(&self, agent_id: Uuid)
    -> Result<Vec<AgentSnapshot>, StorageError>;
    async fn get_latest_snapshot(
        &self,
        agent_id: Uuid,
    ) -> Result<Option<AgentSnapshot>, StorageError>;

    /// Get the latest snapshot for a specific session.
    async fn get_latest_snapshot_for_session(
        &self,
        agent_id: Uuid,
        session_id: Uuid,
    ) -> Result<Option<AgentSnapshot>, StorageError>;

    /// Get messages for a specific session since the given timestamp.
    async fn get_messages_since_for_session(
        &self,
        session_id: Uuid,
        ts: i64,
    ) -> Result<Vec<Message>, StorageError>;
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

    // ── Runtime agent graph (Phase 4) ────────────────────────
    //
    // Persists a historical graph projection of agents seen by the host,
    // keyed by hierarchical path (not UUID) so dashboards and history views
    // can reconstruct topology. Daemon restart uses the separate atomic
    // current-layout snapshot above instead of treating every graph row as
    // still-open.
    //
    // Update semantics: `upsert_agent_graph_entry` writes on spawn
    // (created_at = updated_at = now); `update_agent_graph_status` is
    // called on every observed status transition by the runtime host;
    // `remove_agent_graph_entry` is called on shutdown. The latter
    // happens before the agent is removed from the in-memory registry.

    /// Insert or update an agent's runtime metadata. Called when an
    /// agent is registered with the host for the first time (or when
    /// it is re-registered after a profile change).
    async fn upsert_agent_graph_entry(
        &self,
        entry: PersistedAgentGraph,
    ) -> Result<(), StorageError>;

    /// Update only the `status_json` / `last_event` / `updated_at`
    /// fields of an existing entry. `path` is the primary key. Called
    /// on every observed status transition by `RuntimeHost::observe_status`.
    async fn update_agent_graph_status(
        &self,
        path: &str,
        status_json: &str,
        last_event: Option<&str>,
    ) -> Result<(), StorageError>;

    /// Remove an entry (called on `RuntimeHost::shutdown_agent`). After
    /// this returns, `list_persisted_agents` will not include the path.
    /// Idempotent — removing a non-existent path is not an error.
    async fn remove_agent_graph_entry(&self, path: &str) -> Result<(), StorageError>;

    /// Read all persisted agent metadata. Used by the dashboard on
    /// process startup to surface agents that ran in the previous
    /// session. Returns entries ordered by `updated_at` descending.
    async fn list_persisted_agents(&self) -> Result<Vec<PersistedAgentGraph>, StorageError>;

    /// Persist the exact current daemon layout as one atomic snapshot.
    async fn save_agent_layout(&self, snapshot: &AgentLayoutSnapshot) -> Result<(), StorageError>;

    /// Load the current daemon layout, if one has been written.
    async fn load_agent_layout(&self) -> Result<Option<AgentLayoutSnapshot>, StorageError>;

    // ── Session log (WAL) ────────────────────────────────────

    async fn start_session(&self, agent_id: Uuid, session_id: Uuid) -> Result<(), StorageError>;
    async fn append_message(&self, session_id: Uuid, message: &Message)
    -> Result<(), StorageError>;
    async fn end_session(&self, session_id: Uuid) -> Result<(), StorageError>;

    /// Permanently delete a session and its messages from storage.
    /// Used when the user explicitly closes a session.
    async fn delete_session(&self, session_id: Uuid) -> Result<(), StorageError>;

    /// Replace a session's full conversation state after compaction.
    ///
    /// Deletes all existing messages for the session, inserts the new
    /// (compacted) messages, and records a fresh snapshot so that
    /// `restore_session_state` picks up the post-compaction state.
    async fn replace_session_state(
        &self,
        agent_id: Uuid,
        session_id: Uuid,
        state: &SessionState,
    ) -> Result<(), StorageError>;
    /// Append messages to a session's immutable user-facing transcript.
    ///
    /// Message IDs already archived for that session are ignored, making this
    /// idempotent across snapshots and repeated compactions.
    async fn archive_transcript(
        &self,
        session_id: Uuid,
        messages: &[Message],
    ) -> Result<(), StorageError>;
    /// Read the complete user-facing transcript, including messages removed
    /// from the active model context by compaction.
    async fn get_transcript_messages(&self, session_id: Uuid)
    -> Result<Vec<Message>, StorageError>;
    async fn get_messages_since(
        &self,
        agent_id: Uuid,
        ts: i64,
    ) -> Result<Vec<Message>, StorageError>;

    /// Update the title of a session record.
    async fn update_session_title(&self, session_id: Uuid, title: &str)
    -> Result<(), StorageError>;

    /// List all session records for an agent (for restoring session list on
    /// restart). Returns sessions ordered by creation time ascending.
    async fn list_session_records(
        &self,
        agent_id: Uuid,
    ) -> Result<Vec<SessionRecord>, StorageError>;

    /// Replace cumulative telemetry counters for a session.
    async fn update_session_telemetry(
        &self,
        session_id: Uuid,
        telemetry: &SessionTelemetry,
    ) -> Result<(), StorageError>;

    /// Insert or reopen an explicit agent turn.
    async fn start_agent_turn(&self, turn: AgentTurnRecord) -> Result<(), StorageError>;

    /// Update a turn's terminal status and completion timestamp.
    async fn finish_agent_turn(&self, turn: AgentTurnRecord) -> Result<(), StorageError>;

    /// Upsert the delegation ledger entry.
    async fn upsert_agent_delegation(
        &self,
        delegation: AgentDelegationRecord,
    ) -> Result<(), StorageError>;

    /// List persisted delegations ordered newest first.
    async fn list_agent_delegations(
        &self,
        caller_path: Option<&str>,
        target_path: Option<&str>,
        status: Option<&str>,
        limit: u32,
    ) -> Result<Vec<AgentDelegationRecord>, StorageError>;
    // ── Cross-session memories ─────────────────────────────

    async fn get_memory_stage1_output(
        &self,
        scope_id: Uuid,
        session_id: Uuid,
    ) -> Result<Option<crate::memory::MemoryStage1Record>, StorageError>;

    async fn claim_memory_stage1(
        &self,
        scope_id: Uuid,
        session_id: Uuid,
        source_hash: &str,
        lease_until: i64,
    ) -> Result<bool, StorageError>;

    async fn complete_memory_stage1(
        &self,
        scope_id: Uuid,
        output: crate::memory::MemoryStage1Record,
    ) -> Result<(), StorageError>;

    async fn fail_memory_stage1(
        &self,
        scope_id: Uuid,
        session_id: Uuid,
        source_hash: &str,
        error: &str,
    ) -> Result<(), StorageError>;

    async fn list_memory_stage1_outputs(
        &self,
        scope_id: Uuid,
        limit: usize,
    ) -> Result<Vec<crate::memory::MemoryStage1Record>, StorageError>;

    async fn claim_memory_phase2(
        &self,
        scope_id: Uuid,
        source_hash: &str,
        lease_until: i64,
    ) -> Result<bool, StorageError>;

    async fn fail_memory_phase2(
        &self,
        scope_id: Uuid,
        source_hash: &str,
        error: &str,
    ) -> Result<(), StorageError>;

    // ── Session plan (per-conversation persistent task plan) ──

    /// Persist a session's current plan (full-snapshot upsert).
    async fn save_plan(
        &self,
        agent_id: Uuid,
        session_id: Uuid,
        plan: &AgentPlan,
    ) -> Result<(), StorageError>;

    /// Load a session's plan. Returns `None` if no plan has been stored.
    async fn load_plan(
        &self,
        agent_id: Uuid,
        session_id: Uuid,
    ) -> Result<Option<AgentPlan>, StorageError>;
}

/// Persisted metadata about one session, used to rebuild the session list on
/// agent restart.
#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub session_id: Uuid,
    pub title: Option<String>,
    pub started_at: i64,
    /// Last time the session was paused or closed. `None` means active.
    pub ended_at: Option<i64>,
    /// Last time telemetry observed activity in this session.
    pub last_active: i64,
    pub telemetry: SessionTelemetry,
}

/// One explicitly tracked conversation turn.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentTurnRecord {
    pub turn_id: Uuid,
    pub agent_id: Uuid,
    pub session_id: Uuid,
    pub delegation_id: Option<Uuid>,
    pub status: String,
    pub started_at: i64,
    pub completed_at: Option<i64>,
    #[serde(default)]
    pub telemetry: Option<TurnTelemetry>,
}

/// Persisted agent-to-agent delegation ledger entry.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentDelegationRecord {
    pub delegation_id: Uuid,
    pub caller_path: Option<String>,
    pub target_path: String,
    pub task: String,
    pub status: String,
    pub turn_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    pub response: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

// ═══════════════════════════════════════════════════════════════════════
// Restore helper
// ═══════════════════════════════════════════════════════════════════════

/// Reconstruct a session's [`SessionState`] from storage.
///
/// Loads the latest snapshot as a base, then replays all messages persisted
/// after the snapshot's timestamp. The snapshot's `ts` acts as a watermark:
/// any message with `ts > snapshot.ts` was appended to the WAL after the
/// snapshot was taken and therefore is not yet in the snapshot's state.
pub async fn restore_session_state(
    storage: &dyn AgentStorage,
    agent_id: Uuid,
    session_id: Uuid,
) -> Result<SessionState, StorageError> {
    let snapshot = storage
        .get_latest_snapshot_for_session(agent_id, session_id)
        .await?;
    let snapshot_ts = snapshot.as_ref().map(|s| s.ts).unwrap_or(0);
    let mut state = snapshot.map(|s| s.state).unwrap_or_default();
    let messages = storage
        .get_messages_since_for_session(session_id, snapshot_ts)
        .await?;
    state.messages.extend(messages);
    Ok(state)
}
