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
use agentik_types::{AgentPlan, ContextTelemetry, TurnTelemetry};

use crate::context::ContextState;
use crate::lifecycle::AgentLifecycleStatus;

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
    pub state: ContextState,
    /// Which session this snapshot belongs to.
    #[serde(default)]
    pub session_id: Option<Uuid>,
}

/// A persisted transcript message and its storage timestamp.
///
/// This is storage metadata only. It deliberately does not alter `Message`
/// serialization, so model-provider and gateway wire payloads stay unchanged.
#[derive(Debug, Clone)]
pub struct TimestampedMessage {
    pub message: Message,
    /// Unix epoch milliseconds.
    pub created_at: i64,
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
        state: ContextState,
    },
    /// Replace a session's cumulative telemetry counters.
    UpdateSessionTelemetry {
        session_id: Uuid,
        telemetry: ContextTelemetry,
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
    /// Per-agent reasoning-effort override. `None` inherits the model's
    /// standard `medium` default, except where a required-thinking model's
    /// lightweight fallback applies. Applied at request time, so changes
    /// hot-reload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<agentik_types::ReasoningEffort>,
}

pub use agentik_memory::AgentRuntimeConfig;

// ═══════════════════════════════════════════════════════════════════════
// Error
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("agent not found: {0}")]
    AgentNotFound(Uuid),
    /// Durable history exists but cannot be safely resumed (e.g. an agent's
    /// bound context is missing). Callers must refuse an empty replacement
    /// and surface the reason instead of falling back to another record.
    #[error("recovery required: {0}")]
    RecoveryRequired(String),
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
    /// Search persisted identities by ordered path characters (case-insensitive
    /// SQL `LIKE`), newest activity first, capped to `limit`. Backs the
    /// frontend agent picker: identities are searched and paged, never
    /// listed in full — the record count is unbounded under virtual-actor
    /// mode (every first message materializes a record).
    async fn search_agents(
        &self,
        query: Option<&str>,
        limit: u32,
    ) -> Result<Vec<AgentRecord>, StorageError>;
    async fn delete_agent(&self, agent_id: Uuid) -> Result<(), StorageError>;
    async fn add_relation(&self, relation: AgentRelation) -> Result<(), StorageError>;
    async fn list_children(&self, agent_id: Uuid) -> Result<Vec<AgentRelation>, StorageError>;
    async fn list_parents(&self, agent_id: Uuid) -> Result<Vec<AgentRelation>, StorageError>;
    async fn touch_agent(&self, agent_id: Uuid) -> Result<(), StorageError>;

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
        state: &ContextState,
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
    /// Read transcript rows with their durable creation times. This is an
    /// internal storage API and is intentionally absent from the wire layer.
    async fn get_timestamped_transcript_messages(
        &self,
        session_id: Uuid,
    ) -> Result<Vec<TimestampedMessage>, StorageError>;
    async fn get_messages_since(
        &self,
        agent_id: Uuid,
        ts: i64,
    ) -> Result<Vec<Message>, StorageError>;

    /// Update the title of a session record.
    async fn update_session_title(&self, session_id: Uuid, title: &str)
    -> Result<(), StorageError>;

    /// Record where a forked session came from.
    ///
    /// `parent_session_id` identifies the source conversation. When present,
    /// `fork_point_message_id` identifies the final parent message included
    /// in the child context.
    async fn update_session_lineage(
        &self,
        session_id: Uuid,
        parent_session_id: Option<Uuid>,
        fork_point_message_id: Option<&str>,
    ) -> Result<(), StorageError>;

    /// List all session records for an agent (for restoring session list on
    /// restart). Returns sessions ordered by creation time ascending.
    async fn list_session_records(
        &self,
        agent_id: Uuid,
    ) -> Result<Vec<SessionRecord>, StorageError>;

    /// The agent's immutable context binding: which session record is the
    /// agent's sole working context. `None` until the first restore binds
    /// one (legacy multi-session agents get their most recently active
    /// history bound on first boot).
    async fn agent_context_id(&self, agent_id: Uuid) -> Result<Option<Uuid>, StorageError>;

    /// Bind an agent to its sole context. First writer wins atomically —
    /// this never updates an existing binding; a mismatching binding is a
    /// [`StorageError::RecoveryRequired`].
    async fn bind_agent_context(
        &self,
        agent_id: Uuid,
        context_id: Uuid,
    ) -> Result<(), StorageError>;

    /// Replace cumulative telemetry counters for a session.
    async fn update_session_telemetry(
        &self,
        session_id: Uuid,
        telemetry: &ContextTelemetry,
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
    pub telemetry: ContextTelemetry,
    pub parent_session_id: Option<Uuid>,
    pub fork_point_message_id: Option<String>,
}

/// Select the same sole context for runtime restoration and client hydration.
/// A persisted binding wins; a missing bound record is a recovery error.
/// Legacy identities without a binding use deterministic activity ordering.
pub fn select_agent_context_record(
    bound_id: Option<Uuid>,
    records: Vec<SessionRecord>,
) -> Result<Option<SessionRecord>, StorageError> {
    match bound_id {
        Some(id) => records
            .into_iter()
            .find(|record| record.session_id == id)
            .map(Some)
            .ok_or_else(|| {
                StorageError::RecoveryRequired(format!(
                    "bound context {id} is missing; refusing a replacement"
                ))
            }),
        None => Ok(records
            .into_iter()
            .max_by_key(|record| (record.last_active, record.started_at, record.session_id))),
    }
}

#[cfg(test)]
mod context_record_tests {
    use super::*;

    fn record(id: u128, activity: i64, created: i64) -> SessionRecord {
        SessionRecord {
            session_id: Uuid::from_u128(id),
            title: None,
            started_at: created,
            ended_at: None,
            last_active: activity,
            telemetry: ContextTelemetry::default(),
            parent_session_id: None,
            fork_point_message_id: None,
        }
    }

    #[test]
    fn context_selection_preserves_binding_and_legacy_order() {
        let records = vec![record(1, 1, 1), record(2, 10, 3), record(3, 10, 3)];
        let bound = Uuid::from_u128(1);
        assert_eq!(
            select_agent_context_record(Some(bound), records.clone())
                .unwrap()
                .unwrap()
                .session_id,
            bound
        );
        for ordering in [records.clone(), records.into_iter().rev().collect()] {
            assert_eq!(
                select_agent_context_record(None, ordering)
                    .unwrap()
                    .unwrap()
                    .session_id,
                Uuid::from_u128(3)
            );
        }
        assert!(select_agent_context_record(None, vec![]).unwrap().is_none());
        assert!(matches!(
            select_agent_context_record(Some(bound), vec![record(2, 99, 99)]),
            Err(StorageError::RecoveryRequired(_))
        ));
    }
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

/// Reconstruct a session's [`ContextState`] from storage.
///
/// Loads the latest snapshot as a base, then replays all messages persisted
/// after the snapshot's timestamp. The snapshot's `ts` acts as a watermark:
/// any message with `ts > snapshot.ts` was appended to the WAL after the
/// snapshot was taken and therefore is not yet in the snapshot's state.
pub async fn restore_session_state(
    storage: &dyn AgentStorage,
    agent_id: Uuid,
    session_id: Uuid,
) -> Result<ContextState, StorageError> {
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
