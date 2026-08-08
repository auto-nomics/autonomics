//! Session — a single conversation within an Agent.
//!
//! An [`Agent`](crate::Agent) owns one or more `Session`s. Each Session has
//! independent conversation memory, lifecycle, tool execution state, and
//! cancellation token, but shares the model, tool registry, storage, and
//! system-prompt configuration with its siblings.
//!
//! Currently only one session is *active* at a time (single-active model).
//! Switching sessions pauses the current one and activates the target.

use std::sync::Arc;
use std::time::Duration;

use agentik_sdk::model::Model;
use agentik_sdk::types::messages::{ContentBlock, Message, Role};
use agentik_sdk::types::tools::ToolUse;
use agentik_sdk::types::{AgentEvent, AnthropicError, ToolDefinition};
use agentik_types::{AgentPlan, CompactEvent, PlanUpdate, SessionInfo};
use arc_swap::{ArcSwap, ArcSwapOption};
use chrono::Utc;
use futures::StreamExt;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;
use tracing::{Level, span};
use uuid::Uuid;

use crate::agent::{AgentConfig, InternalEvent, TokenBudget};
use crate::context::ContextProvider;
use crate::error::{AgentError, Result, Retryable};
use crate::lifecycle::AgentLifecycle;
use crate::memory::Memory;
use crate::message_ext::AgentMessageExt;
use crate::prompt::system_prompt_builder;
use crate::skill::SharedSkillRuntime;
use crate::storage::{AgentSnapshot, AgentStorage, PersistOp};
use crate::tools::task_runtime::TaskStore;
use crate::tools::{ToolRegistry, Toolset};

// ─────────────────────────── AgentShared ───────────────────────────

/// Stable resources shared across all sessions of one agent.
///
/// Built once by [`AgentBuilder`](crate::agent_builder::AgentBuilder),
/// frozen into `Arc`, and cloned into every [`Session`]. Fields that need
/// runtime mutation use interior mutability (`ArcSwap`, etc.).
pub(crate) struct AgentShared {
    pub id: Uuid,
    pub name: String,
    pub config_json: serde_json::Value,
    pub model: Arc<ArcSwapOption<Model>>,
    pub config: AgentConfig,
    pub storage: Option<Arc<dyn AgentStorage>>,
    pub context_provider: Option<Arc<dyn ContextProvider>>,
    pub system_prompt_section: Option<String>,
    pub system_prompt_identity: Option<String>,
    pub skill_runtime: Option<SharedSkillRuntime>,
    pub tool_registry: Arc<ToolRegistry>,
    /// Shared background-task list — the same Arc baked into the registry's
    /// task tools (`wait_task`, `view_task_results`, `view_task_status`).
    /// Sessions MUST create their Toolset with this handle (via
    /// `from_registry_with_tasks`) so that background tasks spawned by the
    /// Toolset are visible to the task-viewer tools.
    pub tasks: Arc<tokio::sync::RwLock<TaskStore>>,
    /// Event channel for external observers. Uses `ArcSwap` for interior
    /// mutability so `set_event_tx` works even after sessions are created.
    pub event_tx: ArcSwapOption<UnboundedSender<AgentEvent>>,
    /// WAL persistence sender, set once during `Agent::run()` bootstrap.
    /// Sessions created after bootstrap read this to wire their
    /// `Memory::persist_tx`.
    pub persist_tx: std::sync::OnceLock<UnboundedSender<PersistOp>>,
    /// The agent's persistent task plan — a first-class citizen that lives
    /// as long as the agent does. Updated via the `update_plan` tool.
    /// Uses `Arc<ArcSwap>` for lock-free reads and sharing with the tool.
    pub plan: Arc<ArcSwap<AgentPlan>>,
}

impl AgentShared {
    /// Load the current event sender, if any.
    pub fn event_tx(&self) -> Option<UnboundedSender<AgentEvent>> {
        self.event_tx.load_full().as_deref().cloned()
    }

    /// Send an event to the optional observation channel.
    pub fn send_event(&self, event: AgentEvent) {
        if let Some(tx) = self.event_tx.load_full().as_deref() {
            let _ = tx.send(event);
        }
    }

    // ── Plan (first-class persistent task plan) ───────────

    /// Load a snapshot of the current plan.
    pub fn plan_snapshot(&self) -> AgentPlan {
        AgentPlan::clone(&self.plan.load())
    }

    /// Atomically replace the plan, bumping its revision.
    ///
    /// Returns the new revision. The caller is responsible for persisting
    /// the update and emitting a [`AgentEvent::PlanUpdate`] event if needed.
    pub fn replace_plan(&self, update: PlanUpdate) -> u64 {
        let mut new_plan = AgentPlan::clone(&self.plan.load());
        new_plan.replace(update);
        let revision = new_plan.revision;
        self.plan.store(Arc::new(new_plan));
        revision
    }
}

// ─────────────────────────── Session ───────────────────────────

/// One conversation within an agent. Owns the memory, lifecycle, toolset,
/// and cancellation token for that conversation.
pub struct Session {
    pub id: Uuid,
    pub title: Option<String>,
    pub created_at: i64,
    pub last_active: i64,

    pub memory: Memory,
    pub lifecycle: AgentLifecycle,
    pub toolset: Toolset,
    pub token_budget: TokenBudget,
    pub cancel_token: CancellationToken,

    /// Back-reference to shared agent resources.
    pub(crate) shared: Arc<AgentShared>,
}

impl Session {
    /// Create a new empty session.
    pub(crate) fn new(id: Uuid, shared: Arc<AgentShared>) -> Self {
        let toolset = Toolset::from_registry_with_tasks(
            shared.tool_registry.clone(),
            shared.tasks.clone(),
            shared.event_tx(),
        );
        let now = chrono::Utc::now().timestamp_millis();
        let mut memory = Memory::new();
        // Wire persist_tx if the agent's persist worker is already running.
        if let Some(tx) = shared.persist_tx.get() {
            memory.persist_tx = Some(tx.clone());
        }
        Self {
            id,
            title: None,
            created_at: now,
            last_active: now,
            memory,
            lifecycle: AgentLifecycle::new(),
            toolset,
            token_budget: TokenBudget::default(),
            cancel_token: CancellationToken::new(),
            shared,
        }
    }

    /// Create a session pre-loaded with the given memory (restore path).
    pub(crate) fn new_with_memory(
        id: Uuid,
        shared: Arc<AgentShared>,
        memory: Memory,
        cancel_token: CancellationToken,
    ) -> Self {
        let toolset = Toolset::from_registry_with_tasks(
            shared.tool_registry.clone(),
            shared.tasks.clone(),
            shared.event_tx(),
        );
        let now = chrono::Utc::now().timestamp_millis();
        Self {
            id,
            title: None,
            created_at: now,
            last_active: now,
            memory,
            lifecycle: AgentLifecycle::new(),
            toolset,
            token_budget: TokenBudget::default(),
            cancel_token,
            shared,
        }
    }

    /// Fork a new session from an existing one, deep-cloning its memory.
    pub(crate) fn fork_from(parent: &Session, new_id: Uuid, shared: Arc<AgentShared>) -> Self {
        let toolset = Toolset::from_registry_with_tasks(
            shared.tool_registry.clone(),
            shared.tasks.clone(),
            shared.event_tx(),
        );
        let now = chrono::Utc::now().timestamp_millis();
        let mut memory = parent.memory.clone();
        if let Some(tx) = shared.persist_tx.get() {
            memory.persist_tx = Some(tx.clone());
        }
        Self {
            id: new_id,
            title: Some(format!(
                "Fork of {}",
                parent.title.as_deref().unwrap_or("session")
            )),
            created_at: now,
            last_active: now,
            memory,
            lifecycle: AgentLifecycle::new(),
            toolset,
            token_budget: TokenBudget::default(),
            cancel_token: CancellationToken::new(),
            shared,
        }
    }

    // ── Accessors ──────────────────────────────────────────

    pub fn lifecycle_status(&self) -> agentik_types::AgentLifecycleStatus {
        *self.lifecycle.status()
    }

    pub fn is_running(&self) -> bool {
        self.lifecycle.is_running()
    }

    pub fn snapshot(&self) -> AgentSnapshot {
        AgentSnapshot {
            snapshot_id: Uuid::new_v4(),
            ts: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64,
            agent_id: self.shared.id,
            agent_status: *self.lifecycle.status(),
            memory: self.memory.clone(),
            session_id: Some(self.id),
        }
    }

    pub async fn persist_snapshot(&self) {
        let snapshot = self.snapshot();
        if let Some(storage) = &self.shared.storage {
            let _ = storage.as_ref().create_snapshot(snapshot).await;
        }
    }

    pub fn inject_message(&mut self, user_content: Vec<ContentBlock>) -> Result<()> {
        let message = Message {
            id: Uuid::new_v4().to_string(),
            type_: "message".to_string(),
            role: Role::User,
            content: user_content,
            model: None,
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            request_id: None,
        };
        self.memory.remember(message)?;
        Ok(())
    }

    pub fn set_cancel_token(&mut self, token: CancellationToken) {
        self.cancel_token = token;
    }

    // ── Pause / Resume ────────────────────────────────────

    /// Pause the session: lifecycle → IDLE, persist snapshot, end WAL session.
    pub async fn pause(&mut self) {
        if self.lifecycle.is_running() {
            self.lifecycle.set_idle();
        }
        self.persist_snapshot().await;
        if let Some(storage) = &self.shared.storage {
            if let Some(sid) = self.memory.current_session {
                let _ = storage.end_session(sid).await;
            }
        }
        self.memory.current_session = None;
    }

    /// Resume the session: open WAL session (if not already open), lifecycle → IDLE.
    pub async fn resume(&mut self) {
        // Use Session.id as the WAL session ID so metadata (title) can be
        // queried by the same key. Only start a WAL session if we don't
        // already have one.
        if self.memory.current_session.is_none() {
            let wal_id = self.id;
            if let Some(storage) = &self.shared.storage {
                let _ = storage.start_session(self.shared.id, wal_id).await;
                let _ = storage.touch_agent(self.shared.id).await;
            }
            self.memory.current_session = Some(wal_id);
        }
        self.lifecycle.set_idle();
        self.last_active = chrono::Utc::now().timestamp_millis();
    }

    // ── Apply internal events ─────────────────────────────

    /// Apply a single [`InternalEvent`] against session state.
    ///
    /// Returns `true` for events that represent new work; `false` for
    /// terminal control signals.
    pub(crate) async fn apply_internal_event(&mut self, event: InternalEvent) -> bool {
        match event {
            InternalEvent::MessageInject(content) => {
                let _ = self.inject_message(content);
                true
            }
            InternalEvent::BgTaskComplete { id: _, seq } => {
                if let Some((name, ok, content)) =
                    self.toolset.finished_task_notification(seq).await
                {
                    self.shared
                        .send_event(AgentEvent::ToolBackgroundComplete { seq, ok });
                    let note = if ok {
                        format!(
                            "Background task '{name}' (#{seq}) finished. \
                             Call `view_task_results` with task={seq} to read its result."
                        )
                    } else {
                        format!(
                            "Background task '{name}' (#{seq}) finished with an error: {content}. \
                             Call `view_task_results` with task={seq} to read the error."
                        )
                    };
                    let _ = self.memory.remember(Message::user(note));
                }
                true
            }
            InternalEvent::Shutdown => {
                self.lifecycle.set_aborted();
                false
            }
            InternalEvent::Done => {
                self.stop();
                false
            }
            InternalEvent::ResetCancelToken(token) => {
                self.cancel_token = token;
                true
            }
            // Session management events are handled by Agent, never reach
            // a Session. Return true (no-op) defensively.
            InternalEvent::CreateSession { .. }
            | InternalEvent::SwitchSession { .. }
            | InternalEvent::CloseSession { .. }
            | InternalEvent::ListSessions
            | InternalEvent::RenameSession { .. } => true,
        }
    }

    // ── Session loop ──────────────────────────────────────

    /// Run one "session": a sequence of LLM round-trips until the agent
    /// goes idle, hits an error, or is cancelled.
    pub(crate) async fn run_session(
        &mut self,
        internal_event_tx: &UnboundedSender<InternalEvent>,
        rx: &mut UnboundedReceiver<InternalEvent>,
    ) {
        self.lifecycle.set_running();
        self.shared
            .send_event(AgentEvent::LlmResponse("🤖 Agent started".into()));

        // ── Reuse or start a persisted WAL session ───────────
        // The WAL session ID is always the Session's own ID. This is
        // consistent with `resume()` and prevents session proliferation
        // (each turn would otherwise create a new sessions-table row).
        let wal_session_id = self.id;
        if self.memory.current_session.is_none() {
            if let Some(storage) = &self.shared.storage {
                let _ = storage.start_session(self.shared.id, wal_session_id).await;
                let _ = storage.touch_agent(self.shared.id).await;
            }
            self.memory.current_session = Some(wal_session_id);
        }

        let mut iteration = 0;
        let mut consecutive_retries = 0;
        let mut was_cancelled = false;

        loop {
            // Re-capture the cancel token each iteration so that
            // ResetCancelToken updates (applied during the drain below)
            // are reflected. Without this, a prior cancel() would leave
            // `cancelled` pointing at the old (already-cancelled) token
            // and the select! below would never fire on a fresh token.
            let cancelled = self.cancel_token.clone();

            if iteration >= self.shared.config.max_iterations
                || !self.lifecycle.is_running()
                || cancelled.is_cancelled()
            {
                was_cancelled = was_cancelled || cancelled.is_cancelled();
                break;
            }

            iteration += 1;

            let result = tokio::select! {
                biased;
                _ = cancelled.cancelled() => {
                    self.patch_orphaned_tool_use().await;
                    self.lifecycle.set_aborted();
                    self.persist_snapshot().await;
                    self.shared.send_event(AgentEvent::Error("Task cancelled by user".into()));
                    was_cancelled = true;
                    break;
                }
                result = self.agent_workflow(internal_event_tx, None) => result,
            };

            let session_done = match result {
                Ok(()) => {
                    consecutive_retries = 0;
                    self.persist_snapshot().await;
                    if !self.lifecycle.is_running() {
                        self.shared.send_event(AgentEvent::Done);
                        true
                    } else {
                        false
                    }
                }
                Err(e)
                    if e.is_retryable() && consecutive_retries < self.shared.config.max_retries =>
                {
                    consecutive_retries += 1;
                    tracing::warn!(
                        "retryable error at iteration {}/{}, retry {}/{}: {e}",
                        iteration,
                        self.shared.config.max_iterations,
                        consecutive_retries,
                        self.shared.config.max_retries
                    );
                    self.shared.send_event(AgentEvent::RetryableError {
                        message: format!("{e}"),
                        attempt: consecutive_retries as u32,
                        max_retries: self.shared.config.max_retries as u32,
                    });
                    let delay = Duration::from_secs(1) * (1 << (consecutive_retries - 1));
                    tokio::select! {
                        biased;
                        _ = cancelled.cancelled() => {
                            was_cancelled = true;
                            break;
                        }
                        _ = tokio::time::sleep(delay) => {}
                    }
                    let _ = self.memory.remember(Message::user(e.retry_message()));
                    continue;
                }
                Err(e) => {
                    tracing::error!(
                        iteration,
                        error = %e,
                        error_chain = ?e,
                        "workflow failed"
                    );
                    self.shared.send_event(AgentEvent::Error(format!("{e}")));
                    self.persist_snapshot().await;
                    self.lifecycle.set_idle();
                    true
                }
            };

            if session_done {
                break;
            }

            // Drain control/notification events that arrived during this iteration.
            // Session management events (CreateSession, SwitchSession, etc.)
            // must NOT be consumed here — they need to reach the outer
            // Agent::run() loop. Re-queue them and signal this session to
            // return so the outer loop can process them.
            let mut terminal = false;
            let mut session_mgmt_pending = false;
            while let Ok(event) = rx.try_recv() {
                if matches!(
                    event,
                    InternalEvent::CreateSession { .. }
                        | InternalEvent::SwitchSession { .. }
                        | InternalEvent::CloseSession { .. }
                        | InternalEvent::ListSessions
                        | InternalEvent::RenameSession { .. }
                ) {
                    // Re-queue for the outer Agent::run() loop.
                    let _ = internal_event_tx.send(event);
                    session_mgmt_pending = true;
                    break;
                }
                if !self.apply_internal_event(event).await {
                    terminal = true;
                    break;
                }
            }
            if session_mgmt_pending {
                // Stop this session so the outer loop regains control and
                // can process the re-queued session management event.
                self.stop();
                break;
            }
            if terminal {
                break;
            }
        }

        // Post-loop cleanup
        if was_cancelled && self.lifecycle.is_running() {
            self.lifecycle.set_idle();
            self.shared
                .send_event(AgentEvent::Error("Task cancelled by user".into()));
        } else if !self.lifecycle.is_running() {
            self.lifecycle.set_idle();
        }

        // ── Keep the WAL session open for reuse ──────────────
        // Previously this called end_session() and cleared current_session,
        // which caused the next run_session() to generate a *new* UUID and
        // create a fresh sessions-table row — leading to session
        // proliferation (one row per conversation turn).  By keeping the
        // session open we ensure the same WAL row is reused across turns.
        // The session is properly ended only when pause() is called (on
        // session switch or agent shutdown).
    }

    /// After a mid-workflow cancellation, patch orphaned tool_use blocks.
    async fn patch_orphaned_tool_use(&mut self) {
        let Some(last_msg) = self
            .memory
            .items
            .last()
            .and_then(|item| item.messages.last())
        else {
            return;
        };

        let unresolved: Vec<String> = last_msg
            .content
            .iter()
            .filter_map(|c| match c {
                ContentBlock::ToolUse { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect();

        if unresolved.is_empty() {
            return;
        }

        let stub = Message {
            id: Uuid::new_v4().to_string(),
            type_: "message".to_string(),
            role: Role::User,
            content: unresolved
                .into_iter()
                .map(|tc_id| ContentBlock::ToolResult {
                    tool_use_id: tc_id,
                    content: Some("Tool execution has been interrupted".to_string()),
                    is_error: Some(true),
                })
                .collect(),
            model: None,
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            request_id: None,
        };

        tracing::debug!(
            "patching {} orphaned tool_use blocks after cancellation",
            stub.content.len()
        );
        let _ = self.memory.remember(stub);
    }

    /// Core agent workflow: build context → request API → execute tools → memory.
    async fn agent_workflow(
        &mut self,
        internal_event_tx: &UnboundedSender<InternalEvent>,
        retry_feedback: Option<String>,
    ) -> Result<()> {
        if let Some(feedback) = retry_feedback {
            self.inject_message(vec![ContentBlock::Text { text: feedback }])
                .unwrap();
        }

        self.poll_context_provider().await;

        let context = self.build_context().await?;
        let allowed = self.current_allowed_tools().await;

        self.shared.send_event(AgentEvent::Requesting);
        let response_message = self.request(context, allowed.as_deref()).await?;

        let last_usage = response_message.usage.clone().unwrap_or_default();

        for block in &response_message.content {
            match block {
                ContentBlock::Thinking { thinking, .. } if !thinking.is_empty() => {
                    self.shared
                        .send_event(AgentEvent::Thinking(thinking.clone()));
                }
                ContentBlock::Text { text } if !text.is_empty() => {
                    self.shared
                        .send_event(AgentEvent::LlmResponse(text.clone()));
                }
                _ => {}
            }
        }

        self.token_budget.latest_usage = last_usage.input_tokens + last_usage.output_tokens;

        self.memory.remember(response_message.clone())?;

        let toolcalls = self.extract_toolcalls(&response_message);

        if toolcalls.is_empty() {
            self.stop();
            return Ok(());
        }

        for tc in &toolcalls {
            self.shared.send_event(AgentEvent::ToolCall {
                name: tc.name.clone(),
                input: tc.input.clone(),
            });
        }

        let tool_results = self
            .toolset
            .execute(&toolcalls, Some(internal_event_tx.clone()))
            .await?;
        tracing::debug!(?tool_results, "tool execution results");

        for tr in &tool_results {
            self.shared.send_event(AgentEvent::ToolResult {
                ok: !tr.is_error.unwrap_or_default(),
                content: tr.text_content(),
            });
        }

        for tr in &tool_results {
            self.memory.remember(Message::tool_result(
                tr.tool_use_id.clone(),
                tr.text_content(),
                tr.is_error.unwrap_or_default(),
            ))?;
        }

        Ok(())
    }

    pub(crate) fn stop(&mut self) {
        self.lifecycle.set_idle();
    }

    async fn poll_context_provider(&mut self) {
        if let Some(provider) = &self.shared.context_provider {
            if let Some(text) = provider.poll().await {
                let _ = self.memory.remember(Message::user(text));
            }
        }
    }

    async fn current_allowed_tools(&self) -> Option<Vec<String>> {
        let rt = self.shared.skill_runtime.as_ref()?;
        let guard = rt.lock().await;
        Some(guard.allowed_tools_for_current_step())
    }

    fn visible_tools(&self, allowed: Option<&[String]>) -> Vec<ToolDefinition> {
        let all = self.toolset.tools();
        match allowed {
            None => all,
            Some(names) => all
                .into_iter()
                .filter(|t| names.iter().any(|n| n == &t.name))
                .collect(),
        }
    }

    async fn build_context(&mut self) -> Result<Vec<Message>> {
        use crate::prompt::context::Context;

        let mut builder =
            system_prompt_builder::SystemPromptBuilder::default().build_tooluse_guidance();

        if let Some(ref identity) = self.shared.system_prompt_identity {
            builder = builder.with_identity(identity);
        }

        // Inject the agent's own metadata so it knows its identity in
        // multi-agent interactions. The agent uses this name when other
        // agents delegate tasks to it, and to avoid self-delegation.
        builder = builder.with_agent_metadata(format!(
            "## Your Identity\n\
             Your name is **{}**. \
             When other agents delegate tasks to you, they address you by this name. \
             Do not delegate tasks to yourself — use `list_agents` or `route_task` \
             to find a *different* agent suited for the task.",
            self.shared.name
        ));

        if let Some(ref extra) = self.shared.system_prompt_section {
            builder = builder.with_extra_section(extra);
        }

        if let Some(rt) = &self.shared.skill_runtime {
            let section = rt.lock().await.current_prompt_section();
            if !section.is_empty() {
                builder = builder.with_extra_section(section);
            }
        }

        // Inject the current plan status so the model knows what it has
        // committed to and where it left off.
        let plan = self.shared.plan_snapshot();
        if !plan.is_empty() {
            builder = builder.with_extra_section(render_plan_prompt_section(&plan));
        }

        let system_prompt = builder.parse();
        let context_messages = self.memory.render_context()?.to_vec();

        let context = Context::new()
            .with_system_prompt(system_prompt)
            .with_conversations(context_messages)
            .build();

        Ok(context)
    }

    async fn request(
        &mut self,
        mut context: Vec<Message>,
        allowed: Option<&[String]>,
    ) -> Result<Message> {
        let span = span!(Level::TRACE, "API Request");
        let _enter = span.enter();

        let model = self
            .shared
            .model
            .load_full()
            .ok_or_else(|| AgentError::MissingConfig("no active model configured".into()))?;

        let conversation_msgs = self.memory.render_context()?;
        if self.token_budget.should_compact(
            &conversation_msgs,
            model.model_info.context_length,
            model.model_info.max_output_tokens,
        ) {
            tracing::debug!(
                context_length = model.model_info.context_length,
                max_output_tokens = model.model_info.max_output_tokens,
                "context pressure detected, compacting"
            );
            self.shared.send_event(AgentEvent::Compact {
                event: CompactEvent::CompactStart { ts: Utc::now() },
            });
            let compacted = self.memory.compact(model.as_ref()).await?;
            self.shared.send_event(AgentEvent::Compact {
                event: CompactEvent::CompactFinish { ts: Utc::now() },
            });
            if compacted {
                context = self.build_context().await?;
            } else {
                tracing::warn!(
                    "context pressure detected but nothing to compact; \
                     proceeding with request (current segment may overflow)"
                );
            }
        }

        let all_tools = self.visible_tools(allowed);

        let mut stream = model.request_stream(context, &all_tools).await?;

        while let Some(event) = stream.next().await {
            let stream_event = match event {
                Ok(e) => e,
                Err(e) => match &e {
                    AnthropicError::StreamError(msg) if msg.starts_with("Stream lagged:") => {
                        tracing::debug!("skipping lagged event: {e}");
                        continue;
                    }
                    _ => {
                        tracing::warn!("stream event error: {e}; breaking stream loop");
                        break;
                    }
                },
            };

            if let Some(agent_event) = AgentEvent::from_stream_event(&stream_event) {
                self.shared.send_event(agent_event);
            }
        }
        tracing::info!("stream loop exited, awaiting final_message");

        let response =
            tokio::time::timeout(std::time::Duration::from_secs(5), stream.final_message())
                .await
                .map_err(|e| AgentError::WorkflowFailed {
                    iteration: 0,
                    error: Box::new(AgentError::MissingConfig(format!(
                        "final_message() timed out: {e}"
                    ))),
                })??;

        tracing::debug!(summary = response.log_summary(), "LLM response");

        Ok(response)
    }

    fn extract_toolcalls(&self, message: &Message) -> Vec<ToolUse> {
        message
            .content
            .iter()
            .filter_map(|c| {
                if let ContentBlock::ToolUse { id, name, input } = c {
                    Some(ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    })
                } else {
                    None
                }
            })
            .collect()
    }
}

// ─────────────────────────── SessionInfo ───────────────────────────

impl From<&Session> for SessionInfo {
    fn from(s: &Session) -> Self {
        Self {
            id: s.id,
            title: s.title.clone(),
            created_at: s.created_at,
            last_active: s.last_active,
        }
    }
}

// Suppress unused import warning for ArcSwap (used in AgentShared.event_tx type).
#[allow(unused_imports)]
use ArcSwap as _ArcSwap;

/// Render the current plan as a system-prompt section.
///
/// This is injected into every LLM call so the model remembers its own todo
/// list across turns and context compaction.
fn render_plan_prompt_section(plan: &AgentPlan) -> String {
    use agentik_types::StepStatus;

    let mut s = String::from("## Current plan status\n");
    s.push_str("Your persistent plan (survives across turns). ");
    s.push_str("Continue from the `in_progress` step.\n\n");

    for (i, step) in plan.update.plan.iter().enumerate() {
        let marker = match step.status {
            StepStatus::Completed => '✔',
            StepStatus::InProgress => '▶',
            StepStatus::Pending => '○',
        };
        s.push_str(&format!("{}. {} {}\n", i + 1, marker, step.step));
    }

    if let Some((done, total)) = plan.progress() {
        s.push_str(&format!("\nProgress: {done}/{total} completed.\n"));
    }

    s
}
