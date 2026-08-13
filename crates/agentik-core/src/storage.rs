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
use agentik_types::AgentPlan;

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
    StartSession { agent_id: Uuid, session_id: Uuid },
    AppendMessage { session_id: Uuid, message: Message },
    EndSession { session_id: Uuid },
}

// ═══════════════════════════════════════════════════════════════════════
// Agent Profile (blueprint / preset)
// ═══════════════════════════════════════════════════════════════════════

/// Optional delta fields when deriving a child profile from a parent.
///
/// Any `None` field inherits the parent's value. Used by
/// [`AgentProfile::derive_child`] and the `derive_profile` agent tool.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ProfileOverrides {
    pub description: Option<String>,
    pub agent_identity: Option<String>,
    /// `Some(None)` explicitly clears the parent's system prompt.
    pub system_prompt: Option<Option<String>>,
    /// `Some(None)` explicitly clears the parent's model preference.
    pub preferred_model: Option<Option<String>>,
    pub enable_bibliography: Option<bool>,
    pub enable_writing: Option<bool>,
    pub enable_opengwas: Option<bool>,
    pub enable_opentargets: Option<bool>,
    pub enable_gwascatalog: Option<bool>,
    pub enable_iceberg: Option<bool>,
    pub enable_dag_history: Option<bool>,
}

/// A hierarchical, persisted agent configuration template that can be
/// instantiated into a running [`Agent`](crate::Agent).
///
/// Profiles form a tree mirroring the [`AgentPath`](agentik_types::AgentPath)
/// hierarchy. The `path` field (e.g. `"researcher"`, `"researcher/genomics"`)
/// defines the profile's position in the type tree. Each segment follows the
/// same validation rules as `AgentPath` segments: lowercase `[a-z0-9_]`,
/// 1–32 chars.
///
/// Child profiles inherit all capabilities from their parent and can override
/// specific fields via [`ProfileOverrides`]. Use [`derive_child`](Self::derive_child)
/// to create a specialized child profile.
///
/// Unlike [`AgentRecord`] (which tracks *runtime state* — memory, sessions,
/// last-active), an `AgentProfile` is a **blueprint**: it defines what an
/// agent *is* (identity, prompts, tool capabilities, model preference) but
/// holds no conversation history.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentProfile {
    pub id: Uuid,
    /// Hierarchical profile path using `/`-separated segments
    /// (e.g. `"researcher"`, `"researcher/genomics"`). Stored in the SQL
    /// `name` column for backward compatibility. Each segment validated
    /// against the same `[a-z0-9_]` 1-32 char rule as `AgentPath`.
    #[serde(alias = "name")]
    pub path: String,
    pub description: String,

    // ── Prompt ──
    pub agent_identity: String,
    /// Long-form system prompt. `None` means "use built-in default".
    pub system_prompt: Option<String>,

    // ── Tool capability flags ──
    pub enable_bibliography: bool,
    #[serde(default)]
    pub enable_writing: bool,
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
    /// Last segment of the path — the profile's short name.
    /// Mirrors `AgentPath::name()`.
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    /// Parent profile path, or `None` for root profiles.
    pub fn parent_path(&self) -> Option<&str> {
        self.path.rfind('/').map(|i| &self.path[..i])
    }

    /// Depth in the profile tree (0 = root profile).
    pub fn depth(&self) -> usize {
        self.path.matches('/').count()
    }

    /// Create a new root-level profile with sensible defaults (all tools enabled).
    pub fn new(path: impl Into<String>) -> Self {
        let now = now_ms();
        Self {
            id: Uuid::new_v4(),
            path: path.into(),
            description: String::new(),
            agent_identity: "You are a helpful assistant.".into(),
            system_prompt: None,
            enable_bibliography: true,
            enable_writing: true,
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

    /// Derive a child profile from `self`, appending `segment` to the path
    /// and applying `overrides`.
    ///
    /// The child inherits all resolved capabilities from the parent. Any
    /// field in `overrides` that is `Some` replaces the inherited value.
    pub fn derive_child(&self, segment: &str, overrides: ProfileOverrides) -> Result<Self, String> {
        agentik_types::validate_segment(segment)
            .map_err(|e| format!("invalid profile segment `{segment}`: {e}"))?;

        let child_path = format!("{}/{}", self.path, segment);
        let now = now_ms();

        Ok(Self {
            id: Uuid::new_v4(),
            path: child_path,
            description: overrides
                .description
                .unwrap_or_else(|| self.description.clone()),
            agent_identity: overrides
                .agent_identity
                .unwrap_or_else(|| self.agent_identity.clone()),
            system_prompt: overrides
                .system_prompt
                .unwrap_or_else(|| self.system_prompt.clone()),
            enable_bibliography: overrides
                .enable_bibliography
                .unwrap_or(self.enable_bibliography),
            enable_writing: overrides.enable_writing.unwrap_or(self.enable_writing),
            enable_opengwas: overrides.enable_opengwas.unwrap_or(self.enable_opengwas),
            enable_opentargets: overrides
                .enable_opentargets
                .unwrap_or(self.enable_opentargets),
            enable_gwascatalog: overrides
                .enable_gwascatalog
                .unwrap_or(self.enable_gwascatalog),
            enable_iceberg: overrides.enable_iceberg.unwrap_or(self.enable_iceberg),
            enable_dag_history: overrides
                .enable_dag_history
                .unwrap_or(self.enable_dag_history),
            preferred_model: overrides
                .preferred_model
                .unwrap_or_else(|| self.preferred_model.clone()),
            created_at: now,
            updated_at: now,
        })
    }

    /// Return the built-in default profiles seeded on first run.
    pub fn defaults() -> Vec<AgentProfile> {
        let now = now_ms();
        vec![
            AgentProfile {
                id: Uuid::new_v4(),
                path: "researcher".into(),
                description: "Full-featured biomedical research assistant.".into(),
                agent_identity: "You are a biomedical research assistant specializing \
                    in genomics, GWAS analysis, and literature mining."
                    .into(),
                system_prompt: None,
                enable_bibliography: true,
                enable_writing: true,
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
                path: "literature".into(),
                description: "Literature search and evidence synthesis expert.".into(),
                agent_identity: "You are a literature search expert specializing in \
                    systematic reviews, meta-analyses, and evidence synthesis. \
                    Use PubMed, arXiv, and bioRxiv tools to find and analyze publications."
                    .into(),
                system_prompt: None,
                enable_bibliography: true,
                enable_writing: false,
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
                path: "gwas-analysis".into(),
                description: "GWAS data analysis and statistical genetics expert.".into(),
                agent_identity: "You are a GWAS analysis expert specializing in \
                    statistical genetics. Use OpenGWAS, GWAS Catalog, and the \
                    data pipeline engine to analyze genetic association data."
                    .into(),
                system_prompt: None,
                enable_bibliography: false,
                enable_writing: false,
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
                path: "writer".into(),
                description: "Manuscript writing, editing, and LaTeX compilation expert.".into(),
                agent_identity: "You are a scientific manuscript writing assistant specializing \
                    in LaTeX document preparation, citation management, and compilation. \
                    Use the writing tools (doc_create, doc_insert_section, doc_insert_block, \
                    doc_add_citation, doc_compile) to draft, edit, and compile documents. \
                    Use bibliography tools (lit_search, bib_save) to find and store references."
                    .into(),
                system_prompt: Some(
                    "When writing a manuscript:\n\
                    1. Use doc_create to start a new document\n\
                    2. Use doc_insert_section to build the outline (Introduction, Methods, Results, Discussion)\n\
                    3. Use doc_insert_block to add paragraphs, equations, and tables\n\
                    4. Use lit_search + bib_save to find and store references\n\
                    5. Use doc_add_citation to insert citations\n\
                    6. Use doc_check_citations to verify all citations resolve\n\
                    7. Use doc_compile to produce the final PDF\n\
                    \n\
                    Always run doc_check_citations before doc_compile to catch broken references.".into()
                ),
                enable_bibliography: true,
                enable_writing: true,
                enable_opengwas: false,
                enable_opentargets: false,
                enable_gwascatalog: false,
                enable_iceberg: false,
                enable_dag_history: false,
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
    // Persists the multi-agent host's active-agent metadata across
    // process restarts. Keyed by hierarchical path (not UUID) so the
    // dashboard can be reconstructed even if the agent's identity
    // UUID is rotated (e.g. profile re-derivation). Mirrors codex's
    // `AgentGraphStore` trait.
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

    // ── Session log (WAL) ────────────────────────────────────

    async fn start_session(&self, agent_id: Uuid, session_id: Uuid) -> Result<(), StorageError>;
    async fn append_message(&self, session_id: Uuid, message: &Message)
    -> Result<(), StorageError>;
    async fn end_session(&self, session_id: Uuid) -> Result<(), StorageError>;

    /// Permanently delete a session and its messages from storage.
    /// Used when the user explicitly closes a session.
    async fn delete_session(&self, session_id: Uuid) -> Result<(), StorageError>;
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

    // ── Agent plan (first-class persistent task plan) ──────

    /// Persist the agent's current plan (full-snapshot upsert).
    async fn save_plan(&self, agent_id: Uuid, plan: &AgentPlan) -> Result<(), StorageError>;

    /// Load the agent's plan. Returns `None` if no plan has been stored.
    async fn load_plan(&self, agent_id: Uuid) -> Result<Option<AgentPlan>, StorageError>;
}

/// Persisted metadata about one session, used to rebuild the session list on
/// agent restart.
#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub session_id: Uuid,
    pub title: Option<String>,
    pub started_at: i64,
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
    async fn get_profile_by_path(&self, path: &str) -> Result<Option<AgentProfile>, StorageError>;
    async fn list_profiles(&self) -> Result<Vec<AgentProfile>, StorageError>;
    async fn list_child_profiles(
        &self,
        parent_path: &str,
    ) -> Result<Vec<AgentProfile>, StorageError>;
    async fn update_profile(&self, profile: AgentProfile) -> Result<(), StorageError>;
    async fn delete_profile(&self, id: Uuid) -> Result<(), StorageError>;

    /// Ensure every built-in default profile exists (by path), seeding any
    /// that are missing. Also migrates legacy profile names (e.g. the old
    /// `default` → `researcher` rename). Returns `true` if any change was
    /// made.
    async fn seed_defaults_if_empty(&self) -> Result<bool, StorageError> {
        let existing = self.list_profiles().await?;
        let mut changed = false;

        // ── Legacy migration: rename `default` → `researcher` ──
        if let Some(legacy) = existing.iter().find(|p| p.path == "default").cloned() {
            let mut renamed = legacy;
            renamed.path = "researcher".into();
            renamed.updated_at = now_ms();
            self.update_profile(renamed).await?;
            changed = true;
        }

        // ── Ensure every default profile exists ──
        // Re-read after the migration so the path set reflects any renames.
        let current = self.list_profiles().await?;
        let paths: std::collections::HashSet<String> =
            current.iter().map(|p| p.path.clone()).collect();
        for profile in AgentProfile::defaults() {
            if !paths.contains(&profile.path) {
                self.create_profile(profile).await?;
                changed = true;
            }
        }

        Ok(changed)
    }
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
