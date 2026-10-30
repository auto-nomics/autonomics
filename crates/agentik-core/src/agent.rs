//! # Agent — one persistent working context and its shared resources.
//!
//! An `Agent` owns stable resources (model, tool registry, storage, prompt
//! configuration) and exactly one working context: a
//! [`Session`](crate::Session) that lives as long as the agent does.
//! Independent work uses another agent; continuation restores that agent's
//! context automatically. Conversation creation, switching, forking and
//! closing within an agent are refused — `Session` and `session_id` remain
//! internal persistence and wire-format names for compatibility.
//!
//! On boot the agent restores its bound context from the `agent_contexts`
//! table. Legacy multi-session histories select their most recently active
//! record once (ties break by creation time then UUID) and bind it
//! permanently; the other histories stay readable through storage/history
//! APIs. A missing or ambiguous binding never falls back to another record
//! or an empty context — the agent refuses conversation until recovered.
//!
//! The outer [`Agent::run`] loop is an event dispatcher: it reads
//! [`InternalEvent`]s from its channel, applies session management events
//! (legacy commands are recognized and refused), and delegates conversation
//! events (`MessageInject`, `BgTaskComplete`) to the context's
//! [`Session::run_session`].

use std::sync::Arc;

use agentik_sdk::model::Model;
use agentik_sdk::types::AgentEvent;
use agentik_sdk::types::messages::ContentBlock;
use arc_swap::ArcSwapOption;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::error::{AgentError, Result};
use crate::session::{AgentShared, Session};
use crate::storage::{AgentRecord, AgentStorage, PersistOp, StorageError};
use crate::tools::ToolRegistration;
use agentik_types::SessionInfo;

#[derive(Clone)]
pub struct AgentConfig {
    pub max_iterations: usize,
    pub max_retries: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_iterations: 1000,
            max_retries: 10,
        }
    }
}

/// Internal events that wake the agent's outer [`Agent::run`] loop.
///
/// External callers (e.g. the TUI runtime) send these through
/// [`Agent::internal_event_tx`] to drive the agent without holding a
/// reference to the `Agent` struct.
pub enum InternalEvent {
    /// User injected a new message.
    ///
    /// `from_user` distinguishes TUI-originated prompts from external
    /// injections. A committed TUI prompt emits
    /// `AgentEvent::UserMessageAcknowledged`; external messages emit
    /// `AgentEvent::MessageInjected`.
    MessageInject {
        content: Vec<ContentBlock>,
        from_user: bool,
        delegation_id: Option<Uuid>,
    },
    /// A background tool task finished.
    /// `id` is the `tool_use_id`, `seq` is the short task number.
    BgTaskComplete {
        id: String,
        seq: u64,
    },
    Done,
    /// External Runtime requests the agent to shut down.
    Shutdown,
    /// Replace the active session's cancellation token with a fresh one.
    ResetCancelToken(CancellationToken),
    /// Legacy initialization command. Only an empty agent can initialize
    /// its sole context; replacement and fork arguments are explicitly
    /// refused.
    CreateSession {
        id: Uuid,
        fork_from: Option<Uuid>,
        fork_message_id: Option<String>,
        title: Option<String>,
    },
    /// Legacy selection command: foreign context IDs are refused.
    SwitchSession {
        id: Uuid,
    },
    /// Legacy close command: refused; the context lives as long as its
    /// agent.
    CloseSession {
        id: Uuid,
    },
    /// Request a list of all sessions (reply via event channel). With one
    /// context per agent the reply carries at most one entry.
    ListSessions,
    /// Rename the sole context (same-id renames only; foreign IDs are
    /// refused).
    RenameSession {
        id: Uuid,
        title: String,
    },
    /// Manually trigger compaction on the active session.
    Compact,
}

pub struct Agent {
    pub(crate) shared: Arc<AgentShared>,
    /// The agent's sole working context. `None` only before the first
    /// message (auto-created then) or while persistence recovery is pending.
    pub(crate) context: Option<Session>,
    /// Set when durable history exists but cannot be safely resumed (bound
    /// context missing). Metadata only — conversation is refused with the
    /// stored notice until the operator recovers the agent.
    pub(crate) recovery_notice: Option<String>,
    pub(crate) internal_event_tx: UnboundedSender<InternalEvent>,
    /// Receiver consumed once by [`run()`]; `None` after that.
    pub(crate) internal_event_rx: Option<UnboundedReceiver<InternalEvent>>,
    /// Initial messages from the builder. Used as a fallback when no
    /// session records exist in storage. Consumed during `run()`.
    pub(crate) initial_messages: Option<Vec<agentik_sdk::types::messages::Message>>,
    /// Cancel token from the builder, used when auto-creating the context.
    pub(crate) cancel_token: CancellationToken,
}

const RESTORED_SESSION_TITLE: &str = "Restored";

impl Agent {
    pub fn builder() -> crate::agent_builder::AgentBuilder {
        crate::agent_builder::AgentBuilder::new()
    }

    // ── Identity & shared accessors ───────────────────────

    pub fn id(&self) -> Uuid {
        self.shared.id
    }

    pub fn name(&self) -> &str {
        self.shared.name()
    }

    /// The agent's full hierarchical path (e.g. `/root/researcher`).
    pub fn path(&self) -> &agentik_types::AgentPath {
        self.shared.path()
    }

    pub fn model_handle(&self) -> &Arc<ArcSwapOption<Model>> {
        &self.shared.model
    }

    pub fn set_model(&self, model: Arc<agentik_sdk::model::Model>) {
        self.shared.model.store(Some(model));
    }

    pub fn internal_event_tx(&self) -> UnboundedSender<InternalEvent> {
        self.internal_event_tx.clone()
    }

    pub fn set_agent_event_tx(&self, tx: UnboundedSender<AgentEvent>) {
        self.shared.event_tx.store(Some(Arc::new(tx)));
    }

    pub fn set_system_prompt_identity(&mut self, identity: impl Into<String>) {
        if let Some(shared) = Arc::get_mut(&mut self.shared) {
            shared.system_prompt_identity = Some(identity.into());
        }
    }

    pub fn set_system_prompt_section(&mut self, section: impl Into<String>) {
        if let Some(shared) = Arc::get_mut(&mut self.shared) {
            shared.system_prompt_section = Some(section.into());
        }
    }

    // ── Tool registration ─────────────────────────────────

    /// Register a single tool into the agent's tool registry.
    ///
    /// Only works before sessions are created (the registry Arc must be
    /// uniquely owned). After sessions exist, rebuild with
    /// [`Agent::builder`].
    pub fn register_tool(&mut self, registration: ToolRegistration) -> Result<()> {
        let shared = Arc::get_mut(&mut self.shared).ok_or_else(|| {
            AgentError::Tool(crate::tools::error::ToolError::RegistryError {
                message: "AgentShared is frozen (sessions exist)".into(),
            })
        })?;
        Arc::get_mut(&mut shared.tool_registry)
            .ok_or_else(|| {
                AgentError::Tool(crate::tools::error::ToolError::RegistryError {
                    message: "Tool registry is frozen (shared across sessions)".into(),
                })
            })?
            .register(registration)?;
        Ok(())
    }

    /// Register multiple tools at once. See [`register_tool`](Self::register_tool).
    pub fn register_tools(&mut self, registrations: Vec<ToolRegistration>) -> Result<()> {
        let shared = Arc::get_mut(&mut self.shared).ok_or_else(|| {
            AgentError::Tool(crate::tools::error::ToolError::RegistryError {
                message: "AgentShared is frozen (sessions exist)".into(),
            })
        })?;
        Arc::get_mut(&mut shared.tool_registry)
            .ok_or_else(|| {
                AgentError::Tool(crate::tools::error::ToolError::RegistryError {
                    message: "Tool registry is frozen (shared across sessions)".into(),
                })
            })?
            .register_all(registrations)?;
        Ok(())
    }

    // ── Context accessors ─────────────────────────────────

    /// The agent's sole working context (see the module docs).
    pub fn active_session(&self) -> Option<&Session> {
        self.context.as_ref()
    }

    pub fn active_session_mut(&mut self) -> Option<&mut Session> {
        self.context.as_mut()
    }

    pub fn lifecycle_status(&self) -> agentik_types::AgentLifecycleStatus {
        self.active_session()
            .map(|s| *s.lifecycle.status())
            .unwrap_or(agentik_types::AgentLifecycleStatus::Idle)
    }

    pub fn is_running(&self) -> bool {
        self.active_session()
            .map(|s| s.lifecycle.is_running())
            .unwrap_or(false)
    }

    pub fn inject_message(
        &mut self,
        user_content: Vec<ContentBlock>,
        from_user: bool,
    ) -> Result<()> {
        self.active_session_mut()
            .map(|s| s.inject_message(user_content, from_user))
            .transpose()
            .map(|_| ())
    }

    pub fn set_cancel_token(&mut self, token: CancellationToken) {
        if let Some(s) = self.active_session_mut() {
            s.set_cancel_token(token);
        }
    }

    /// Session metadata for `ListSessions` replies: the sole context, or an
    /// empty list while recovery is pending.
    pub fn session_infos(&self) -> Vec<SessionInfo> {
        self.context
            .as_ref()
            .map(SessionInfo::from)
            .into_iter()
            .collect()
    }

    // ── Snapshot ──────────────────────────────────────────

    pub async fn snapshot(&self) -> Option<crate::storage::AgentSnapshot> {
        self.active_session().map(|s| s.snapshot())
    }

    // ── Main run loop ─────────────────────────────────────

    pub async fn run(&mut self) {
        let mut rx = self
            .internal_event_rx
            .take()
            .expect("internal_event_rx already consumed by a prior run()");

        // ── Persistence bootstrap ────────────────────────────
        if let Some(storage) = self.shared.storage.clone() {
            let now = chrono::Utc::now().timestamp_millis();
            let _ = storage
                .as_ref()
                .upsert_agent(AgentRecord {
                    id: self.shared.id,
                    name: self.shared.path().as_str().to_string(),
                    config_json: self.shared.config_json.clone(),
                    created_at: now,
                    last_active: now,
                })
                .await;

            let (persist_tx, persist_rx) = tokio::sync::mpsc::unbounded_channel::<PersistOp>();

            // Store in shared so contexts created later can also access it.
            let _ = self.shared.persist_tx.set(persist_tx);
            // Wire into an existing context.
            if let Some(session) = self.context.as_mut() {
                if let Some(tx) = self.shared.persist_tx.get() {
                    session.persist_tx = Some(tx.clone());
                }
            }
            crate::supervise::spawn_safe_drop(
                "persist_worker",
                persist_worker(persist_rx, Arc::clone(&storage)),
            );

            if let Some(memory) = &self.shared.memory {
                let memory_config = memory.effective_memory_config();
                if memory_config.generate_memory && memory_config.is_root_agent(&self.shared.path) {
                    let agent_id = self.shared.id;
                    if let Err(error) = memory
                        .submit_knowledge_event(crate::memory::KnowledgeEvent::AgentStarted {
                            agent_id,
                        })
                        .await
                    {
                        tracing::warn!(
                            agent_id = %agent_id,
                            error = %error,
                            "failed to queue knowledge extraction"
                        );
                    }
                }
            }
        }

        // ── Restore the sole context from storage ──
        // Select BEFORE recovery: a damaged or missing bound context must
        // never silently fall back to another record or an empty one.
        // Legacy multi-session histories select their most recently active
        // record once and bind it permanently; older histories stay
        // read-only in storage.
        if let Some(storage) = self.shared.storage.clone() {
            match Self::restore_context_record(storage.as_ref(), self.shared.id).await {
                Ok(Some(rec)) => {
                    // Rebuild the context from storage using the unified
                    // restore_session_state helper (snapshot + WAL).
                    let mut s = Session::new(rec.session_id, self.shared.clone());
                    s.cancel_token = self.cancel_token.clone();
                    s.title = rec.title;
                    s.created_at = rec.started_at;
                    s.last_active = rec.last_active;
                    s.telemetry = rec.telemetry;
                    s.parent_session_id = rec.parent_session_id;
                    s.fork_point_message_id = rec.fork_point_message_id;

                    match crate::storage::restore_session_state(
                        storage.as_ref(),
                        self.shared.id,
                        rec.session_id,
                    )
                    .await
                    {
                        Ok(state) => {
                            s.messages = state.messages;
                            s.summary = state.summary;
                            s.ancestor_summaries = state.ancestor_summaries;
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "failed to restore context state for {}", rec.session_id
                            );
                        }
                    }

                    // Restore the context's persisted plan. `set_plan`
                    // stores in place, so the Toolset's captured handle
                    // sees the restored plan immediately.
                    match storage
                        .as_ref()
                        .load_plan(self.shared.id, rec.session_id)
                        .await
                    {
                        Ok(Some(plan)) if !plan.is_empty() => s.set_plan(plan),
                        Ok(_) => {}
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "failed to restore plan for session {}", rec.session_id
                            );
                        }
                    }

                    tracing::info!(
                        session_id = %rec.session_id,
                        title = ?s.title,
                        "restored sole context from storage"
                    );
                    s.resume().await;
                    self.context = Some(s);
                }
                Ok(None) => {}
                Err(StorageError::RecoveryRequired(reason)) => {
                    let notice = format!(
                        "PERSISTENCE_RECOVERY_REQUIRED: {reason}. History preserved. \
                         Create another agent for independent work; this context was not resumed."
                    );
                    tracing::warn!(agent_id = %self.shared.id, %reason,
                        "context recovery required; refusing an empty replacement");
                    self.shared.send_event(AgentEvent::Error(notice.clone()));
                    self.recovery_notice = Some(notice);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to list session records");
                }
            }
        }

        // ── Fallback: no session records in storage ──
        // If the builder provided initial messages and no context was
        // restored, create one with those messages and bind it.
        if self.context.is_none() && self.recovery_notice.is_none() {
            if let Some(messages) = self.initial_messages.take() {
                let id = Uuid::new_v4();
                let state = crate::session::SessionState {
                    messages,
                    summary: None,
                    ancestor_summaries: Vec::new(),
                    telemetry: agentik_types::SessionTelemetry::default(),
                };
                let mut s = Session::new_with_state(
                    id,
                    self.shared.clone(),
                    state,
                    self.cancel_token.clone(),
                );
                s.title = Some(RESTORED_SESSION_TITLE.into());
                s.resume().await;
                self.persist_session_title(id, RESTORED_SESSION_TITLE).await;
                self.bind_context(id).await;
                self.context = Some(s);
                tracing::info!("created fallback session from builder initial messages");
            }
        }

        loop {
            let event = match rx.recv().await {
                Some(e) => e,
                None => break,
            };

            // ── Auto-create the context on first message ──
            // If no context exists yet, create one before processing the
            // event so the message isn't lost. Titled with the agent name:
            // the context IS the agent's conversation now.
            if matches!(event, InternalEvent::MessageInject { .. })
                && self.context.is_none()
                && self.recovery_notice.is_none()
            {
                let id = Uuid::new_v4();
                let title = self.shared.name().to_string();
                let mut s = Session::new(id, self.shared.clone());
                s.title = Some(title.clone());
                s.cancel_token = self.cancel_token.clone();
                s.resume().await;
                self.persist_session_title(id, &title).await;
                self.bind_context(id).await;
                self.context = Some(s);
                self.shared.send_event(AgentEvent::SessionActivated {
                    id,
                    title: Some(title),
                });
                tracing::info!("auto-created session on first message");
            }

            // Session management events are handled inline before delegation.
            let should_run = match &event {
                InternalEvent::CreateSession {
                    id,
                    fork_from,
                    fork_message_id,
                    title,
                } => {
                    self.handle_create_session(
                        *id,
                        *fork_from,
                        fork_message_id.as_deref(),
                        title.clone(),
                    )
                    .await;
                    false
                }
                InternalEvent::SwitchSession { id } => {
                    self.handle_switch_session(*id).await;
                    false
                }
                InternalEvent::CloseSession { id } => {
                    self.handle_close_session(*id).await;
                    false
                }
                InternalEvent::ListSessions => {
                    let infos: Vec<SessionInfo> = self.session_infos();
                    self.shared
                        .send_event(AgentEvent::SessionList { sessions: infos });
                    false
                }
                InternalEvent::RenameSession { id, title } => {
                    self.handle_rename_session(*id, title.clone()).await;
                    false
                }
                InternalEvent::MessageInject { .. } | InternalEvent::BgTaskComplete { .. } => true,
                _ => false,
            };

            let delegation_id = match &event {
                InternalEvent::MessageInject { delegation_id, .. } => *delegation_id,
                _ => None,
            };

            // Non-session-management events go through apply_event.
            if !matches!(
                event,
                InternalEvent::CreateSession { .. }
                    | InternalEvent::SwitchSession { .. }
                    | InternalEvent::CloseSession { .. }
                    | InternalEvent::ListSessions
                    | InternalEvent::RenameSession { .. }
            ) {
                let keep_going = self.apply_event(event).await;
                if !keep_going {
                    break;
                }
            }

            if should_run {
                let tx = self.internal_event_tx.clone();
                if let Some(session) = self.context.as_mut() {
                    session.run_session(&tx, &mut rx, delegation_id).await;
                    // A cancel can replace the token while run_session owns
                    // the receiver. Keep Agent-owned context creation in
                    // sync with that fresh token.
                    self.cancel_token = session.cancel_token.clone();
                }
            }
        }

        // Shutdown: cancel background tool tasks, then pause the context.
        // Background tasks (e.g. run_dag, run_bash) are tokio::spawn'd and
        // would outlive the agent task; cancelling their tokens lets them
        // exit cleanly before the runtime is dropped.
        if let Some(session) = self.context.as_mut() {
            session.finish_turn(agentik_types::TurnExecutionStatus::Interrupted);
            session.toolset.cancel_all_tasks().await;
            session.pause().await;
        }
    }

    /// Select the agent's sole context record and pin the binding.
    ///
    /// The `agent_contexts` binding is authoritative once written. Without
    /// one (legacy agents predating the binding), the most recently active
    /// record wins — ties break by creation time, then UUID, so the choice
    /// is deterministic even across clock rollbacks. The winner is bound
    /// BEFORE any recovery attempt: a damaged latest context must never
    /// silently fall back to an older or empty one.
    async fn restore_context_record(
        storage: &dyn AgentStorage,
        agent_id: Uuid,
    ) -> std::result::Result<Option<crate::storage::SessionRecord>, StorageError> {
        let records = storage.list_session_records(agent_id).await?;
        let count = records.len();
        let selected = match storage.agent_context_id(agent_id).await? {
            Some(id) => Some(
                records
                    .into_iter()
                    .find(|record| record.session_id == id)
                    .ok_or_else(|| {
                        StorageError::RecoveryRequired(format!(
                            "bound context {id} is missing; refusing a replacement"
                        ))
                    })?,
            ),
            None => records
                .into_iter()
                .max_by_key(|record| (record.last_active, record.started_at, record.session_id)),
        };
        if let Some(record) = &selected {
            storage
                .bind_agent_context(agent_id, record.session_id)
                .await?;
            if count > 1 {
                tracing::warn!(archived_contexts = count - 1, context_id = %record.session_id,
                    "restoring sole context; legacy histories retained read-only");
            }
        }
        Ok(selected)
    }

    /// Best-effort context binding after creating a fresh context. Binding
    /// failures are surfaced but never block the conversation: the record
    /// itself already exists, and the next restore re-attempts selection.
    async fn bind_context(&self, id: Uuid) {
        if let Some(storage) = &self.shared.storage {
            if let Err(error) = storage
                .as_ref()
                .bind_agent_context(self.shared.id, id)
                .await
            {
                tracing::warn!(session_id = %id, error = %error, "failed to bind agent context");
            }
        }
    }

    // ── Session management handlers ───────────────────────

    /// Uniform refusal for obsolete context-management commands.
    fn refuse_context_change(&self) {
        self.shared.send_event(AgentEvent::Error(
            "SINGLE_CONTEXT_AGENT: each agent has one persistent context; create another agent for independent work".into(),
        ));
    }

    /// Recognize the obsolete creation command so older senders get an
    /// explicit refusal. Only an empty agent may initialize its sole
    /// context this way; replacement and fork arguments are refused.
    async fn handle_create_session(
        &mut self,
        id: Uuid,
        fork_from: Option<Uuid>,
        fork_message_id: Option<&str>,
        title: Option<String>,
    ) {
        if self.context.is_some()
            || self.recovery_notice.is_some()
            || fork_from.is_some()
            || fork_message_id.is_some()
        {
            self.refuse_context_change();
            return;
        }
        // Compatibility for internal initialization of an empty agent only.
        let mut context = Session::new(id, self.shared.clone());
        context.title = title;
        context.cancel_token = self.cancel_token.clone();
        context.resume().await;
        self.bind_context(id).await;
        if let Some(ref t) = context.title {
            self.persist_session_title(id, t).await;
        }
        self.shared.send_event(AgentEvent::SessionActivated {
            id,
            title: context.title.clone(),
        });
        self.context = Some(context);
        self.shared.send_event(AgentEvent::SessionList {
            sessions: self.session_infos(),
        });
    }

    /// Legacy selection command: only the already-bound context id is
    /// accepted (as a no-op); foreign ids are refused.
    async fn handle_switch_session(&mut self, id: Uuid) {
        if self.context.as_ref().is_none_or(|context| context.id != id) {
            self.refuse_context_change();
        }
    }

    /// Rename the sole context (same-id renames only).
    async fn handle_rename_session(&mut self, id: Uuid, title: String) {
        if self.context.as_ref().is_none_or(|context| context.id != id) {
            self.refuse_context_change();
            return;
        }
        if let Some(s) = self.context.as_mut() {
            s.title = Some(title.clone());
            tracing::info!(session_id = %id, title = %title, "context renamed");
        }
        if let Some(storage) = &self.shared.storage {
            let _ = storage.update_session_title(id, &title).await;
        }
        self.shared.send_event(AgentEvent::SessionActivated {
            id,
            title: Some(title),
        });
    }

    /// Legacy close command: refused; the context lives as long as its
    /// agent and its history stays readable through storage APIs.
    async fn handle_close_session(&mut self, _id: Uuid) {
        self.refuse_context_change();
    }

    /// Dispatch a conversation event to the active session.
    async fn apply_event(&mut self, event: InternalEvent) -> bool {
        match event {
            InternalEvent::Shutdown => {
                if let Some(s) = self.active_session_mut() {
                    s.set_lifecycle(agentik_types::AgentLifecycleStatus::Idle);
                }
                false
            }
            InternalEvent::Done => {
                if let Some(s) = self.active_session_mut() {
                    s.stop();
                }
                false
            }
            InternalEvent::ResetCancelToken(ref token) => {
                self.cancel_token = token.clone();
                match self.active_session_mut() {
                    Some(session) => session.apply_internal_event(event).await,
                    None => true,
                }
            }
            _ => {
                // While recovery is pending, conversation events are refused
                // with the stored notice instead of running against no (or a
                // wrong) context.
                if let Some(notice) = &self.recovery_notice {
                    self.shared.send_event(AgentEvent::Error(notice.clone()));
                    return true;
                }
                if let Some(session) = self.active_session_mut() {
                    session.apply_internal_event(event).await
                } else {
                    true
                }
            }
        }
    }

    async fn persist_session_title(&self, id: Uuid, title: &str) {
        if let Some(storage) = &self.shared.storage {
            if let Err(e) = storage.as_ref().update_session_title(id, title).await {
                tracing::warn!(
                    session_id = %id,
                    error = %e,
                    "failed to persist session title"
                );
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// TokenBudget (stays here — re-exported)
// ═══════════════════════════════════════════════════════════════════════

/// Soft compaction trigger: fire when context usage reaches this percentage
/// of the model's `context_length`. Matches Codex's default
/// `auto_compact_token_limit` derivation (`context_window * 90%`).
const AUTO_COMPACT_THRESHOLD_PERCENT: u64 = 90;

/// Tracks token usage for compaction decisions.
///
/// Ported from Codex: uses real API-reported `total_tokens` as the primary
/// signal, with char-based estimation for items appended since the last API
/// response (tool results, new user messages).
#[derive(Default)]
pub struct TokenBudget {
    /// Last API-reported total tokens (input + output) from the most recent
    /// model response. This is the most accurate context-size signal.
    pub(crate) last_api_total: u64,
    /// Estimated tokens of conversation items added *after* the last API
    /// response (tool results, injected messages). Added to `last_api_total`
    /// for the next pre-request compaction check.
    pub(crate) pending_tokens: u64,
}

impl TokenBudget {
    /// Record real token usage from an API response.
    ///
    /// Uses [`Usage::total_input_tokens`] — `input_tokens` plus cache reads
    /// and cache writes — **not** [`Usage::total_tokens`]. Anthropic reports
    /// `input_tokens` as only the *newly added, uncached* portion, so with
    /// prompt caching active (this codebase deliberately keeps the system
    /// prompt byte-stable to maximise the cached prefix) `total_tokens()`
    /// reads as a tiny fraction of the real prompt size. Compaction would
    /// then never trigger while the UI correctly showed a nearly full
    /// context window. `output_tokens` is included so a long reply nudges
    /// the threshold slightly early — the safe direction.
    pub fn record_api_usage(&mut self, usage: &agentik_types::Usage) {
        self.last_api_total = usage
            .total_input_tokens()
            .saturating_add(usage.output_tokens);
        self.pending_tokens = 0;
    }

    /// Accumulate estimated tokens for a message added since last API response.
    pub fn add_pending_message(&mut self, msg: &agentik_sdk::types::Message) {
        self.pending_tokens += crate::session::estimate_message_tokens(msg);
    }

    /// Current best estimate of total context tokens.
    pub fn current_total(&self) -> u64 {
        self.last_api_total.saturating_add(self.pending_tokens)
    }

    /// Estimated context fill percentage, using the same total that
    /// [`should_compact`](Self::should_compact) considers. Returns 0 when
    /// the model's context length is unknown — display-only, never a
    /// compaction trigger.
    pub fn context_fill_pct(
        &self,
        messages: &[agentik_sdk::types::Message],
        context_length: u64,
    ) -> u64 {
        if context_length == 0 {
            return 0;
        }
        let total = if self.last_api_total > 0 {
            self.current_total()
        } else {
            self.estimate_messages_tokens(messages)
        };
        total.saturating_mul(100) / context_length
    }

    /// Fallback: estimate tokens by iterating messages (chars/4 heuristic).
    pub fn estimate_messages_tokens(&self, messages: &[agentik_sdk::types::Message]) -> u64 {
        messages
            .iter()
            .map(crate::session::estimate_message_tokens)
            .sum()
    }

    /// Returns `true` when context usage reaches the auto-compact threshold
    /// (90&nbsp;% of `context_length`).
    ///
    /// Prefers real API usage + pending estimates; falls back to pure
    /// message-level estimation when no API response has been received yet
    /// (e.g. first turn).
    ///
    /// Returns `false` when `context_length` is 0 — the threshold would
    /// compute to 0 and `total >= 0` would hold unconditionally, compacting
    /// on every request. Reachable via `ModelInfo::default()` and
    /// user-defined models whose Context length was left unset. Matches the
    /// unknown-window handling in [`context_fill_pct`](Self::context_fill_pct).
    pub fn should_compact(
        &self,
        messages: &[agentik_sdk::types::Message],
        context_length: u64,
    ) -> bool {
        if context_length == 0 {
            return false;
        }
        let total = if self.last_api_total > 0 {
            self.current_total()
        } else {
            self.estimate_messages_tokens(messages)
        };
        let threshold = context_length.saturating_mul(AUTO_COMPACT_THRESHOLD_PERCENT) / 100;
        total >= threshold
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Persistence worker
// ═══════════════════════════════════════════════════════════════════════

async fn persist_worker(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<PersistOp>,
    storage: Arc<dyn AgentStorage>,
) {
    while let Some(op) = rx.recv().await {
        let result = match op {
            PersistOp::StartSession {
                agent_id,
                session_id,
            } => storage.start_session(agent_id, session_id).await,
            PersistOp::AppendMessage {
                session_id,
                message,
            } => storage.append_message(session_id, &message).await,
            PersistOp::EndSession { session_id } => storage.end_session(session_id).await,
            PersistOp::ReplaceSessionState {
                agent_id,
                session_id,
                state,
            } => {
                storage
                    .replace_session_state(agent_id, session_id, &state)
                    .await
            }
            PersistOp::UpdateSessionTelemetry {
                session_id,
                telemetry,
            } => {
                storage
                    .update_session_telemetry(session_id, &telemetry)
                    .await
            }
            PersistOp::ArchiveTranscript {
                session_id,
                messages,
            } => storage.archive_transcript(session_id, &messages).await,
        };
        if let Err(e) = result {
            tracing::warn!("persist op failed (non-fatal): {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TursoAgentStorage;
    use crate::testing::dummy_model_info;
    use agentik_sdk::AgentMessageExt;
    use agentik_sdk::model::Model;
    use agentik_sdk::model::model_info::ModelInfo;
    use agentik_sdk::provider::client::MockApiClient;
    use agentik_sdk::types::errors::AnthropicError;
    use agentik_sdk::types::messages::{ContentBlock, Message};

    fn test_model_info() -> ModelInfo {
        dummy_model_info("test-model")
    }

    /// Build a minimal agent with an event receiver wired up.
    async fn build_test_agent(
        mock_api: MockApiClient,
    ) -> (Agent, tokio::sync::mpsc::UnboundedReceiver<AgentEvent>) {
        let model = Model::with_client(test_model_info(), mock_api);

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        let agent = Agent::builder()
            .with_model(Arc::new(ArcSwapOption::from_pointee(Some(model))))
            .with_config(AgentConfig {
                max_iterations: 5,
                max_retries: 0,
            })
            .with_agent_event_tx(tx)
            .build()
            .await
            .unwrap();

        (agent, rx)
    }

    #[tokio::test]
    async fn auto_created_session_title_is_persisted() {
        let storage = std::sync::Arc::new(TursoAgentStorage::open_in_memory().await.unwrap());
        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        cancel_token.cancel();

        let mut agent = Agent::builder()
            .with_storage(storage.clone())
            .with_agent_event_tx(event_tx)
            .with_cancel_token(cancel_token)
            .build()
            .await
            .unwrap();
        let agent_id = agent.id();
        // The auto-created context is titled with the agent's own name.
        let agent_name = agent.name().to_string();
        let internal_tx = agent.internal_event_tx();
        let task = tokio::spawn(async move {
            agent.run().await;
        });

        internal_tx
            .send(InternalEvent::MessageInject {
                content: vec![ContentBlock::Text {
                    text: "hello".into(),
                }],
                from_user: true,
                delegation_id: None,
            })
            .unwrap();

        let activated_title = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                match event_rx.recv().await {
                    Some(AgentEvent::SessionActivated { title, .. }) => break title,
                    Some(_) => continue,
                    None => panic!("agent event channel closed"),
                }
            }
        })
        .await
        .expect("auto-created session activation timed out");
        assert_eq!(activated_title.as_deref(), Some(agent_name.as_str()));

        internal_tx.send(InternalEvent::Shutdown).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), task)
            .await
            .expect("agent shutdown timed out")
            .unwrap();

        let records = storage.list_session_records(agent_id).await.unwrap();
        let title = records
            .iter()
            .find(|record| record.title.is_some())
            .and_then(|record| record.title.clone());
        assert_eq!(title.as_deref(), Some(agent_name.as_str()));
    }

    /// The auto-created context is permanently bound as the agent's sole
    /// context: a restart restores exactly that context, never a new one.
    #[tokio::test]
    async fn auto_created_context_is_bound_and_restored() {
        let storage = std::sync::Arc::new(TursoAgentStorage::open_in_memory().await.unwrap());
        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        cancel_token.cancel();

        let mut agent = Agent::builder()
            .with_storage(storage.clone())
            .with_agent_event_tx(event_tx)
            .with_cancel_token(cancel_token)
            .build()
            .await
            .unwrap();
        let agent_id = agent.id();
        let internal_tx = agent.internal_event_tx();
        let task = tokio::spawn(async move {
            agent.run().await;
        });

        internal_tx
            .send(InternalEvent::MessageInject {
                content: vec![ContentBlock::Text {
                    text: "hello".into(),
                }],
                from_user: true,
                delegation_id: None,
            })
            .unwrap();

        let activated_id = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                match event_rx.recv().await {
                    Some(AgentEvent::SessionActivated { id, .. }) => break id,
                    Some(_) => continue,
                    None => panic!("agent event channel closed"),
                }
            }
        })
        .await
        .expect("auto-created session activation timed out");

        internal_tx.send(InternalEvent::Shutdown).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), task)
            .await
            .expect("agent shutdown timed out")
            .unwrap();

        // The binding is durable.
        assert_eq!(
            storage.agent_context_id(agent_id).await.unwrap(),
            Some(activated_id)
        );

        // A second run of the same agent restores the bound context and
        // reports it via ListSessions.
        let (event_tx2, mut event_rx2) = tokio::sync::mpsc::unbounded_channel();
        let mut agent2 = Agent::builder()
            .with_id(agent_id)
            .with_storage(storage.clone())
            .with_agent_event_tx(event_tx2)
            .build()
            .await
            .unwrap();
        assert_eq!(agent2.id(), agent_id);
        let internal_tx2 = agent2.internal_event_tx();
        let task2 = tokio::spawn(async move {
            agent2.run().await;
        });
        internal_tx2.send(InternalEvent::ListSessions).unwrap();
        let listed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                match event_rx2.recv().await {
                    Some(AgentEvent::SessionList { sessions }) => break sessions,
                    Some(_) => continue,
                    None => panic!("agent event channel closed"),
                }
            }
        })
        .await
        .expect("session list timed out");
        internal_tx2.send(InternalEvent::Shutdown).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), task2)
            .await
            .expect("agent shutdown timed out")
            .unwrap();

        assert_eq!(listed.len(), 1, "exactly one context is restored");
        assert_eq!(listed[0].id, activated_id);
    }

    /// Legacy multi-session agents bind their most recently active record
    /// once; older histories stay readable but are never restored.
    #[tokio::test]
    async fn legacy_sessions_bind_most_recent_record() {
        let storage = std::sync::Arc::new(TursoAgentStorage::open_in_memory().await.unwrap());
        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        cancel_token.cancel();

        let mut agent = Agent::builder()
            .with_storage(storage.clone())
            .with_agent_event_tx(event_tx)
            .with_cancel_token(cancel_token)
            .build()
            .await
            .unwrap();
        let agent_id = agent.id();

        // Two legacy session rows with strictly ordered activity: the newer
        // row starts (and is therefore last active) strictly later.
        let older = uuid::Uuid::new_v4();
        let newer = uuid::Uuid::new_v4();
        storage.start_session(agent_id, older).await.unwrap();
        storage.update_session_title(older, "older").await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        storage.start_session(agent_id, newer).await.unwrap();
        storage.update_session_title(newer, "newer").await.unwrap();

        let internal_tx = agent.internal_event_tx();
        let task = tokio::spawn(async move {
            agent.run().await;
        });
        internal_tx.send(InternalEvent::ListSessions).unwrap();
        let listed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                match event_rx.recv().await {
                    Some(AgentEvent::SessionList { sessions }) => break sessions,
                    Some(_) => continue,
                    None => panic!("agent event channel closed"),
                }
            }
        })
        .await
        .expect("session list timed out");
        internal_tx.send(InternalEvent::Shutdown).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), task)
            .await
            .expect("agent shutdown timed out")
            .unwrap();

        // The most recently active legacy record became the sole context.
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, newer);
        assert_eq!(listed[0].title.as_deref(), Some("newer"));
        assert_eq!(
            storage.agent_context_id(agent_id).await.unwrap(),
            Some(newer)
        );
        // The older history is retained read-only.
        let records = storage.list_session_records(agent_id).await.unwrap();
        assert_eq!(records.len(), 2, "legacy histories are retained");
    }

    #[allow(dead_code)]
    async fn collect_events(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    ) -> Vec<AgentEvent> {
        let mut events = vec![];
        while let Some(e) = rx.recv().await {
            events.push(e);
        }
        events
    }

    #[tokio::test]
    async fn test_compaction_noop_does_not_loop() {
        use crate::session::SessionState;

        let messages = vec![Message::user("x".repeat(500_000))];
        let state = SessionState {
            messages: messages.clone(),
            summary: None,
            ancestor_summaries: Vec::new(),
            telemetry: Default::default(),
        };

        let budget = TokenBudget::default();
        let context_length = 128_000u64;

        let msgs = state.messages;

        assert!(
            budget.should_compact(&msgs, context_length),
            "should_compact must fire for a 500K-char single-segment conversation"
        );
    }

    /// An unknown context window (`ModelInfo::default()`, or a user-defined
    /// model whose Context length was left unset) must never trigger
    /// compaction. The threshold would compute to 0, making `total >= 0`
    /// hold unconditionally and compacting on every single request.
    #[test]
    fn should_compact_is_false_when_context_length_unknown() {
        let msgs = vec![Message::user("x".repeat(500_000))];

        // Estimate path (no API response recorded yet).
        assert!(
            !TokenBudget::default().should_compact(&msgs, 0),
            "unknown window must not compact off the estimate path"
        );

        // API-usage path (last_api_total > 0) — same zero threshold.
        // 125_000 tokens sits above 90% of a 128k window.
        let mut budget = TokenBudget::default();
        budget.record_api_usage(&agentik_types::Usage {
            input_tokens: 120_000,
            output_tokens: 5_000,
            ..Default::default()
        });
        assert!(
            !budget.should_compact(&msgs, 0),
            "unknown window must not compact off the API-usage path"
        );

        // The display-side sibling agrees.
        assert_eq!(budget.context_fill_pct(&msgs, 0), 0);

        // And a known window still fires at the same usage.
        assert!(
            budget.should_compact(&msgs, 128_000),
            "a known window above 90% must still trigger"
        );
    }

    /// Regression: compaction must account for *cached* prompt tokens.
    ///
    /// Anthropic's `input_tokens` counts only newly added, uncached tokens.
    /// With prompt caching on, a nearly-full 200k context reports a tiny
    /// `input_tokens` and a huge `cache_read_input_tokens`. The budget used
    /// to read `input_tokens + output_tokens`, so a session at 91% of its
    /// window measured as ~1.4% and auto-compaction never fired — while
    /// the TUI (which has always summed all three) showed the red
    /// over-threshold warning. This pins the two views together.
    #[test]
    fn cached_prompt_tokens_count_toward_the_compaction_threshold() {
        let usage = agentik_types::Usage {
            input_tokens: 2_000,
            output_tokens: 800,
            cache_creation_input_tokens: Some(0),
            cache_read_input_tokens: Some(180_000),
            ..Default::default()
        };
        assert_eq!(
            usage.total_tokens(),
            2_800,
            "the naive sum is what used to drive the threshold"
        );

        let mut budget = TokenBudget::default();
        budget.record_api_usage(&usage);

        assert_eq!(budget.current_total(), 182_800);
        assert_eq!(
            budget.context_fill_pct(&[], 200_000),
            91,
            "the budget must agree with the UI's over-threshold reading"
        );
        assert!(
            budget.should_compact(&[], 200_000),
            "a 91%-full cached context must trigger compaction"
        );

        // A cache-miss first request (no cache fields) still behaves as
        // before — the fix must not disturb the uncached path.
        let uncached = agentik_types::Usage {
            input_tokens: 190_000,
            output_tokens: 500,
            ..Default::default()
        };
        let mut budget = TokenBudget::default();
        budget.record_api_usage(&uncached);
        assert!(budget.should_compact(&[], 200_000));

        // Pending messages still add on top of the cached baseline.
        let mut budget = TokenBudget::default();
        budget.record_api_usage(&usage);
        budget.add_pending_message(&Message::user("x".repeat(40_000))); // ~10k tokens
        assert_eq!(budget.current_total(), 192_800);
    }

    #[tokio::test]
    async fn stream_api_errors_are_retried() {
        let mut mock = MockApiClient::new();
        mock.expect_request_stream_with_system()
            .times(2)
            .returning(|_, _, _, _| {
                Err(AnthropicError::StreamError(
                    "transient stream failure".to_string(),
                ))
            });

        let model = Model::with_client(test_model_info(), mock);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut agent = Agent::builder()
            .with_model(Arc::new(ArcSwapOption::from_pointee(Some(model))))
            .with_config(AgentConfig {
                max_iterations: 5,
                max_retries: 1,
            })
            .with_agent_event_tx(tx)
            .build()
            .await
            .unwrap();

        let internal_tx = agent.internal_event_tx();
        internal_tx
            .send(InternalEvent::MessageInject {
                content: vec![ContentBlock::Text {
                    text: "hello".into(),
                }],
                from_user: true,
                delegation_id: None,
            })
            .unwrap();
        let task = tokio::spawn(async move {
            agent.run().await;
        });

        let mut saw_retryable_error = false;
        let mut saw_terminal_error = false;
        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::RetryableError { attempt, .. } => {
                    saw_retryable_error = true;
                    assert_eq!(attempt, 1);
                }
                AgentEvent::Error(message) => {
                    assert!(message.contains("transient stream failure"));
                    saw_terminal_error = true;
                    break;
                }
                _ => {}
            }
        }
        internal_tx.send(InternalEvent::Shutdown).unwrap();
        task.await.unwrap();

        assert!(saw_retryable_error);
        assert!(saw_terminal_error);
    }

    #[tokio::test]
    #[ignore]
    async fn test_events_received_on_simple_text_response() {
        let mock = MockApiClient::new();
        let (mut agent, mut rx) = build_test_agent(mock).await;

        tokio::spawn(async move {
            let _ = agent.run().await;
        });

        let _events = collect_events(&mut rx).await;
    }
}
