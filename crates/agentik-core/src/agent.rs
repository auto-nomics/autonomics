//! # Agent — session manager and shared resource owner.
//!
//! An `Agent` owns stable resources (model, tool registry, storage, prompt
//! configuration) and manages one or more [`Session`](crate::Session)s — each
//! representing an independent conversation. Only one session is *active*
//! at a time (single-active model). Switching sessions pauses the current
//! one and activates the target.
//!
//! The outer [`Agent::run`] loop is an event dispatcher: it reads
//! [`InternalEvent`]s from its channel, applies session management events
//! (create / switch / close), and delegates conversation events
//! (`MessageInject`, `BgTaskComplete`) to the active session's
//! [`Session::run_session`].

use std::collections::HashMap;
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
use crate::storage::{AgentRecord, AgentStorage, PersistOp};
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
    /// `from_user` is `true` when the message originated from the TUI user
    /// input (already shown locally — no display event needed). It is `false`
    /// when the message was injected by an external source such as
    /// `delegate_to` / `send_message` (caller hasn't shown it — TUI must
    /// render it via `AgentEvent::MessageInjected`).
    MessageInject {
        content: Vec<ContentBlock>,
        from_user: bool,
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
    /// Create a new session. If `fork_from` is `Some`, deep-clone memory
    /// from the named parent session.
    CreateSession {
        id: Uuid,
        fork_from: Option<Uuid>,
        title: Option<String>,
    },
    /// Switch the active session to `id`.
    SwitchSession {
        id: Uuid,
    },
    /// Close and remove a session.
    CloseSession {
        id: Uuid,
    },
    /// Request a list of all sessions (reply via event channel).
    ListSessions,
    /// Rename a session (update title in memory + storage).
    RenameSession {
        id: Uuid,
        title: String,
    },
    /// Manually trigger compaction on the active session.
    Compact,
}

pub struct Agent {
    pub(crate) shared: Arc<AgentShared>,
    pub(crate) sessions: HashMap<Uuid, Session>,
    pub(crate) active_session_id: Option<Uuid>,
    pub(crate) internal_event_tx: UnboundedSender<InternalEvent>,
    /// Receiver consumed once by [`run()`]; `None` after that.
    pub(crate) internal_event_rx: Option<UnboundedReceiver<InternalEvent>>,
    /// Initial messages from the builder. Used as a fallback when no
    /// session records exist in storage. Consumed during `run()`.
    pub(crate) initial_messages: Option<Vec<agentik_sdk::types::messages::Message>>,
    /// Cancel token from the builder, used when auto-creating sessions.
    pub(crate) cancel_token: CancellationToken,
}

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

    // ── Session accessors ─────────────────────────────────

    pub fn active_session(&self) -> Option<&Session> {
        self.active_session_id.and_then(|id| self.sessions.get(&id))
    }

    pub fn active_session_mut(&mut self) -> Option<&mut Session> {
        self.active_session_id
            .and_then(|id| self.sessions.get_mut(&id))
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

    pub fn session_infos(&self) -> Vec<SessionInfo> {
        self.sessions.values().map(SessionInfo::from).collect()
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

            // ── Restore the agent's persistent plan ──────────
            if let Ok(Some(plan)) = storage.as_ref().load_plan(self.shared.id).await {
                if !plan.is_empty() {
                    self.shared.plan.store(Arc::new(plan));
                    tracing::debug!(
                        agent_id = %self.shared.id,
                        "restored agent plan from storage"
                    );
                }
            }

            // Store in shared so sessions created later can also access it.
            let _ = self.shared.persist_tx.set(persist_tx);
            // Wire into existing sessions.
            for session in self.sessions.values_mut() {
                if let Some(tx) = self.shared.persist_tx.get() {
                    session.persist_tx = Some(tx.clone());
                }
            }
            crate::supervise::spawn_safe_drop(
                "persist_worker",
                persist_worker(persist_rx, Arc::clone(&storage)),
            );

            if let Some(memory) = &self.shared.memory {
                if memory.config.generate_memory && memory.config.is_root_agent(&self.shared.path) {
                    let agent_id = self.shared.id;
                    let storage = Arc::clone(&storage);
                    let model = Arc::clone(&self.shared.model);
                    let config = memory.config.clone();
                    crate::supervise::spawn_safe_drop(
                        "memory_startup_pipeline",
                        crate::memory::run_memory_pipeline(agent_id, storage, model, config),
                    );
                }
            }
        }

        // ── Restore sessions from storage ──
        // Query all session records and rebuild sessions that no longer
        // exist in the HashMap (they were created in a prior run but lost
        // on restart).
        if let Some(storage) = self.shared.storage.clone() {
            match storage.as_ref().list_session_records(self.shared.id).await {
                Ok(records) => {
                    for rec in records {
                        // Skip if this session is already in the HashMap.
                        if self.sessions.contains_key(&rec.session_id) {
                            if let Some(t) = &rec.title {
                                if let Some(s) = self.sessions.get_mut(&rec.session_id) {
                                    if s.title.is_none() {
                                        s.title = Some(t.clone());
                                    }
                                }
                            }
                            continue;
                        }
                        // Rebuild a session from storage using the unified
                        // restore_session_state helper (snapshot + WAL).
                        let mut s = Session::new(rec.session_id, self.shared.clone());
                        s.cancel_token = self.cancel_token.clone();
                        s.title = rec.title;
                        s.created_at = rec.started_at;

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
                                    "failed to restore session state for {}", rec.session_id
                                );
                            }
                        }

                        tracing::info!(
                            session_id = %rec.session_id,
                            title = ?s.title,
                            "restored session from storage"
                        );
                        self.sessions.insert(rec.session_id, s);
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to list session records");
                }
            }
        }

        // ── Fallback: no session records in storage ──
        // If the builder provided initial messages and no sessions were
        // loaded from storage, create one session with those messages.
        if self.sessions.is_empty() {
            if let Some(messages) = self.initial_messages.take() {
                let id = Uuid::new_v4();
                let state = crate::session::SessionState {
                    messages,
                    summary: None,
                    ancestor_summaries: Vec::new(),
                };
                let mut s = Session::new_with_state(
                    id,
                    self.shared.clone(),
                    state,
                    self.cancel_token.clone(),
                );
                s.title = Some("Restored".into());
                s.resume().await;
                self.sessions.insert(id, s);
                self.active_session_id = Some(id);
                tracing::info!("created fallback session from builder initial messages");
            }
        } else if self.active_session_id.is_none() {
            // Sessions exist but none is active — activate the first one.
            if let Some(&id) = self.sessions.keys().next() {
                self.active_session_id = Some(id);
                if let Some(s) = self.sessions.get_mut(&id) {
                    s.resume().await;
                }
            }
        }

        loop {
            let event = match rx.recv().await {
                Some(e) => e,
                None => break,
            };

            // ── Auto-create a session on first message ──
            // If no session exists yet, create one before processing the
            // event so the message isn't lost.
            if matches!(event, InternalEvent::MessageInject { .. }) && self.active_session_id.is_none()
            {
                let id = Uuid::new_v4();
                let mut s = Session::new(id, self.shared.clone());
                s.title = Some("New conversation".into());
                s.cancel_token = self.cancel_token.clone();
                s.resume().await;
                self.sessions.insert(id, s);
                self.active_session_id = Some(id);
                self.shared.send_event(AgentEvent::SessionActivated {
                    id,
                    title: Some("New conversation".into()),
                });
                tracing::info!("auto-created session on first message");
            }

            // Session management events are handled inline before delegation.
            let should_run = match &event {
                InternalEvent::CreateSession {
                    id,
                    fork_from,
                    title,
                } => {
                    self.handle_create_session(*id, *fork_from, title.clone())
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
                if let Some(id) = self.active_session_id {
                    let tx = self.internal_event_tx.clone();
                    if let Some(session) = self.sessions.get_mut(&id) {
                        session.run_session(&tx, &mut rx).await;
                    }
                }
            }
        }

        // Shutdown: cancel background tool tasks, then pause all sessions.
        // Background tasks (e.g. run_dag, run_bash) are tokio::spawn'd and
        // would outlive the agent task; cancelling their tokens lets them
        // exit cleanly before the runtime is dropped.
        for session in self.sessions.values_mut() {
            session.toolset.cancel_all_tasks().await;
            session.pause().await;
        }
    }

    // ── Session management handlers ───────────────────────

    async fn handle_create_session(
        &mut self,
        id: Uuid,
        fork_from: Option<Uuid>,
        title: Option<String>,
    ) {
        let mut session = match fork_from {
            Some(parent_id) => {
                if let Some(parent) = self.sessions.get(&parent_id) {
                    let mut s = Session::fork_from(parent, id, self.shared.clone());
                    s.title = title.clone();
                    s
                } else {
                    tracing::warn!(parent_id = %parent_id, "fork parent not found");
                    let mut s = Session::new(id, self.shared.clone());
                    s.title = title.clone();
                    s
                }
            }
            None => {
                let mut s = Session::new(id, self.shared.clone());
                s.title = title.clone();
                s
            }
        };
        // Ensure the new session shares the agent's cancel token so that
        // AgentHandle::cancel() can interrupt an in-flight run_session.
        session.cancel_token = self.cancel_token.clone();
        self.sessions.insert(id, session);

        // resume() (called by handle_switch_session) creates the sessions
        // table row via start_session(). We must persist the title *after*
        // that row exists, otherwise the UPDATE matches zero rows.
        self.handle_switch_session(id).await;
        if let Some(storage) = &self.shared.storage {
            if let Some(ref t) = title {
                let _ = storage.update_session_title(id, t).await;
            }
        }

        let title = self.sessions.get(&id).and_then(|s| s.title.clone());
        self.shared
            .send_event(AgentEvent::SessionActivated { id, title });
    }

    async fn handle_switch_session(&mut self, id: Uuid) {
        if !self.sessions.contains_key(&id) {
            tracing::warn!(session_id = %id, "switch target not found");
            return;
        }
        if let Some(old) = self.active_session_id {
            if old != id {
                if let Some(s) = self.sessions.get_mut(&old) {
                    s.pause().await;
                }
                self.shared
                    .send_event(AgentEvent::SessionPaused { id: old });
            }
        }
        self.active_session_id = Some(id);
        if let Some(s) = self.sessions.get_mut(&id) {
            s.resume().await;
        }
        let title = self.sessions.get(&id).and_then(|s| s.title.clone());
        self.shared
            .send_event(AgentEvent::SessionActivated { id, title });
    }

    async fn handle_rename_session(&mut self, id: Uuid, title: String) {
        if let Some(s) = self.sessions.get_mut(&id) {
            s.title = Some(title.clone());
            tracing::info!(session_id = %id, title = %title, "session renamed");
        }
        // Persist to storage.
        if let Some(storage) = &self.shared.storage {
            let _ = storage.update_session_title(id, &title).await;
        }
        // Notify TUI.
        self.shared.send_event(AgentEvent::SessionActivated {
            id,
            title: Some(title),
        });
    }

    async fn handle_close_session(&mut self, id: Uuid) {
        if let Some(s) = self.sessions.get_mut(&id) {
            s.pause().await;
        }
        self.sessions.remove(&id);
        // Permanently delete the session from storage so it doesn't
        // reappear as an empty session on restart.
        if let Some(storage) = &self.shared.storage {
            let _ = storage.delete_session(id).await;
        }
        self.shared.send_event(AgentEvent::SessionClosed { id });
        if self.active_session_id == Some(id) {
            self.active_session_id = self.sessions.keys().next().copied();
            if let Some(new_id) = self.active_session_id {
                if let Some(s) = self.sessions.get_mut(&new_id) {
                    s.resume().await;
                }
                let title = self.sessions.get(&new_id).and_then(|s| s.title.clone());
                self.shared
                    .send_event(AgentEvent::SessionActivated { id: new_id, title });
            }
        }
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
            _ => {
                if let Some(session) = self.active_session_mut() {
                    session.apply_internal_event(event).await
                } else {
                    true
                }
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
    pub fn record_api_usage(&mut self, usage: &agentik_types::Usage) {
        self.last_api_total = usage.total_tokens();
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
    pub fn should_compact(
        &self,
        messages: &[agentik_sdk::types::Message],
        context_length: u64,
    ) -> bool {
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
            } => storage
                .replace_session_state(agent_id, session_id, &state)
                .await,
        };
        if let Err(e) = result {
            tracing::warn!("persist op failed (non-fatal): {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message_ext::AgentMessageExt;
    use crate::testing::dummy_model_info;
    use agentik_sdk::model::Model;
    use agentik_sdk::model::model_info::ModelInfo;
    use agentik_sdk::provider::client::MockApiClient;
    use agentik_sdk::types::messages::{ContentBlock, Message, Role};
    use agentik_sdk::types::shared::Usage;

    fn test_model_info() -> ModelInfo {
        dummy_model_info("test-model")
    }

    #[allow(dead_code)]
    fn test_final_message(text: &str) -> Message {
        Message {
            id: "msg_test".into(),
            type_: "message".into(),
            role: Role::Assistant,
            content: vec![ContentBlock::Text { text: text.into() }],
            model: Some("test-model".into()),
            stop_reason: None,
            stop_sequence: None,
            usage: Some(Usage {
                input_tokens: 10,
                output_tokens: 20,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                server_tool_use: None,
                service_tier: None,
            }),
            request_id: None,
        }
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
        };

        let budget = TokenBudget::default();
        let context_length = 128_000u64;

        let msgs = state.messages;

        assert!(
            budget.should_compact(&msgs, context_length),
            "should_compact must fire for a 500K-char single-segment conversation"
        );
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
